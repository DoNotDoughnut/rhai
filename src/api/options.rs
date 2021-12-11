//! Settings for [`Engine`]'s language options.

use crate::Engine;
#[cfg(feature = "no_std")]
use std::prelude::v1::*;

/// A type containing all language options for the [`Engine`].
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub struct LanguageOptions {
    /// Is `if`-expression allowed?
    pub allow_if_expr: bool,
    /// Is `switch` expression allowed?
    pub allow_switch_expr: bool,
    /// Is statement-expression allowed?
    pub allow_stmt_expr: bool,
    /// Is anonymous function allowed?
    #[cfg(not(feature = "no_function"))]
    pub allow_anonymous_fn: bool,
    /// Is looping allowed?
    pub allow_loop: bool,
    /// Strict variables mode?
    pub strict_var: bool,
}

impl LanguageOptions {
    /// Create a new [`Options`] with default values.
    #[inline(always)]
    pub const fn new() -> Self {
        Self {
            allow_if_expr: true,
            allow_switch_expr: true,
            allow_stmt_expr: true,
            #[cfg(not(feature = "no_function"))]
            allow_anonymous_fn: true,
            allow_loop: true,
            strict_var: false,
        }
    }
}

impl Default for LanguageOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    /// Is `if`-expression allowed?
    #[inline(always)]
    pub fn allow_if_expression(&self) -> bool {
        self.options.allow_if_expr
    }
    /// Set whether `if`-expression is allowed.
    #[inline(always)]
    pub fn set_allow_if_expression(&mut self, enable: bool) {
        self.options.allow_if_expr = enable;
    }
    /// Is `switch` expression allowed?
    #[inline(always)]
    pub fn allow_switch_expression(&self) -> bool {
        self.options.allow_switch_expr
    }
    /// Set whether `switch` expression is allowed.
    #[inline(always)]
    pub fn set_allow_switch_expression(&mut self, enable: bool) {
        self.options.allow_switch_expr = enable;
    }
    /// Is statement-expression allowed?
    #[inline(always)]
    pub fn allow_statement_expression(&self) -> bool {
        self.options.allow_stmt_expr
    }
    /// Set whether statement-expression is allowed.
    #[inline(always)]
    pub fn set_allow_statement_expression(&mut self, enable: bool) {
        self.options.allow_stmt_expr = enable;
    }
    /// Is anonymous function allowed?
    ///
    /// Not available under `no_function`.
    #[cfg(not(feature = "no_function"))]
    #[inline(always)]
    pub fn allow_anonymous_fn(&self) -> bool {
        self.options.allow_anonymous_fn
    }
    /// Set whether anonymous function is allowed.
    ///
    /// Not available under `no_function`.
    #[cfg(not(feature = "no_function"))]
    #[inline(always)]
    pub fn set_allow_anonymous_fn(&mut self, enable: bool) {
        self.options.allow_anonymous_fn = enable;
    }
    /// Is looping allowed?
    #[inline(always)]
    pub fn allow_looping(&self) -> bool {
        self.options.allow_loop
    }
    /// Set whether looping is allowed.
    #[inline(always)]
    pub fn set_allow_looping(&mut self, enable: bool) {
        self.options.allow_loop = enable;
    }
    /// Is strict variables mode enabled?
    #[inline(always)]
    pub fn strict_variables(&self) -> bool {
        self.options.strict_var
    }
    /// Set whether strict variables mode is enabled.
    #[inline(always)]
    pub fn set_strict_variables(&mut self, enable: bool) {
        self.options.strict_var = enable;
    }
}
