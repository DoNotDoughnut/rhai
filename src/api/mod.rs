//! Module defining the public API of the Rhai engine.

pub mod eval;

pub mod run;

pub mod compile;

pub mod files;

pub mod register;

pub mod call_fn;

pub mod options;

pub mod limits;

pub mod events;

pub mod deprecated;

use crate::engine::Precedence;
use crate::tokenizer::Token;
use crate::{Engine, Identifier};

#[cfg(feature = "no_std")]
use std::prelude::v1::*;

/// Script optimization API.
#[cfg(not(feature = "no_optimize"))]
impl Engine {
    /// Control whether and how the [`Engine`] will optimize an [`AST`][crate::AST] after compilation.
    ///
    /// Not available under `no_optimize`.
    #[inline(always)]
    pub fn set_optimization_level(
        &mut self,
        optimization_level: crate::OptimizationLevel,
    ) -> &mut Self {
        self.optimization_level = optimization_level;
        self
    }
    /// The current optimization level.
    /// It controls whether and how the [`Engine`] will optimize an [`AST`][crate::AST] after compilation.
    ///
    /// Not available under `no_optimize`.
    #[inline(always)]
    #[must_use]
    pub const fn optimization_level(&self) -> crate::OptimizationLevel {
        self.optimization_level
    }
    /// Optimize the [`AST`][crate::AST] with constants defined in an external Scope. An optimized
    /// copy of the [`AST`][crate::AST] is returned while the original [`AST`][crate::AST] is consumed.
    ///
    /// Not available under `no_optimize`.
    ///
    /// Although optimization is performed by default during compilation, sometimes it is necessary
    /// to _re_-optimize an [`AST`][crate::AST]. For example, when working with constants that are
    /// passed in via an external scope, it will be more efficient to optimize the
    /// [`AST`][crate::AST] once again to take advantage of the new constants.
    ///
    /// With this method, it is no longer necessary to recompile a large script. The script
    /// [`AST`][crate::AST] can be compiled just once. Before evaluation, constants are passed into
    /// the [`Engine`] via an external scope (i.e. with
    /// [`Scope::push_constant`][crate::Scope::push_constant]). Then, the [`AST`][crate::AST] is
    /// cloned and the copy re-optimized before running.
    #[inline]
    #[must_use]
    pub fn optimize_ast(
        &self,
        scope: &crate::Scope,
        ast: crate::AST,
        optimization_level: crate::OptimizationLevel,
    ) -> crate::AST {
        let mut ast = ast;

        #[cfg(not(feature = "no_function"))]
        let lib = ast
            .shared_lib()
            .iter_fn()
            .filter(|f| f.func.is_script())
            .map(|f| {
                f.func
                    .get_script_fn_def()
                    .expect("scripted function")
                    .clone()
            })
            .collect();

        let statements = std::mem::take(ast.statements_mut());

        crate::optimizer::optimize_into_ast(
            self,
            scope,
            statements,
            #[cfg(not(feature = "no_function"))]
            lib,
            optimization_level,
        )
    }
}

impl Engine {
    /// Set the module resolution service used by the [`Engine`].
    ///
    /// Not available under `no_module`.
    #[cfg(not(feature = "no_module"))]
    #[inline(always)]
    pub fn set_module_resolver(
        &mut self,
        resolver: impl crate::ModuleResolver + 'static,
    ) -> &mut Self {
        self.module_resolver = Some(Box::new(resolver));
        self
    }
    /// Disable a particular keyword or operator in the language.
    ///
    /// # Examples
    ///
    /// The following will raise an error during parsing because the `if` keyword is disabled
    /// and is recognized as a reserved symbol!
    ///
    /// ```rust,should_panic
    /// # fn main() -> Result<(), rhai::ParseError> {
    /// use quad_compat_rhai::Engine;
    ///
    /// let mut engine = Engine::new();
    ///
    /// engine.disable_symbol("if");    // disable the 'if' keyword
    ///
    /// engine.compile("let x = if true { 42 } else { 0 };")?;
    /// //                      ^ 'if' is rejected as a reserved symbol
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// The following will raise an error during parsing because the `+=` operator is disabled.
    ///
    /// ```rust,should_panic
    /// # fn main() -> Result<(), rhai::ParseError> {
    /// use quad_compat_rhai::Engine;
    ///
    /// let mut engine = Engine::new();
    ///
    /// engine.disable_symbol("+=");    // disable the '+=' operator
    ///
    /// engine.compile("let x = 42; x += 1;")?;
    /// //                            ^ unknown operator
    /// # Ok(())
    /// # }
    /// ```
    #[inline(always)]
    pub fn disable_symbol(&mut self, symbol: impl Into<Identifier>) -> &mut Self {
        self.disabled_symbols.insert(symbol.into());
        self
    }
    /// Register a custom operator with a precedence into the language.
    ///
    /// The operator can be a valid identifier, a reserved symbol, a disabled operator or a disabled keyword.
    ///
    /// The precedence cannot be zero.
    ///
    /// # Example
    ///
    /// ```rust
    /// # fn main() -> Result<(), Box<rhai::EvalAltResult>> {
    /// use quad_compat_rhai::Engine;
    ///
    /// let mut engine = Engine::new();
    ///
    /// // Register a custom operator called '#' and give it
    /// // a precedence of 160 (i.e. between +|- and *|/).
    /// engine.register_custom_operator("#", 160).expect("should succeed");
    ///
    /// // Register a binary function named '#'
    /// engine.register_fn("#", |x: i64, y: i64| (x * y) - (x + y));
    ///
    /// assert_eq!(
    ///     engine.eval_expression::<i64>("1 + 2 * 3 # 4 - 5 / 6")?,
    ///     15
    /// );
    /// # Ok(())
    /// # }
    /// ```
    pub fn register_custom_operator(
        &mut self,
        keyword: impl AsRef<str> + Into<Identifier>,
        precedence: u8,
    ) -> Result<&mut Self, String> {
        let precedence = Precedence::new(precedence);

        if precedence.is_none() {
            return Err("precedence cannot be zero".into());
        }

        match Token::lookup_from_syntax(keyword.as_ref()) {
            // Standard identifiers, reserved keywords and custom keywords are OK
            None | Some(Token::Reserved(_)) | Some(Token::Custom(_)) => (),
            // Active standard keywords cannot be made custom
            // Disabled keywords are OK
            Some(token) if token.is_standard_keyword() => {
                if !self.disabled_symbols.contains(&*token.syntax()) {
                    return Err(format!("'{}' is a reserved keyword", keyword.as_ref()));
                }
            }
            // Active standard symbols cannot be made custom
            Some(token) if token.is_standard_symbol() => {
                if !self.disabled_symbols.contains(&*token.syntax()) {
                    return Err(format!("'{}' is a reserved operator", keyword.as_ref()));
                }
            }
            // Active standard symbols cannot be made custom
            Some(token) if !self.disabled_symbols.contains(&*token.syntax()) => {
                return Err(format!("'{}' is a reserved symbol", keyword.as_ref()))
            }
            // Disabled symbols are OK
            Some(_) => (),
        }

        // Add to custom keywords
        self.custom_keywords.insert(keyword.into(), precedence);

        Ok(self)
    }
}
