//! Module that defines the public evaluation API of [`Engine`].

use crate::engine::{EvalState, Imports};
use crate::parser::ParseState;
use crate::types::dynamic::Variant;
use crate::{Dynamic, Engine, EvalAltResult, Position, RhaiResult, Scope, AST};
use std::any::type_name;
#[cfg(feature = "no_std")]
use std::prelude::v1::*;

impl Engine {
    /// Evaluate a string.
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() -> Result<(), Box<rhai::EvalAltResult>> {
    /// use rhai::Engine;
    ///
    /// let engine = Engine::new();
    ///
    /// assert_eq!(engine.eval::<i64>("40 + 2")?, 42);
    /// # Ok(())
    /// # }
    /// ```
    #[inline(always)]
    pub fn eval<T: Variant + Clone>(&self, script: &str) -> Result<T, Box<EvalAltResult>> {
        self.eval_with_scope(&mut Scope::new(), script)
    }
    /// Evaluate a string with own scope.
    ///
    /// ## Constants Propagation
    ///
    /// If not [`OptimizationLevel::None`][crate::OptimizationLevel::None], constants defined within
    /// the scope are propagated throughout the script _including_ functions. This allows functions
    /// to be optimized based on dynamic global constants.
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() -> Result<(), Box<rhai::EvalAltResult>> {
    /// use rhai::{Engine, Scope};
    ///
    /// let engine = Engine::new();
    ///
    /// // Create initialized scope
    /// let mut scope = Scope::new();
    /// scope.push("x", 40_i64);
    ///
    /// assert_eq!(engine.eval_with_scope::<i64>(&mut scope, "x += 2; x")?, 42);
    /// assert_eq!(engine.eval_with_scope::<i64>(&mut scope, "x += 2; x")?, 44);
    ///
    /// // The variable in the scope is modified
    /// assert_eq!(scope.get_value::<i64>("x").expect("variable x should exist"), 44);
    /// # Ok(())
    /// # }
    /// ```
    #[inline]
    pub fn eval_with_scope<T: Variant + Clone>(
        &self,
        scope: &mut Scope,
        script: &str,
    ) -> Result<T, Box<EvalAltResult>> {
        let ast = self.compile_with_scope_and_optimization_level(
            scope,
            &[script],
            #[cfg(not(feature = "no_optimize"))]
            self.optimization_level,
        )?;
        self.eval_ast_with_scope(scope, &ast)
    }
    /// Evaluate a string containing an expression.
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() -> Result<(), Box<rhai::EvalAltResult>> {
    /// use rhai::Engine;
    ///
    /// let engine = Engine::new();
    ///
    /// assert_eq!(engine.eval_expression::<i64>("40 + 2")?, 42);
    /// # Ok(())
    /// # }
    /// ```
    #[inline(always)]
    pub fn eval_expression<T: Variant + Clone>(
        &self,
        script: &str,
    ) -> Result<T, Box<EvalAltResult>> {
        self.eval_expression_with_scope(&mut Scope::new(), script)
    }
    /// Evaluate a string containing an expression with own scope.
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() -> Result<(), Box<rhai::EvalAltResult>> {
    /// use rhai::{Engine, Scope};
    ///
    /// let engine = Engine::new();
    ///
    /// // Create initialized scope
    /// let mut scope = Scope::new();
    /// scope.push("x", 40_i64);
    ///
    /// assert_eq!(engine.eval_expression_with_scope::<i64>(&mut scope, "x + 2")?, 42);
    /// # Ok(())
    /// # }
    /// ```
    #[inline]
    pub fn eval_expression_with_scope<T: Variant + Clone>(
        &self,
        scope: &mut Scope,
        script: &str,
    ) -> Result<T, Box<EvalAltResult>> {
        let scripts = [script];
        let (stream, tokenizer_control) =
            self.lex_raw(&scripts, self.token_mapper.as_ref().map(Box::as_ref));
        let mut state = ParseState::new(self, tokenizer_control);

        // No need to optimize a lone expression
        let ast = self.parse_global_expr(
            &mut stream.peekable(),
            &mut state,
            scope,
            #[cfg(not(feature = "no_optimize"))]
            crate::OptimizationLevel::None,
        )?;

        self.eval_ast_with_scope(scope, &ast)
    }
    /// Evaluate an [`AST`].
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() -> Result<(), Box<rhai::EvalAltResult>> {
    /// use rhai::Engine;
    ///
    /// let engine = Engine::new();
    ///
    /// // Compile a script to an AST and store it for later evaluation
    /// let ast = engine.compile("40 + 2")?;
    ///
    /// // Evaluate it
    /// assert_eq!(engine.eval_ast::<i64>(&ast)?, 42);
    /// # Ok(())
    /// # }
    /// ```
    #[inline(always)]
    pub fn eval_ast<T: Variant + Clone>(&self, ast: &AST) -> Result<T, Box<EvalAltResult>> {
        self.eval_ast_with_scope(&mut Scope::new(), ast)
    }
    /// Evaluate an [`AST`] with own scope.
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() -> Result<(), Box<rhai::EvalAltResult>> {
    /// use rhai::{Engine, Scope};
    ///
    /// let engine = Engine::new();
    ///
    /// // Compile a script to an AST and store it for later evaluation
    /// let ast = engine.compile("x + 2")?;
    ///
    /// // Create initialized scope
    /// let mut scope = Scope::new();
    /// scope.push("x", 40_i64);
    ///
    /// // Compile a script to an AST and store it for later evaluation
    /// let ast = engine.compile("x += 2; x")?;
    ///
    /// // Evaluate it
    /// assert_eq!(engine.eval_ast_with_scope::<i64>(&mut scope, &ast)?, 42);
    /// assert_eq!(engine.eval_ast_with_scope::<i64>(&mut scope, &ast)?, 44);
    ///
    /// // The variable in the scope is modified
    /// assert_eq!(scope.get_value::<i64>("x").expect("variable x should exist"), 44);
    /// # Ok(())
    /// # }
    /// ```
    #[inline]
    pub fn eval_ast_with_scope<T: Variant + Clone>(
        &self,
        scope: &mut Scope,
        ast: &AST,
    ) -> Result<T, Box<EvalAltResult>> {
        let mods = &mut Imports::new();

        let result = self.eval_ast_with_scope_raw(scope, mods, ast, 0)?;

        let typ = self.map_type_name(result.type_name());

        result.try_cast::<T>().ok_or_else(|| {
            EvalAltResult::ErrorMismatchOutputType(
                self.map_type_name(type_name::<T>()).into(),
                typ.into(),
                Position::NONE,
            )
            .into()
        })
    }
    /// Evaluate an [`AST`] with own scope.
    #[inline]
    pub(crate) fn eval_ast_with_scope_raw<'a>(
        &self,
        scope: &mut Scope,
        mods: &mut Imports,
        ast: &'a AST,
        level: usize,
    ) -> RhaiResult {
        let mut state = EvalState::new();
        if ast.source_raw().is_some() {
            mods.source = ast.source_raw().cloned();
        }
        #[cfg(not(feature = "no_module"))]
        {
            mods.embedded_module_resolver = ast.resolver();
        }

        let statements = ast.statements();

        if statements.is_empty() {
            return Ok(Dynamic::UNIT);
        }

        let lib = &[ast.lib()];
        self.eval_global_statements(scope, mods, &mut state, statements, lib, level)
    }
}
