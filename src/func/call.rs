//! Implement function-calling mechanism for [`Engine`].

use super::native::{CallableFunction, FnAny};
use super::{get_builtin_binary_op_fn, get_builtin_op_assignment_fn};
use crate::ast::FnCallHashes;
use crate::engine::{
    EvalState, FnResolutionCacheEntry, Imports, KEYWORD_DEBUG, KEYWORD_EVAL, KEYWORD_FN_PTR,
    KEYWORD_FN_PTR_CALL, KEYWORD_FN_PTR_CURRY, KEYWORD_IS_DEF_VAR, KEYWORD_PRINT, KEYWORD_TYPE_OF,
    MAX_DYNAMIC_PARAMETERS,
};
use crate::module::NamespaceRef;
use crate::tokenizer::Token;
use crate::{
    ast::{Expr, Stmt},
    calc_fn_hash, calc_fn_params_hash, combine_hashes, Dynamic, Engine, EvalAltResult, FnPtr,
    Identifier, ImmutableString, Module, Position, RhaiResult, Scope, StaticVec,
};
#[cfg(feature = "no_std")]
use std::prelude::v1::*;
use std::{
    any::{type_name, TypeId},
    convert::TryFrom,
    mem,
};

#[cfg(not(feature = "no_object"))]
use crate::Map;

/// Arguments to a function call, which is a list of [`&mut Dynamic`][Dynamic].
pub type FnCallArgs<'a> = [&'a mut Dynamic];

/// A type that temporarily stores a mutable reference to a `Dynamic`,
/// replacing it with a cloned copy.
#[derive(Debug)]
struct ArgBackup<'a> {
    orig_mut: Option<&'a mut Dynamic>,
    value_copy: Dynamic,
}

impl<'a> ArgBackup<'a> {
    /// Create a new `ArgBackup`.
    pub fn new() -> Self {
        Self {
            orig_mut: None,
            value_copy: Dynamic::UNIT,
        }
    }
    /// This function replaces the first argument of a method call with a clone copy.
    /// This is to prevent a pure function unintentionally consuming the first argument.
    ///
    /// `restore_first_arg` must be called before the end of the scope to prevent the shorter lifetime from leaking.
    ///
    /// # Safety
    ///
    /// This method blindly casts a reference to another lifetime, which saves allocation and string cloning.
    ///
    /// As long as `restore_first_arg` is called before the end of the scope, the shorter lifetime
    /// will not leak.
    ///
    /// # Panics
    ///
    /// Panics when `args` is empty.
    #[inline]
    fn change_first_arg_to_copy(&mut self, args: &mut FnCallArgs<'a>) {
        // Clone the original value.
        self.value_copy = args[0].clone();

        // Replace the first reference with a reference to the clone, force-casting the lifetime.
        // Must remember to restore it later with `restore_first_arg`.
        //
        // # Safety
        //
        // Blindly casting a reference to another lifetime saves allocation and string cloning,
        // but must be used with the utmost care.
        //
        // We can do this here because, before the end of this scope, we'd restore the original reference
        // via `restore_first_arg`. Therefore this shorter lifetime does not leak.
        self.orig_mut = Some(mem::replace(&mut args[0], unsafe {
            mem::transmute(&mut self.value_copy)
        }));
    }
    /// This function restores the first argument that was replaced by `change_first_arg_to_copy`.
    ///
    /// # Safety
    ///
    /// If `change_first_arg_to_copy` has been called, this function **MUST** be called _BEFORE_ exiting
    /// the current scope.  Otherwise it is undefined behavior as the shorter lifetime will leak.
    #[inline(always)]
    fn restore_first_arg(mut self, args: &mut FnCallArgs<'a>) {
        if let Some(p) = self.orig_mut.take() {
            args[0] = p;
        }
    }
}

impl Drop for ArgBackup<'_> {
    #[inline]
    fn drop(&mut self) {
        // Panic if the shorter lifetime leaks.
        assert!(
            self.orig_mut.is_none(),
            "ArgBackup::restore_first_arg has not been called prior to existing this scope"
        );
    }
}

#[cfg(not(feature = "no_closure"))]
#[inline]
pub fn ensure_no_data_race(
    fn_name: impl AsRef<str>,
    args: &FnCallArgs,
    is_method_call: bool,
) -> Result<(), Box<EvalAltResult>> {
    if let Some((n, _)) = args
        .iter()
        .enumerate()
        .skip(if is_method_call { 1 } else { 0 })
        .find(|(_, a)| a.is_locked())
    {
        return Err(EvalAltResult::ErrorDataRace(
            format!("argument #{} of function '{}'", n + 1, fn_name.as_ref()),
            Position::NONE,
        )
        .into());
    }

    Ok(())
}

impl Engine {
    /// Generate the signature for a function call.
    #[inline]
    #[must_use]
    fn gen_call_signature(
        &self,
        namespace: Option<&NamespaceRef>,
        fn_name: impl AsRef<str>,
        args: &[&mut Dynamic],
    ) -> String {
        format!(
            "{}{}{} ({})",
            namespace.map_or(String::new(), |ns| ns.to_string()),
            if namespace.is_some() {
                Token::DoubleColon.literal_syntax()
            } else {
                ""
            },
            fn_name.as_ref(),
            args.iter()
                .map(|a| if a.is::<ImmutableString>() {
                    "&str | ImmutableString | String"
                } else {
                    self.map_type_name(a.type_name())
                })
                .collect::<StaticVec<_>>()
                .join(", ")
        )
    }

