//! Module that defines the `call_fn` API of [`Engine`].
#![cfg(not(feature = "no_function"))]

use crate::engine::{EvalState, Imports};
use crate::types::dynamic::Variant;
use crate::{
    Dynamic, Engine, EvalAltResult, FuncArgs, Position, RhaiResult, Scope, StaticVec, AST,
};
use std::any::type_name;
#[cfg(feature = "no_std")]
use std::prelude::v1::*;

impl Engine {
    /// Call a script function defined in an [`AST`] with multiple arguments.
    ///
    /// Not available under `no_function`.
    ///
    /// The [`AST`] is evaluated before calling the function.
    /// This allows a script to load the necessary modules.
    /// This is usually desired. If not, a specialized [`AST`] can be prepared that contains only
    /// function definitions without any body script via [`AST::clear_statements`].
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() -> Result<(), Box<rhai::EvalAltResult>> {
    /// # #[cfg(not(feature = "no_function"))]
    /// # {
    /// use rhai::{Engine, Scope};
    ///
    /// let engine = Engine::new();
    ///
    /// let ast = engine.compile("
    ///     fn add(x, y) { len(x) + y + foo }
    ///     fn add1(x)   { len(x) + 1 + foo }
    ///     fn bar()     { foo/2 }
    /// ")?;
    ///
    /// let mut scope = Scope::new();
    /// scope.push("foo", 42_i64);
    ///
    /// // Call the script-defined function
    /// let result: i64 = engine.call_fn(&mut scope, &ast, "add", ( "abc", 123_i64 ) )?;
    /// assert_eq!(result, 168);
    ///
    /// let result: i64 = engine.call_fn(&mut scope, &ast, "add1", ( "abc", ) )?;
    /// //                                                         ^^^^^^^^^^ tuple of one
    /// assert_eq!(result, 46);
    ///
    /// let result: i64 = engine.call_fn(&mut scope, &ast, "bar", () )?;
    /// assert_eq!(result, 21);
    /// # }
    /// # Ok(())
    /// # }
    /// ```
    #[inline]
    pub fn call_fn<T: Variant + Clone>(
        &self,
        scope: &mut Scope,
        ast: &AST,
        name: impl AsRef<str>,
        args: impl FuncArgs,
    ) -> Result<T, Box<EvalAltResult>> {
        let mut arg_values = StaticVec::new_const();
        args.parse(&mut arg_values);

        let result = self.call_fn_raw(scope, ast, true, true, name, None, arg_values)?;

        let typ = self.map_type_name(result.type_name());

        result.try_cast().ok_or_else(|| {
            EvalAltResult::ErrorMismatchOutputType(
                self.map_type_name(type_name::<T>()).into(),
                typ.into(),
                Position::NONE,
            )
            .into()
        })
    }
    /// Call a script function defined in an [`AST`] with multiple [`Dynamic`] arguments
    /// and the following options:
    ///
    /// * whether to evaluate the [`AST`] to load necessary modules before calling the function
    /// * whether to rewind the [`Scope`] after the function call
    /// * a value for binding to the `this` pointer (if any)
    ///
    /// Not available under `no_function`.
    ///
    /// # WARNING - Low Level API
    ///
    /// This function is very low level.
    ///
    /// ## Arguments
    ///
    /// All the arguments are _consumed_, meaning that they're replaced by `()`.
    /// This is to avoid unnecessarily cloning the arguments.
    /// Do not use the arguments after this call. If they are needed afterwards,
    /// clone them _before_ calling this function.
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() -> Result<(), Box<rhai::EvalAltResult>> {
    /// # #[cfg(not(feature = "no_function"))]
    /// # {
    /// use rhai::{Engine, Scope, Dynamic};
    ///
    /// let engine = Engine::new();
    ///
    /// let ast = engine.compile("
    ///     fn add(x, y) { len(x) + y + foo }
    ///     fn add1(x)   { len(x) + 1 + foo }
    ///     fn bar()     { foo/2 }
    ///     fn action(x) { this += x; }         // function using 'this' pointer
    ///     fn decl(x)   { let hello = x; }     // declaring variables
    /// ")?;
    ///
    /// let mut scope = Scope::new();
    /// scope.push("foo", 42_i64);
    ///
    /// // Call the script-defined function
    /// let result = engine.call_fn_raw(&mut scope, &ast, true, true, "add", None, [ "abc".into(), 123_i64.into() ])?;
    /// //                                                                   ^^^^ no 'this' pointer
    /// assert_eq!(result.cast::<i64>(), 168);
    ///
    /// let result = engine.call_fn_raw(&mut scope, &ast, true, true, "add1", None, [ "abc".into() ])?;
    /// assert_eq!(result.cast::<i64>(), 46);
    ///
    /// let result = engine.call_fn_raw(&mut scope, &ast, true, true, "bar", None, [])?;
    /// assert_eq!(result.cast::<i64>(), 21);
    ///
    /// let mut value: Dynamic = 1_i64.into();
    /// let result = engine.call_fn_raw(&mut scope, &ast, true, true, "action", Some(&mut value), [ 41_i64.into() ])?;
    /// //                                                                      ^^^^^^^^^^^^^^^^ binding the 'this' pointer
    /// assert_eq!(value.as_int().expect("value should be INT"), 42);
    ///
    /// engine.call_fn_raw(&mut scope, &ast, true, false, "decl", None, [ 42_i64.into() ])?;
    /// //                                         ^^^^^ do not rewind scope
    /// assert_eq!(scope.get_value::<i64>("hello").unwrap(), 42);
    /// # }
    /// # Ok(())
    /// # }
    /// ```
    #[inline]
    pub fn call_fn_raw(
        &self,
        scope: &mut Scope,
        ast: &AST,
        eval_ast: bool,
        rewind_scope: bool,
        name: impl AsRef<str>,
        this_ptr: Option<&mut Dynamic>,
        arg_values: impl AsMut<[Dynamic]>,
    ) -> RhaiResult {
        let state = &mut EvalState::new();
        let mods = &mut Imports::new();
        let statements = ast.statements();

        let orig_scope_len = scope.len();

        if eval_ast && !statements.is_empty() {
            // Make sure new variables introduced at global level do not _spill_ into the function call
            self.eval_global_statements(scope, mods, state, statements, &[ast.as_ref()], 0)?;

            if rewind_scope {
                scope.rewind(orig_scope_len);
            }
        }

        let name = name.as_ref();
        let mut this_ptr = this_ptr;
        let mut arg_values = arg_values;
        let mut args: StaticVec<_> = arg_values.as_mut().iter_mut().collect();

        let fn_def = ast
            .shared_lib()
            .get_script_fn(name, args.len())
            .ok_or_else(|| EvalAltResult::ErrorFunctionNotFound(name.into(), Position::NONE))?;

        // Check for data race.
        #[cfg(not(feature = "no_closure"))]
        crate::func::call::ensure_no_data_race(name, &mut args, false)?;

        let result = self.call_script_fn(
            scope,
            mods,
            state,
            &[ast.as_ref()],
            &mut this_ptr,
            fn_def,
            &mut args,
            Position::NONE,
            rewind_scope,
            0,
        );

        result
    }
}
