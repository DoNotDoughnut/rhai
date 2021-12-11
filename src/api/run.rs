//! Module that defines the public evaluation API of [`Engine`].

use crate::engine::{EvalState, Imports};
use crate::parser::ParseState;
use crate::{Engine, EvalAltResult, Module, Scope, AST};
#[cfg(feature = "no_std")]
use std::prelude::v1::*;

impl Engine {
    /// Evaluate a script, returning any error (if any).
    #[inline(always)]
    pub fn run(&self, script: &str) -> Result<(), Box<EvalAltResult>> {
        self.run_with_scope(&mut Scope::new(), script)
    }
    /// Evaluate a script with own scope, returning any error (if any).
    ///
    /// ## Constants Propagation
    ///
    /// If not [`OptimizationLevel::None`][crate::OptimizationLevel::None], constants defined within
    /// the scope are propagated throughout the script _including_ functions. This allows functions
    /// to be optimized based on dynamic global constants.
    #[inline]
    pub fn run_with_scope(
        &self,
        scope: &mut Scope,
        script: &str,
    ) -> Result<(), Box<EvalAltResult>> {
        let scripts = [script];
        let (stream, tokenizer_control) =
            self.lex_raw(&scripts, self.token_mapper.as_ref().map(Box::as_ref));
        let mut state = ParseState::new(self, tokenizer_control);

        let ast = self.parse(
            &mut stream.peekable(),
            &mut state,
            scope,
            #[cfg(not(feature = "no_optimize"))]
            self.optimization_level,
        )?;

        self.run_ast_with_scope(scope, &ast)
    }
    /// Evaluate an [`AST`], returning any error (if any).
    #[inline(always)]
    pub fn run_ast(&self, ast: &AST) -> Result<(), Box<EvalAltResult>> {
        self.run_ast_with_scope(&mut Scope::new(), ast)
    }
    /// Evaluate an [`AST`] with own scope, returning any error (if any).
    #[inline]
    pub fn run_ast_with_scope(
        &self,
        scope: &mut Scope,
        ast: &AST,
    ) -> Result<(), Box<EvalAltResult>> {
        let mods = &mut Imports::new();
        let mut state = EvalState::new();
        if ast.source_raw().is_some() {
            mods.source = ast.source_raw().cloned();
        }
        #[cfg(not(feature = "no_module"))]
        {
            mods.embedded_module_resolver = ast.resolver().cloned();
        }

        let statements = ast.statements();
        if !statements.is_empty() {
            let lib = [
                #[cfg(not(feature = "no_function"))]
                ast.as_ref(),
            ];
            let lib = if lib.first().map(|m: &&Module| m.is_empty()).unwrap_or(true) {
                &lib[0..0]
            } else {
                &lib
            };
            self.eval_global_statements(scope, mods, &mut state, statements, lib, 0)?;
        }
        Ok(())
    }
}
