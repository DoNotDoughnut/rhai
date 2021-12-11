//! Implement script function-calling mechanism for [`Engine`].
#![cfg(not(feature = "no_function"))]

use crate::ast::ScriptFnDef;
use crate::engine::{EvalState, Imports};
use crate::func::call::FnCallArgs;
use crate::r#unsafe::unsafe_cast_var_name_to_lifetime;
use crate::{Dynamic, Engine, EvalAltResult, Module, Position, RhaiResult, Scope, StaticVec};
use std::mem;
#[cfg(feature = "no_std")]
use std::prelude::v1::*;

impl Engine {
    /// Call a script-defined function.
    ///
    /// If `rewind_scope` is `false`, arguments are removed from the scope but new variables are not.
    ///
    /// # WARNING
    ///
    /// Function call arguments may be _consumed_ when the function requires them to be passed by value.
    /// All function arguments not in the first position are always passed by value and thus consumed.
    ///
    /// **DO NOT** reuse the argument values unless for the first `&mut` argument - all others are silently replaced by `()`!
    pub(crate) fn call_script_fn(
        &self,
        scope: &mut Scope,
        mods: &mut Imports,
        state: &mut EvalState,
        lib: &[&Module],
        this_ptr: &mut Option<&mut Dynamic>,
        fn_def: &ScriptFnDef,
        args: &mut FnCallArgs,
        pos: Position,
        rewind_scope: bool,
        level: usize,
    ) -> RhaiResult {
        #[inline(never)]
        fn make_error(
            name: String,
            fn_def: &ScriptFnDef,
            mods: &Imports,
            err: Box<EvalAltResult>,
            pos: Position,
        ) -> RhaiResult {
            Err(EvalAltResult::ErrorInFunctionCall(
                name,
                fn_def
                    .lib
                    .as_ref()
                    .and_then(|m| m.id().map(|id| id.to_string()))
                    .or_else(|| mods.source.as_ref().map(|s| s.to_string()))
                    .unwrap_or_default(),
                err,
                pos,
            )
            .into())
        }

        assert!(fn_def.params.len() == args.len());

        #[cfg(not(feature = "unchecked"))]
        self.inc_operations(&mut mods.num_operations, pos)?;

        if fn_def.body.is_empty() {
            return Ok(Dynamic::UNIT);
        }

        // Check for stack overflow
        #[cfg(not(feature = "unchecked"))]
        if level > self.max_call_levels() {
            return Err(EvalAltResult::ErrorStackOverflow(pos).into());
        }

        let orig_scope_len = scope.len();
        let orig_mods_len = mods.len();

        // Put arguments into scope as variables
        // Actually consume the arguments instead of cloning them
        scope.extend(
            fn_def
                .params
                .iter()
                .zip(args.iter_mut().map(|v| mem::take(*v)))
                .map(|(name, value)| {
                    let var_name: std::borrow::Cow<'_, str> =
                        unsafe_cast_var_name_to_lifetime(name).into();
                    (var_name, value)
                }),
        );

        // Merge in encapsulated environment, if any
        let mut lib_merged = StaticVec::with_capacity(lib.len() + 1);
        let orig_fn_resolution_caches_len = state.fn_resolution_caches_len();

        let lib = if let Some(ref fn_lib) = fn_def.lib {
            if fn_lib.is_empty() {
                lib
            } else {
                state.push_fn_resolution_cache();
                lib_merged.push(fn_lib.as_ref());
                lib_merged.extend(lib.iter().cloned());
                &lib_merged
            }
        } else {
            lib
        };

        #[cfg(not(feature = "no_module"))]
        if !fn_def.mods.is_empty() {
            fn_def
                .mods
                .iter_raw()
                .for_each(|(n, m)| mods.push(n.clone(), m.clone()));
        }

        // Evaluate the function
        let body = &fn_def.body;
        let result = self
            .eval_stmt_block(
                scope,
                mods,
                state,
                lib,
                this_ptr,
                body,
                true,
                rewind_scope,
                level,
            )
            .or_else(|err| match *err {
                // Convert return statement to return value
                EvalAltResult::Return(x, _) => Ok(x),
                // Error in sub function call
                EvalAltResult::ErrorInFunctionCall(name, src, err, _) => {
                    let fn_name = if src.is_empty() {
                        format!("{} < {}", name, fn_def.name)
                    } else {
                        format!("{} @ '{}' < {}", name, src, fn_def.name)
                    };

                    make_error(fn_name, fn_def, mods, err, pos)
                }
                // System errors are passed straight-through
                mut err if err.is_system_exception() => {
                    err.set_position(pos);
                    Err(err.into())
                }
                // Other errors are wrapped in `ErrorInFunctionCall`
                _ => make_error(fn_def.name.to_string(), fn_def, mods, err, pos),
            });

        // Remove all local variables
        if rewind_scope {
            scope.rewind(orig_scope_len);
        } else if !args.is_empty() {
            // Remove arguments only, leaving new variables in the scope
            scope.remove_range(orig_scope_len, args.len())
        }

        mods.truncate(orig_mods_len);
        state.rewind_fn_resolution_caches(orig_fn_resolution_caches_len);

        result
    }

    // Does a scripted function exist?
    #[must_use]
    pub(crate) fn has_script_fn(
        &self,
        mods: Option<&Imports>,
        state: &mut EvalState,
        lib: &[&Module],
        hash_script: u64,
    ) -> bool {
        let cache = state.fn_resolution_cache_mut();

        if let Some(result) = cache.get(&hash_script).map(|v| v.is_some()) {
            return result;
        }

        // First check script-defined functions
        let result = lib.iter().any(|&m| m.contains_fn(hash_script))
            // Then check the global namespace and packages
            || self.global_modules.iter().any(|m| m.contains_fn(hash_script))
            // Then check imported modules
            || mods.map_or(false, |m| m.contains_fn(hash_script))
            // Then check sub-modules
            || self.global_sub_modules.values().any(|m| m.contains_qualified_fn(hash_script));

        if !result {
            cache.insert(hash_script, None);
        }

        result
    }
}
