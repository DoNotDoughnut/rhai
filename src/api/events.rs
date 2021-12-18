//! Module that defines public event handlers for [`Engine`].

use crate::engine::EvalContext;
use crate::func::SendSync;
use crate::{Dynamic, Engine, EvalAltResult, Position};
#[cfg(feature = "no_std")]
use std::prelude::v1::*;

impl Engine {
    /// Provide a callback that will be invoked before each variable access.
    ///
    /// # Callback Function Signature
    ///
    /// The callback function signature takes the following form:
    ///
    /// > `Fn(name: &str, index: usize, context: &EvalContext)`  
    /// > `   -> Result<Option<Dynamic>, Box<EvalAltResult>> + 'static`
    ///
    /// where:
    /// * `index`: an offset from the bottom of the current [`Scope`][crate::Scope] that the
    ///   variable is supposed to reside. Offsets start from 1, with 1 meaning the last variable in
    ///   the current [`Scope`][crate::Scope].  Essentially the correct variable is at position
    ///   `scope.len() - index`. If `index` is zero, then there is no pre-calculated offset position
    ///   and a search through the current [`Scope`][crate::Scope] must be performed.
    ///
    /// * `context`: the current [evaluation context][`EvalContext`].
    ///
    /// ## Return value
    ///
    /// * `Ok(None)`: continue with normal variable access.
    /// * `Ok(Some(Dynamic))`: the variable's value.
    ///
    /// ## Raising errors
    ///
    /// Return `Err(...)` if there is an error.
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() -> Result<(), Box<rhai::EvalAltResult>> {
    /// use quad_compat_rhai::Engine;
    ///
    /// let mut engine = Engine::new();
    ///
    /// // Register a variable resolver.
    /// engine.on_var(|name, _, _| {
    ///     match name {
    ///         "MYSTIC_NUMBER" => Ok(Some(42_i64.into())),
    ///         _ => Ok(None)
    ///     }
    /// });
    ///
    /// engine.eval::<i64>("MYSTIC_NUMBER")?;
    ///
    /// # Ok(())
    /// # }
    /// ```
    #[inline(always)]
    pub fn on_var(
        &mut self,
        callback: impl Fn(&str, usize, &EvalContext) -> Result<Option<Dynamic>, Box<EvalAltResult>>
            + SendSync
            + 'static,
    ) -> &mut Self {
        self.resolve_var = Some(Box::new(callback));
        self
    }
    /// _(internals)_ Provide a callback that will be invoked during parsing to remap certain tokens.
    /// Exported under the `internals` feature only.
    ///
    /// # Callback Function Signature
    ///
    /// The callback function signature takes the following form:
    ///
    /// > `Fn(token: Token, pos: Position, state: &TokenizeState) -> Token`
    ///
    /// where:
    /// * [`token`][crate::tokenizer::Token]: current token parsed
    /// * [`pos`][`Position`]: location of the token
    /// * [`state`][crate::tokenizer::TokenizeState]: current state of the tokenizer
    ///
    /// ## Raising errors
    ///
    /// It is possible to raise a parsing error by returning
    /// [`Token::LexError`][crate::tokenizer::Token::LexError] as the mapped token.
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() -> Result<(), Box<rhai::EvalAltResult>> {
    /// use quad_compat_rhai::{Engine, Token};
    ///
    /// let mut engine = Engine::new();
    ///
    /// // Register a token mapper.
    /// engine.on_parse_token(|token, _, _| {
    ///     match token {
    ///         // Convert all integer literals to strings
    ///         Token::IntegerConstant(n) => Token::StringConstant(n.to_string().into()),
    ///         // Convert 'begin' .. 'end' to '{' .. '}'
    ///         Token::Identifier(s) if &*s == "begin" => Token::LeftBrace,
    ///         Token::Identifier(s) if &*s == "end" => Token::RightBrace,
    ///         // Pass through all other tokens unchanged
    ///         _ => token
    ///     }
    /// });
    ///
    /// assert_eq!(engine.eval::<String>("42")?, "42");
    /// assert_eq!(engine.eval::<bool>("true")?, true);
    /// assert_eq!(engine.eval::<String>("let x = 42; begin let x = 0; end; x")?, "42");
    ///
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "internals")]
    #[inline(always)]
    pub fn on_parse_token(
        &mut self,
        callback: impl Fn(
                crate::tokenizer::Token,
                Position,
                &crate::tokenizer::TokenizeState,
            ) -> crate::tokenizer::Token
            + SendSync
            + 'static,
    ) -> &mut Self {
        self.token_mapper = Some(Box::new(callback));
        self
    }
    /// Register a callback for script evaluation progress.
    ///
    /// Not available under `unchecked`.
    ///
    /// # Callback Function Signature
    ///
    /// The callback function signature takes the following form:
    ///
    /// > `Fn(counter: u64) -> Option<Dynamic>`
    ///
    /// ## Return value
    ///
    /// * `None`: continue running the script.
    /// * `Some(Dynamic)`: terminate the script with the specified exception value.
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() -> Result<(), Box<rhai::EvalAltResult>> {
    /// # use std::sync::RwLock;
    /// # use std::sync::Arc;
    /// use quad_compat_rhai::Engine;
    ///
    /// let result = Arc::new(RwLock::new(0_u64));
    /// let logger = result.clone();
    ///
    /// let mut engine = Engine::new();
    ///
    /// engine.on_progress(move |ops| {
    ///     if ops > 1000 {
    ///         Some("Over 1,000 operations!".into())
    ///     } else if ops % 123 == 0 {
    ///         *logger.write().unwrap() = ops;
    ///         None
    ///     } else {
    ///         None
    ///     }
    /// });
    ///
    /// engine.run("for x in 0..5000 { print(x); }")
    ///       .expect_err("should error");
    ///
    /// assert_eq!(*result.read().unwrap(), 984);
    ///
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(not(feature = "unchecked"))]
    #[inline(always)]
    pub fn on_progress(
        &mut self,
        callback: impl Fn(u64) -> Option<Dynamic> + SendSync + 'static,
    ) -> &mut Self {
        self.progress = Some(Box::new(callback));
        self
    }
    /// Override default action of `print` (print to stdout using [`println!`])
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() -> Result<(), Box<rhai::EvalAltResult>> {
    /// # use std::sync::RwLock;
    /// # use std::sync::Arc;
    /// use quad_compat_rhai::Engine;
    ///
    /// let result = Arc::new(RwLock::new(String::new()));
    ///
    /// let mut engine = Engine::new();
    ///
    /// // Override action of 'print' function
    /// let logger = result.clone();
    /// engine.on_print(move |s| logger.write().unwrap().push_str(s));
    ///
    /// engine.run("print(40 + 2);")?;
    ///
    /// assert_eq!(*result.read().unwrap(), "42");
    /// # Ok(())
    /// # }
    /// ```
    #[inline(always)]
    pub fn on_print(&mut self, callback: impl Fn(&str) + SendSync + 'static) -> &mut Self {
        self.print = Some(Box::new(callback));
        self
    }
    /// Override default action of `debug` (print to stdout using [`println!`])
    ///
    /// # Callback Function Signature
    ///
    /// The callback function signature passed takes the following form:
    ///
    /// > `Fn(text: &str, source: Option<&str>, pos: Position)`
    ///
    /// where:
    /// * `text`: the text to display
    /// * `source`: current source, if any
    /// * [`pos`][`Position`]: location of the `debug` call
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() -> Result<(), Box<rhai::EvalAltResult>> {
    /// # use std::sync::RwLock;
    /// # use std::sync::Arc;
    /// use quad_compat_rhai::Engine;
    ///
    /// let result = Arc::new(RwLock::new(String::new()));
    ///
    /// let mut engine = Engine::new();
    ///
    /// // Override action of 'print' function
    /// let logger = result.clone();
    /// engine.on_debug(move |s, src, pos| logger.write().unwrap().push_str(
    ///                     &format!("{} @ {:?} > {}", src.unwrap_or("unknown"), pos, s)
    ///                ));
    ///
    /// let mut ast = engine.compile(r#"let x = "hello"; debug(x);"#)?;
    /// ast.set_source("world");
    /// engine.run_ast(&ast)?;
    ///
    /// #[cfg(not(feature = "no_position"))]
    /// assert_eq!(*result.read().unwrap(), r#"world @ 1:18 > "hello""#);
    /// #[cfg(feature = "no_position")]
    /// assert_eq!(*result.read().unwrap(), r#"world @ none > "hello""#);
    /// # Ok(())
    /// # }
    /// ```
    #[inline(always)]
    pub fn on_debug(
        &mut self,
        callback: impl Fn(&str, Option<&str>, Position) + SendSync + 'static,
    ) -> &mut Self {
        self.debug = Some(Box::new(callback));
        self
    }
}