    /// Resolve a function call.
    ///
    /// Search order:
    /// 1) AST - script functions in the AST
    /// 2) Global namespace - functions registered via Engine::register_XXX
    /// 3) Global modules - packages
    /// 4) Imported modules - functions marked with global namespace
    /// 5) Global sub-modules - functions marked with global namespace
    #[must_use]
    fn resolve_fn<'s>(
        &self,
        mods: &Imports,
        state: &'s mut EvalState,
        lib: &[&Module],
        fn_name: impl AsRef<str>,
        hash_script: u64,
        args: Option<&mut FnCallArgs>,
        allow_dynamic: bool,
        is_op_assignment: bool,
    ) -> Option<&'s FnResolutionCacheEntry> {
        let fn_name = fn_name.as_ref();

        let mut hash = args.as_ref().map_or(hash_script, |args| {
            combine_hashes(
                hash_script,
                calc_fn_params_hash(args.iter().map(|a| a.type_id())),
            )
        });

        let result = state
            .fn_resolution_cache_mut()
            .entry(hash)
            .or_insert_with(|| {
                let num_args = args.as_ref().map_or(0, |a| a.len());
                let max_bitmask = if !allow_dynamic {
                    0
                } else {
                    1usize << num_args.min(MAX_DYNAMIC_PARAMETERS)
                };
                let mut bitmask = 1usize; // Bitmask of which parameter to replace with `Dynamic`

                loop {
                    let func = lib
                        .iter()
                        .find_map(|m| {
                            m.get_fn(hash).cloned().map(|func| FnResolutionCacheEntry {
                                func,
                                source: m.id_raw().cloned(),
                            })
                        })
                        .or_else(|| {
                            self.global_modules.iter().find_map(|m| {
                                m.get_fn(hash).cloned().map(|func| FnResolutionCacheEntry {
                                    func,
                                    source: m.id_raw().cloned(),
                                })
                            })
                        })
                        .or_else(|| {
                            mods.get_fn(hash)
                                .map(|(func, source)| FnResolutionCacheEntry {
                                    func: func.clone(),
                                    source: source.cloned(),
                                })
                        })
                        .or_else(|| {
                            self.global_sub_modules.values().find_map(|m| {
                                m.get_qualified_fn(hash).cloned().map(|func| {
                                    FnResolutionCacheEntry {
                                        func,
                                        source: m.id_raw().cloned(),
                                    }
                                })
                            })
                        });

                    match func {
                        // Specific version found
                        Some(f) => return Some(Box::new(f)),

                        // Stop when all permutations are exhausted
                        None if bitmask >= max_bitmask => {
                            if num_args != 2 {
                                return None;
                            }

                            return args.and_then(|args| {
                                if !is_op_assignment {
                                    get_builtin_binary_op_fn(fn_name, &args[0], &args[1]).map(|f| {
                                        FnResolutionCacheEntry {
                                            func: CallableFunction::from_method(
                                                Box::new(f) as Box<FnAny>
                                            ),
                                            source: None,
                                        }
                                    })
                                } else {
                                    let (first_arg, rest_args) =
                                        args.split_first().expect("two arguments");

                                    get_builtin_op_assignment_fn(fn_name, *first_arg, rest_args[0])
                                        .map(|f| FnResolutionCacheEntry {
                                            func: CallableFunction::from_method(
                                                Box::new(f) as Box<FnAny>
                                            ),
                                            source: None,
                                        })
                                }
                                .map(Box::new)
                            });
                        }

                        // Try all permutations with `Dynamic` wildcards
                        None => {
                            let hash_params = calc_fn_params_hash(
                                args.as_ref()
                                    .expect("no permutations")
                                    .iter()
                                    .enumerate()
                                    .map(|(i, a)| {
                                        let mask = 1usize << (num_args - i - 1);
                                        if bitmask & mask != 0 {
                                            // Replace with `Dynamic`
                                            TypeId::of::<Dynamic>()
                                        } else {
                                            a.type_id()
                                        }
                                    }),
                            );
                            hash = combine_hashes(hash_script, hash_params);

                            bitmask += 1;
                        }
                    }
                }
            });

        result.as_ref().map(Box::as_ref)
    }

    /// Call a native Rust function registered with the [`Engine`].
    ///
    /// # WARNING
    ///
    /// Function call arguments be _consumed_ when the function requires them to be passed by value.
    /// All function arguments not in the first position are always passed by value and thus consumed.
    ///
    /// **DO NOT** reuse the argument values unless for the first `&mut` argument - all others are silently replaced by `()`!
    pub(crate) fn call_native_fn(
        &self,
        mods: &mut Imports,
        state: &mut EvalState,
        lib: &[&Module],
        name: impl AsRef<str>,
        hash: u64,
        args: &mut FnCallArgs,
        is_ref_mut: bool,
        is_op_assign: bool,
        pos: Position,
    ) -> Result<(Dynamic, bool), Box<EvalAltResult>> {
        #[cfg(not(feature = "unchecked"))]
        self.inc_operations(&mut mods.num_operations, pos)?;

        let name = name.as_ref();
        let parent_source = mods.source.clone();

        // Check if function access already in the cache
        let func = self.resolve_fn(mods, state, lib, name, hash, Some(args), true, is_op_assign);

        if let Some(FnResolutionCacheEntry { func, source }) = func {
            assert!(func.is_native());

            // Calling pure function but the first argument is a reference?
            let mut backup: Option<ArgBackup> = None;
            if is_ref_mut && func.is_pure() && !args.is_empty() {
                // Clone the first argument
                backup = Some(ArgBackup::new());
                backup
                    .as_mut()
                    .expect("`Some`")
                    .change_first_arg_to_copy(args);
            }

            // Run external function
            let source = source
                .as_ref()
                .or_else(|| parent_source.as_ref())
                .map(|s| s.as_str());

            let context = (self, name, source, &*mods, lib, pos).into();

            let result = if func.is_plugin_fn() {
                func.get_plugin_fn()
                    .expect("plugin function")
                    .call(context, args)
            } else {
                func.get_native_fn().expect("native function")(context, args)
            };

            // Restore the original reference
            if let Some(bk) = backup {
                bk.restore_first_arg(args)
            }

            let result = result.map_err(|err| err.fill_position(pos))?;

            // See if the function match print/debug (which requires special processing)
            return Ok(match name {
                KEYWORD_PRINT => {
                    if let Some(ref print) = self.print {
                        let text = result.into_immutable_string().map_err(|typ| {
                            EvalAltResult::ErrorMismatchOutputType(
                                self.map_type_name(type_name::<ImmutableString>()).into(),
                                typ.into(),
                                pos,
                            )
                        })?;
                        (print(&text).into(), false)
                    } else {
                        (Dynamic::UNIT, false)
                    }
                }
                KEYWORD_DEBUG => {
                    if let Some(ref debug) = self.debug {
                        let text = result.into_immutable_string().map_err(|typ| {
                            EvalAltResult::ErrorMismatchOutputType(
                                self.map_type_name(type_name::<ImmutableString>()).into(),
                                typ.into(),
                                pos,
                            )
                        })?;
                        let source = mods.source.as_ref().map(|s| s.as_str());
                        (debug(&text, source, pos).into(), false)
                    } else {
                        (Dynamic::UNIT, false)
                    }
                }
                _ => (result, func.is_method()),
            });
        }

        // Error handling

        match name {
            // index getter function not found?
            #[cfg(any(not(feature = "no_index"), not(feature = "no_object")))]
            crate::engine::FN_IDX_GET => {
                assert!(args.len() == 2);

                Err(EvalAltResult::ErrorIndexingType(
                    format!(
                        "{} [{}]",
                        self.map_type_name(args[0].type_name()),
                        self.map_type_name(args[1].type_name())
                    ),
                    pos,
                )
                .into())
            }

            // index setter function not found?
            #[cfg(any(not(feature = "no_index"), not(feature = "no_object")))]
            crate::engine::FN_IDX_SET => {
                assert!(args.len() == 3);

                Err(EvalAltResult::ErrorIndexingType(
                    format!(
                        "{} [{}] = {}",
                        self.map_type_name(args[0].type_name()),
                        self.map_type_name(args[1].type_name()),
                        self.map_type_name(args[2].type_name())
                    ),
                    pos,
                )
                .into())
            }

            // Getter function not found?
            #[cfg(not(feature = "no_object"))]
            _ if name.starts_with(crate::engine::FN_GET) => {
                assert!(args.len() == 1);

                Err(EvalAltResult::ErrorDotExpr(
                    format!(
                        "Unknown property '{}' - a getter is not registered for type '{}'",
                        &name[crate::engine::FN_GET.len()..],
                        self.map_type_name(args[0].type_name())
                    ),
                    pos,
                )
                .into())
            }

            // Setter function not found?
            #[cfg(not(feature = "no_object"))]
            _ if name.starts_with(crate::engine::FN_SET) => {
                assert!(args.len() == 2);

                Err(EvalAltResult::ErrorDotExpr(
                    format!(
                        "No writable property '{}' - a setter is not registered for type '{}' to handle '{}'",
                        &name[crate::engine::FN_SET.len()..],
                        self.map_type_name(args[0].type_name()),
                        self.map_type_name(args[1].type_name()),
                    ),
                    pos,
                )
                .into())
            }

            // Raise error
            _ => Err(EvalAltResult::ErrorFunctionNotFound(
                self.gen_call_signature(None, name, args),
                pos,
            )
            .into()),
        }
    }

    /// Perform an actual function call, native Rust or scripted, taking care of special functions.
    ///
    /// # WARNING
    ///
    /// Function call arguments may be _consumed_ when the function requires them to be passed by value.
    /// All function arguments not in the first position are always passed by value and thus consumed.
    ///
    /// **DO NOT** reuse the argument values unless for the first `&mut` argument - all others are silently replaced by `()`!
    pub(crate) fn exec_fn_call(
        &self,
        mods: &mut Imports,
        state: &mut EvalState,
        lib: &[&Module],
        fn_name: impl AsRef<str>,
        hashes: FnCallHashes,
        args: &mut FnCallArgs,
        is_ref_mut: bool,
        is_method_call: bool,
        pos: Position,
        scope: Option<&mut Scope>,
        level: usize,
    ) -> Result<(Dynamic, bool), Box<EvalAltResult>> {
        fn no_method_err(name: &str, pos: Position) -> Result<(Dynamic, bool), Box<EvalAltResult>> {
            let msg = format!("'{0}' should not be called this way. Try {0}(...);", name);
            Err(EvalAltResult::ErrorRuntime(msg.into(), pos).into())
        }

        let fn_name = fn_name.as_ref();

        // Check for data race.
        #[cfg(not(feature = "no_closure"))]
        ensure_no_data_race(fn_name, args, is_ref_mut)?;

        let _scope = scope;
        let _level = level;
        let _is_method_call = is_method_call;

        // These may be redirected from method style calls.
        match fn_name {
            // Handle type_of()
            KEYWORD_TYPE_OF if args.len() == 1 => {
                return Ok((
                    self.map_type_name(args[0].type_name()).to_string().into(),
                    false,
                ))
            }

            // Handle is_def_fn()
            #[cfg(not(feature = "no_function"))]
            crate::engine::KEYWORD_IS_DEF_FN
                if args.len() == 2 && args[0].is::<FnPtr>() && args[1].is::<crate::INT>() =>
            {
                let fn_name = args[0].read_lock::<ImmutableString>().expect("`FnPtr`");
                let num_params = args[1].as_int().expect("`INT`");

                return Ok((
                    if num_params < 0 {
                        false
                    } else {
                        let hash_script = calc_fn_hash(fn_name.as_str(), num_params as usize);
                        self.has_script_fn(Some(mods), state, lib, hash_script)
                    }
                    .into(),
                    false,
                ));
            }

            // Handle is_shared()
            #[cfg(not(feature = "no_closure"))]
            crate::engine::KEYWORD_IS_SHARED if args.len() == 1 => {
                return no_method_err(fn_name, pos)
            }

            KEYWORD_FN_PTR | KEYWORD_EVAL | KEYWORD_IS_DEF_VAR if args.len() == 1 => {
                return no_method_err(fn_name, pos)
            }

            KEYWORD_FN_PTR_CALL | KEYWORD_FN_PTR_CURRY if !args.is_empty() => {
                return no_method_err(fn_name, pos)
            }

            _ => (),
        }

        // Scripted function call?
        #[cfg(not(feature = "no_function"))]
        let hash_script = hashes.script;

        #[cfg(not(feature = "no_function"))]
        if let Some(FnResolutionCacheEntry { func, source }) = hash_script.and_then(|hash| {
            self.resolve_fn(mods, state, lib, fn_name, hash, None, false, false)
                .cloned()
        }) {
            // Script function call
            assert!(func.is_script());

            let func = func.get_script_fn_def().expect("scripted function");

            if func.body.is_empty() {
                return Ok((Dynamic::UNIT, false));
            }

            let mut empty_scope;
            let scope = if let Some(scope) = _scope {
                scope
            } else {
                empty_scope = Scope::new();
                &mut empty_scope
            };

            let result = if _is_method_call {
                // Method call of script function - map first argument to `this`
                let (first_arg, rest_args) = args.split_first_mut().expect("not empty");

                let orig_source = mods.source.take();
                mods.source = source;

                let level = _level + 1;

                let result = self.call_script_fn(
                    scope,
                    mods,
                    state,
                    lib,
                    &mut Some(*first_arg),
                    func,
                    rest_args,
                    pos,
                    true,
                    level,
                );

                // Restore the original source
                mods.source = orig_source;

                result?
            } else {
                // Normal call of script function
                // The first argument is a reference?
                let mut backup: Option<ArgBackup> = None;
                if is_ref_mut && !args.is_empty() {
                    backup = Some(ArgBackup::new());
                    backup
                        .as_mut()
                        .expect("`Some`")
                        .change_first_arg_to_copy(args);
                }

                let orig_source = mods.source.take();
                mods.source = source;

                let level = _level + 1;

                let result = self.call_script_fn(
                    scope, mods, state, lib, &mut None, func, args, pos, true, level,
                );

                // Restore the original source
                mods.source = orig_source;

                // Restore the original reference
                if let Some(bk) = backup {
                    bk.restore_first_arg(args)
                }

                result?
            };

            return Ok((result, false));
        }

        // Native function call
        let hash = hashes.native;
        self.call_native_fn(
            mods, state, lib, fn_name, hash, args, is_ref_mut, false, pos,
        )
    }

    /// Evaluate a list of statements with no `this` pointer.
    /// This is commonly used to evaluate a list of statements in an [`AST`] or a script function body.
    #[inline]
    pub(crate) fn eval_global_statements(
        &self,
        scope: &mut Scope,
        mods: &mut Imports,
        state: &mut EvalState,
        statements: &[Stmt],
        lib: &[&Module],
        level: usize,
    ) -> RhaiResult {
        self.eval_stmt_block(
            scope, mods, state, lib, &mut None, statements, false, false, level,
        )
        .or_else(|err| match *err {
            EvalAltResult::Return(out, _) => Ok(out),
            EvalAltResult::LoopBreak(_, _) => {
                unreachable!("no outer loop scope to break out of")
            }
            _ => Err(err),
        })
    }

    /// Evaluate a text script in place - used primarily for 'eval'.
    fn eval_script_expr_in_place(
        &self,
        scope: &mut Scope,
        mods: &mut Imports,
        lib: &[&Module],
        script: impl AsRef<str>,
        _pos: Position,
        level: usize,
    ) -> RhaiResult {
        #[cfg(not(feature = "unchecked"))]
        self.inc_operations(&mut mods.num_operations, _pos)?;

        let script = script.as_ref().trim();
        if script.is_empty() {
            return Ok(Dynamic::UNIT);
        }

        // Compile the script text
        // No optimizations because we only run it once
        let ast = self.compile_with_scope_and_optimization_level(
            &Scope::new(),
            &[script],
            #[cfg(not(feature = "no_optimize"))]
            crate::OptimizationLevel::None,
        )?;

        // If new functions are defined within the eval string, it is an error
        #[cfg(not(feature = "no_function"))]
        if !ast.shared_lib().is_empty() {
            return Err(crate::ParseErrorType::WrongFnDefinition.into());
        }

        let statements = ast.statements();
        if statements.is_empty() {
            return Ok(Dynamic::UNIT);
        }

        // Evaluate the AST
        self.eval_global_statements(scope, mods, &mut EvalState::new(), statements, lib, level)
    }

    /// Call a dot method.
    #[cfg(not(feature = "no_object"))]
    pub(crate) fn make_method_call(
        &self,
        mods: &mut Imports,
        state: &mut EvalState,
        lib: &[&Module],
        fn_name: impl AsRef<str>,
        mut hash: FnCallHashes,
        target: &mut crate::engine::Target,
        (call_args, call_arg_pos): &mut (StaticVec<Dynamic>, Position),
        pos: Position,
        level: usize,
    ) -> Result<(Dynamic, bool), Box<EvalAltResult>> {
        let fn_name = fn_name.as_ref();
        let is_ref_mut = target.is_ref();

        let (result, updated) = match fn_name {
            KEYWORD_FN_PTR_CALL if target.is::<FnPtr>() => {
                // FnPtr call
                let fn_ptr = target.read_lock::<FnPtr>().expect("`FnPtr`");
                // Redirect function name
                let fn_name = fn_ptr.fn_name();
                let args_len = call_args.len() + fn_ptr.curry().len();
                // Recalculate hashes
                let new_hash = calc_fn_hash(fn_name, args_len).into();
                // Arguments are passed as-is, adding the curried arguments
                let mut curry = StaticVec::with_capacity(fn_ptr.num_curried());
                curry.extend(fn_ptr.curry().iter().cloned());
                let mut args = StaticVec::with_capacity(curry.len() + call_args.len());
                args.extend(curry.iter_mut());
                args.extend(call_args.iter_mut());

                // Map it to name(args) in function-call style
                self.exec_fn_call(
                    mods, state, lib, fn_name, new_hash, &mut args, false, false, pos, None, level,
                )
            }
            KEYWORD_FN_PTR_CALL => {
                if !call_args.is_empty() {
                    if !call_args[0].is::<FnPtr>() {
                        return Err(self.make_type_mismatch_err::<FnPtr>(
                            self.map_type_name(call_args[0].type_name()),
                            *call_arg_pos,
                        ));
                    }
                } else {
                    return Err(self.make_type_mismatch_err::<FnPtr>(
                        self.map_type_name(target.type_name()),
                        pos,
                    ));
                }

                // FnPtr call on object
                let fn_ptr = call_args.remove(0).cast::<FnPtr>();
                // Redirect function name
                let fn_name = fn_ptr.fn_name();
                let args_len = call_args.len() + fn_ptr.curry().len();
                // Recalculate hash
                let new_hash = FnCallHashes::from_all(
                    #[cfg(not(feature = "no_function"))]
                    calc_fn_hash(fn_name, args_len),
                    calc_fn_hash(fn_name, args_len + 1),
                );
                // Replace the first argument with the object pointer, adding the curried arguments
                let mut curry = StaticVec::with_capacity(fn_ptr.num_curried());
                curry.extend(fn_ptr.curry().iter().cloned());
                let mut args = StaticVec::with_capacity(curry.len() + call_args.len() + 1);
                args.push(target.as_mut());
                args.extend(curry.iter_mut());
                args.extend(call_args.iter_mut());

                // Map it to name(args) in function-call style
                self.exec_fn_call(
                    mods, state, lib, fn_name, new_hash, &mut args, is_ref_mut, true, pos, None,
                    level,
                )
            }
            KEYWORD_FN_PTR_CURRY => {
                if !target.is::<FnPtr>() {
                    return Err(self.make_type_mismatch_err::<FnPtr>(
                        self.map_type_name(target.type_name()),
                        pos,
                    ));
                }

                let fn_ptr = target.read_lock::<FnPtr>().expect("`FnPtr`");

                // Curry call
                Ok((
                    if call_args.is_empty() {
                        fn_ptr.clone()
                    } else {
                        FnPtr::new_unchecked(
                            fn_ptr.fn_name_raw().clone(),
                            fn_ptr
                                .curry()
                                .iter()
                                .cloned()
                                .chain(call_args.iter_mut().map(mem::take))
                                .collect(),
                        )
                    }
                    .into(),
                    false,
                ))
            }

            // Handle is_shared()
            #[cfg(not(feature = "no_closure"))]
            crate::engine::KEYWORD_IS_SHARED if call_args.is_empty() => {
                return Ok((target.is_shared().into(), false));
            }

            _ => {
                let mut fn_name = fn_name;
                let _redirected;

                // Check if it is a map method call in OOP style
                #[cfg(not(feature = "no_object"))]
                if let Some(map) = target.read_lock::<Map>() {
                    if let Some(val) = map.get(fn_name) {
                        if let Some(fn_ptr) = val.read_lock::<FnPtr>() {
                            // Remap the function name
                            _redirected = fn_ptr.fn_name_raw().clone();
                            fn_name = &_redirected;
                            // Add curried arguments
                            if fn_ptr.is_curried() {
                                call_args.insert_many(0, fn_ptr.curry().iter().cloned());
                            }
                            // Recalculate the hash based on the new function name and new arguments
                            hash = FnCallHashes::from_all(
                                #[cfg(not(feature = "no_function"))]
                                calc_fn_hash(fn_name, call_args.len()),
                                calc_fn_hash(fn_name, call_args.len() + 1),
                            );
                        }
                    }
                };

                // Attached object pointer in front of the arguments
                let mut args = StaticVec::with_capacity(call_args.len() + 1);
                args.push(target.as_mut());
                args.extend(call_args.iter_mut());

                self.exec_fn_call(
                    mods, state, lib, fn_name, hash, &mut args, is_ref_mut, true, pos, None, level,
                )
            }
        }?;

        // Propagate the changed value back to the source if necessary
        if updated {
            target
                .propagate_changed_value()
                .map_err(|err| err.fill_position(pos))?;
        }

        Ok((result, updated))
    }

    /// Evaluate an argument.
    #[inline]
    pub(crate) fn get_arg_value(
        &self,
        scope: &mut Scope,
        mods: &mut Imports,
        state: &mut EvalState,
        lib: &[&Module],
        this_ptr: &mut Option<&mut Dynamic>,
        level: usize,
        arg_expr: &Expr,
        constants: &[Dynamic],
    ) -> Result<(Dynamic, Position), Box<EvalAltResult>> {
        match arg_expr {
            Expr::Stack(slot, pos) => Ok((constants[*slot].clone(), *pos)),
            ref arg => self
                .eval_expr(scope, mods, state, lib, this_ptr, arg, level)
                .map(|v| (v, arg.position())),
        }
    }

    /// Call a function in normal function-call style.
    pub(crate) fn make_function_call(
        &self,
        scope: &mut Scope,
        mods: &mut Imports,
        state: &mut EvalState,
        lib: &[&Module],
        this_ptr: &mut Option<&mut Dynamic>,
        fn_name: impl AsRef<str>,
        args_expr: &[Expr],
        constants: &[Dynamic],
        hashes: FnCallHashes,
        pos: Position,
        capture_scope: bool,
        level: usize,
    ) -> RhaiResult {
        let fn_name = fn_name.as_ref();
        let mut a_expr = args_expr;
        let mut total_args = a_expr.len();
        let mut curry = StaticVec::new_const();
        let mut name = fn_name;
        let mut hashes = hashes;
        let redirected; // Handle call() - Redirect function call

        match name {
            // Handle call()
            KEYWORD_FN_PTR_CALL if total_args >= 1 => {
                let (arg, arg_pos) = self.get_arg_value(
                    scope, mods, state, lib, this_ptr, level, &a_expr[0], constants,
                )?;

                if !arg.is::<FnPtr>() {
                    return Err(self.make_type_mismatch_err::<FnPtr>(
                        self.map_type_name(arg.type_name()),
                        arg_pos,
                    ));
                }

                let fn_ptr = arg.cast::<FnPtr>();
                curry.extend(fn_ptr.curry().iter().cloned());

                // Redirect function name
                redirected = fn_ptr.take_data().0;
                name = &redirected;

                // Skip the first argument
                a_expr = &a_expr[1..];
                total_args -= 1;

                // Recalculate hash
                let args_len = total_args + curry.len();
                hashes = if !hashes.is_native_only() {
                    calc_fn_hash(name, args_len).into()
                } else {
                    FnCallHashes::from_native(calc_fn_hash(name, args_len))
                };
            }
            // Handle Fn()
            KEYWORD_FN_PTR if total_args == 1 => {
                let (arg, arg_pos) = self.get_arg_value(
                    scope, mods, state, lib, this_ptr, level, &a_expr[0], constants,
                )?;

                // Fn - only in function call style
                return arg
                    .into_immutable_string()
                    .map_err(|typ| self.make_type_mismatch_err::<ImmutableString>(typ, arg_pos))
                    .and_then(FnPtr::try_from)
                    .map(Into::<Dynamic>::into)
                    .map_err(|err| err.fill_position(arg_pos));
            }

            // Handle curry()
            KEYWORD_FN_PTR_CURRY if total_args > 1 => {
                let (arg, arg_pos) = self.get_arg_value(
                    scope, mods, state, lib, this_ptr, level, &a_expr[0], constants,
                )?;

                if !arg.is::<FnPtr>() {
                    return Err(self.make_type_mismatch_err::<FnPtr>(
                        self.map_type_name(arg.type_name()),
                        arg_pos,
                    ));
                }

                let (name, fn_curry) = arg.cast::<FnPtr>().take_data();

                // Append the new curried arguments to the existing list.
                let fn_curry = a_expr.iter().skip(1).try_fold(
                    fn_curry,
                    |mut curried, expr| -> Result<_, Box<EvalAltResult>> {
                        let (value, _) = self.get_arg_value(
                            scope, mods, state, lib, this_ptr, level, expr, constants,
                        )?;
                        curried.push(value);
                        Ok(curried)
                    },
                )?;

                return Ok(FnPtr::new_unchecked(name, fn_curry).into());
            }

            // Handle is_shared()
            #[cfg(not(feature = "no_closure"))]
            crate::engine::KEYWORD_IS_SHARED if total_args == 1 => {
                let (arg, _) = self.get_arg_value(
                    scope, mods, state, lib, this_ptr, level, &a_expr[0], constants,
                )?;
                return Ok(arg.is_shared().into());
            }

            // Handle is_def_fn()
            #[cfg(not(feature = "no_function"))]
            crate::engine::KEYWORD_IS_DEF_FN if total_args == 2 => {
                let (arg, arg_pos) = self.get_arg_value(
                    scope, mods, state, lib, this_ptr, level, &a_expr[0], constants,
                )?;

                let fn_name = arg
                    .into_immutable_string()
                    .map_err(|typ| self.make_type_mismatch_err::<ImmutableString>(typ, arg_pos))?;

                let (arg, arg_pos) = self.get_arg_value(
                    scope, mods, state, lib, this_ptr, level, &a_expr[1], constants,
                )?;

                let num_params = arg
                    .as_int()
                    .map_err(|typ| self.make_type_mismatch_err::<crate::INT>(typ, arg_pos))?;

                return Ok(if num_params < 0 {
                    false
                } else {
                    let hash_script = calc_fn_hash(&fn_name, num_params as usize);
                    self.has_script_fn(Some(mods), state, lib, hash_script)
                }
                .into());
            }

            // Handle is_def_var()
            KEYWORD_IS_DEF_VAR if total_args == 1 => {
                let (arg, arg_pos) = self.get_arg_value(
                    scope, mods, state, lib, this_ptr, level, &a_expr[0], constants,
                )?;
                let var_name = arg
                    .into_immutable_string()
                    .map_err(|typ| self.make_type_mismatch_err::<ImmutableString>(typ, arg_pos))?;
                return Ok(scope.contains(&var_name).into());
            }

            // Handle eval()
            KEYWORD_EVAL if total_args == 1 => {
                // eval - only in function call style
                let orig_scope_len = scope.len();
                let (value, pos) = self.get_arg_value(
                    scope, mods, state, lib, this_ptr, level, &a_expr[0], constants,
                )?;
                let script = &value
                    .into_immutable_string()
                    .map_err(|typ| self.make_type_mismatch_err::<ImmutableString>(typ, pos))?;
                let result =
                    self.eval_script_expr_in_place(scope, mods, lib, script, pos, level + 1);

                // IMPORTANT! If the eval defines new variables in the current scope,
                //            all variable offsets from this point on will be mis-aligned.
                if scope.len() != orig_scope_len {
                    state.always_search_scope = true;
                }

                return result.map_err(|err| {
                    EvalAltResult::ErrorInFunctionCall(
                        KEYWORD_EVAL.to_string(),
                        mods.source
                            .as_ref()
                            .map(Identifier::to_string)
                            .unwrap_or_default(),
                        err,
                        pos,
                    )
                    .into()
                });
            }

            _ => (),
        }

        // Normal function call - except for Fn, curry, call and eval (handled above)
        let mut arg_values = StaticVec::with_capacity(a_expr.len());
        let mut args = StaticVec::with_capacity(a_expr.len() + curry.len());
        let mut is_ref_mut = false;

        // Capture parent scope?
        //
        // If so, do it separately because we cannot convert the first argument (if it is a simple
        // variable access) to &mut because `scope` is needed.
        if capture_scope && !scope.is_empty() {
            a_expr.iter().try_for_each(|expr| {
                self.get_arg_value(scope, mods, state, lib, this_ptr, level, expr, constants)
                    .map(|(value, _)| arg_values.push(value.flatten()))
            })?;
            args.extend(curry.iter_mut());
            args.extend(arg_values.iter_mut());

            // Use parent scope
            let scope = Some(scope);

            return self
                .exec_fn_call(
                    mods, state, lib, name, hashes, &mut args, is_ref_mut, false, pos, scope, level,
                )
                .map(|(v, _)| v);
        }

        // Call with blank scope
        if a_expr.is_empty() && curry.is_empty() {
            // No arguments
        } else {
            // If the first argument is a variable, and there is no curried arguments,
            // convert to method-call style in order to leverage potential &mut first argument and
            // avoid cloning the value
            if curry.is_empty() && !a_expr.is_empty() && a_expr[0].is_variable_access(false) {
                // func(x, ...) -> x.func(...)
                let (first_expr, rest_expr) = a_expr.split_first().expect("not empty");

                rest_expr.iter().try_for_each(|expr| {
                    self.get_arg_value(scope, mods, state, lib, this_ptr, level, expr, constants)
                        .map(|(value, _)| arg_values.push(value.flatten()))
                })?;

                let (mut target, _pos) =
                    self.search_namespace(scope, mods, state, lib, this_ptr, first_expr)?;

                if target.as_ref().is_read_only() {
                    target = target.into_owned();
                }

                #[cfg(not(feature = "unchecked"))]
                self.inc_operations(&mut mods.num_operations, _pos)?;

                #[cfg(not(feature = "no_closure"))]
                let target_is_shared = target.is_shared();
                #[cfg(feature = "no_closure")]
                let target_is_shared = false;

                if target_is_shared || target.is_temp_value() {
                    arg_values.insert(0, target.take_or_clone().flatten());
                    args.extend(arg_values.iter_mut())
                } else {
                    // Turn it into a method call only if the object is not shared and not a simple value
                    is_ref_mut = true;
                    let obj_ref = target.take_ref().expect("reference");
                    args.push(obj_ref);
                    args.extend(arg_values.iter_mut());
                }
            } else {
                // func(..., ...)
                a_expr.iter().try_for_each(|expr| {
                    self.get_arg_value(scope, mods, state, lib, this_ptr, level, expr, constants)
                        .map(|(value, _)| arg_values.push(value.flatten()))
                })?;
                args.extend(curry.iter_mut());
                args.extend(arg_values.iter_mut());
            }
        }

        self.exec_fn_call(
            mods, state, lib, name, hashes, &mut args, is_ref_mut, false, pos, None, level,
        )
        .map(|(v, _)| v)
    }

    /// Call a namespace-qualified function in normal function-call style.
    pub(crate) fn make_qualified_function_call(
        &self,
        scope: &mut Scope,
        mods: &mut Imports,
        state: &mut EvalState,
        lib: &[&Module],
        this_ptr: &mut Option<&mut Dynamic>,
        namespace: &NamespaceRef,
        fn_name: impl AsRef<str>,
        args_expr: &[Expr],
        constants: &[Dynamic],
        hash: u64,
        pos: Position,
        level: usize,
    ) -> RhaiResult {
        let fn_name = fn_name.as_ref();
        let mut arg_values = StaticVec::with_capacity(args_expr.len());
        let mut args = StaticVec::with_capacity(args_expr.len());
        let mut first_arg_value = None;

        if args_expr.is_empty() {
            // No arguments
        } else {
            // See if the first argument is a variable (not namespace-qualified).
            // If so, convert to method-call style in order to leverage potential
            // &mut first argument and avoid cloning the value
            if !args_expr.is_empty() && args_expr[0].is_variable_access(true) {
                // func(x, ...) -> x.func(...)
                arg_values.push(Dynamic::UNIT);

                args_expr.iter().skip(1).try_for_each(|expr| {
                    self.get_arg_value(scope, mods, state, lib, this_ptr, level, expr, constants)
                        .map(|(value, _)| arg_values.push(value.flatten()))
                })?;

                // Get target reference to first argument
                let (target, _pos) =
                    self.search_scope_only(scope, mods, state, lib, this_ptr, &args_expr[0])?;

                #[cfg(not(feature = "unchecked"))]
                self.inc_operations(&mut mods.num_operations, _pos)?;

                #[cfg(not(feature = "no_closure"))]
                let target_is_shared = target.is_shared();
                #[cfg(feature = "no_closure")]
                let target_is_shared = false;

                if target_is_shared || target.is_temp_value() {
                    arg_values[0] = target.take_or_clone().flatten();
                    args.extend(arg_values.iter_mut());
                } else {
                    // Turn it into a method call only if the object is not shared and not a simple value
                    let (first, rest) = arg_values.split_first_mut().expect("not empty");
                    first_arg_value = Some(first);
                    let obj_ref = target.take_ref().expect("reference");
                    args.push(obj_ref);
                    args.extend(rest.iter_mut());
                }
            } else {
                // func(..., ...) or func(mod::x, ...)
                args_expr.iter().try_for_each(|expr| {
                    self.get_arg_value(scope, mods, state, lib, this_ptr, level, expr, constants)
                        .map(|(value, _)| arg_values.push(value.flatten()))
                })?;
                args.extend(arg_values.iter_mut());
            }
        }

        let module = self.search_imports(mods, state, namespace).ok_or_else(|| {
            EvalAltResult::ErrorModuleNotFound(namespace.to_string(), namespace[0].pos)
        })?;

        // First search in script-defined functions (can override built-in)
        let func = match module.get_qualified_fn(hash) {
            // Then search in Rust functions
            None => {
                #[cfg(not(feature = "unchecked"))]
                self.inc_operations(&mut mods.num_operations, pos)?;

                let hash_params = calc_fn_params_hash(args.iter().map(|a| a.type_id()));
                let hash_qualified_fn = combine_hashes(hash, hash_params);

                module.get_qualified_fn(hash_qualified_fn)
            }
            r => r,
        };

        // Clone first argument if the function is not a method after-all
        if !func.map(|f| f.is_method()).unwrap_or(true) {
            if let Some(first) = first_arg_value {
                *first = args[0].clone();
                args[0] = first;
            }
        }

        match func {
            #[cfg(not(feature = "no_function"))]
            Some(f) if f.is_script() => {
                let fn_def = f.get_script_fn_def().expect("scripted function");

                if fn_def.body.is_empty() {
                    Ok(Dynamic::UNIT)
                } else {
                    let new_scope = &mut Scope::new();

                    let mut source = module.id_raw().cloned();
                    mem::swap(&mut mods.source, &mut source);

                    let level = level + 1;

                    let result = self.call_script_fn(
                        new_scope, mods, state, lib, &mut None, fn_def, &mut args, pos, true, level,
                    );

                    mods.source = source;

                    result
                }
            }

            Some(f) if f.is_plugin_fn() => {
                let context = (self, fn_name, module.id(), &*mods, lib, pos).into();
                f.get_plugin_fn()
                    .expect("plugin function")
                    .clone()
                    .call(context, &mut args)
                    .map_err(|err| err.fill_position(pos))
            }

            Some(f) if f.is_native() => {
                let func = f.get_native_fn().expect("native function");
                let context = (self, fn_name, module.id(), &*mods, lib, pos).into();
                func(context, &mut args).map_err(|err| err.fill_position(pos))
            }

            Some(f) => unreachable!("unknown function type: {:?}", f),

            None => Err(EvalAltResult::ErrorFunctionNotFound(
                self.gen_call_signature(Some(namespace), fn_name, &args),
                pos,
            )
            .into()),
        }
    }
}
