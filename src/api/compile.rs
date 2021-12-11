//! Module that defines the public compilation API of [`Engine`].

use crate::parser::ParseState;
use crate::{Engine, ParseError, Scope, AST};
#[cfg(feature = "no_std")]
use std::prelude::v1::*;

#[cfg(not(feature = "no_object"))]
use crate::Map;

impl Engine {
    /// Compile a string into an [`AST`], which can be used later for evaluation.
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
    /// for _ in 0..42 {
    ///     assert_eq!(engine.eval_ast::<i64>(&ast)?, 42);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    #[inline(always)]
    pub fn compile(&self, script: impl AsRef<str>) -> Result<AST, ParseError> {
        self.compile_with_scope(&Scope::new(), script)
    }
    /// Compile a string into an [`AST`] using own scope, which can be used later for evaluation.
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
    /// # #[cfg(not(feature = "no_optimize"))]
    /// # {
    /// use rhai::{Engine, Scope, OptimizationLevel};
    ///
    /// let mut engine = Engine::new();
    ///
    /// // Create initialized scope
    /// let mut scope = Scope::new();
    /// scope.push_constant("x", 42_i64);   // 'x' is a constant
    ///
    /// // Compile a script to an AST and store it for later evaluation.
    /// // Notice that `Full` optimization is on, so constants are folded
    /// // into function calls and operators.
    /// let ast = engine.compile_with_scope(&mut scope,
    ///             "if x > 40 { x } else { 0 }"    // all 'x' are replaced with 42
    /// )?;
    ///
    /// // Normally this would have failed because no scope is passed into the 'eval_ast'
    /// // call and so the variable 'x' does not exist.  Here, it passes because the script
    /// // has been optimized and all references to 'x' are already gone.
    /// assert_eq!(engine.eval_ast::<i64>(&ast)?, 42);
    /// # }
    /// # Ok(())
    /// # }
    /// ```
    #[inline(always)]
    pub fn compile_with_scope(
        &self,
        scope: &Scope,
        script: impl AsRef<str>,
    ) -> Result<AST, ParseError> {
        self.compile_scripts_with_scope(scope, &[script])
    }
    /// Compile a string into an [`AST`] using own scope, which can be used later for evaluation,
    /// embedding all imported modules.
    ///
    /// Not available under `no_module`.
    ///
    /// Modules referred by `import` statements containing literal string paths are eagerly resolved
    /// via the current [module resolver][crate::ModuleResolver] and embedded into the resultant
    /// [`AST`]. When it is evaluated later, `import` statement directly recall pre-resolved
    /// [modules][crate::Module] and the resolution process is not performed again.
    #[cfg(not(feature = "no_module"))]
    pub fn compile_into_self_contained(
        &self,
        scope: &Scope,
        script: impl AsRef<str>,
    ) -> Result<AST, Box<crate::EvalAltResult>> {
        use crate::{
            ast::{ASTNode, Expr, Stmt},
            func::native::shared_take_or_clone,
            module::resolvers::StaticModuleResolver,
        };
        use std::collections::BTreeSet;

        fn collect_imports(
            ast: &AST,
            resolver: &StaticModuleResolver,
            imports: &mut BTreeSet<crate::Identifier>,
        ) {
            ast.walk(
                &mut |path| match path.last().expect("contains current node") {
                    // Collect all `import` statements with a string constant path
                    ASTNode::Stmt(Stmt::Import(Expr::StringConstant(s, _), _, _))
                        if !resolver.contains_path(s) && !imports.contains(s.as_str()) =>
                    {
                        imports.insert(s.clone().into());
                        true
                    }
                    _ => true,
                },
            );
        }

        let mut ast = self.compile_scripts_with_scope(scope, &[script])?;

        if let Some(ref module_resolver) = self.module_resolver {
            let mut resolver = StaticModuleResolver::new();
            let mut imports = BTreeSet::new();

            collect_imports(&ast, &resolver, &mut imports);

            if !imports.is_empty() {
                while let Some(path) = imports.iter().next() {
                    let path = path.clone();

                    match module_resolver.resolve_ast(self, None, &path, crate::Position::NONE) {
                        Some(Ok(module_ast)) => {
                            collect_imports(&module_ast, &resolver, &mut imports)
                        }
                        Some(err) => return err,
                        None => (),
                    }

                    let module =
                        module_resolver.resolve(self, None, &path, crate::Position::NONE)?;
                    let module = shared_take_or_clone(module);

                    imports.remove(&path);
                    resolver.insert(path, module);
                }
                ast.set_resolver(resolver);
            }
        }

        Ok(ast)
    }
    /// When passed a list of strings, first join the strings into one large script, and then
    /// compile them into an [`AST`] using own scope, which can be used later for evaluation.
    ///
    /// The scope is useful for passing constants into the script for optimization when using
    /// [`OptimizationLevel::Full`][crate::OptimizationLevel::Full].
    ///
    /// ## Note
    ///
    /// All strings are simply parsed one after another with nothing inserted in between, not even a
    /// newline or space.
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
    /// # #[cfg(not(feature = "no_optimize"))]
    /// # {
    /// use rhai::{Engine, Scope, OptimizationLevel};
    ///
    /// let mut engine = Engine::new();
    ///
    /// // Create initialized scope
    /// let mut scope = Scope::new();
    /// scope.push_constant("x", 42_i64);   // 'x' is a constant
    ///
    /// // Compile a script made up of script segments to an AST and store it for later evaluation.
    /// // Notice that `Full` optimization is on, so constants are folded
    /// // into function calls and operators.
    /// let ast = engine.compile_scripts_with_scope(&mut scope, &[
    ///             "if x > 40",            // all 'x' are replaced with 42
    ///             "{ x } el",
    ///             "se { 0 }"              // segments do not need to be valid scripts!
    /// ])?;
    ///
    /// // Normally this would have failed because no scope is passed into the 'eval_ast'
    /// // call and so the variable 'x' does not exist.  Here, it passes because the script
    /// // has been optimized and all references to 'x' are already gone.
    /// assert_eq!(engine.eval_ast::<i64>(&ast)?, 42);
    /// # }
    /// # Ok(())
    /// # }
    /// ```
    #[inline(always)]
    pub fn compile_scripts_with_scope(
        &self,
        scope: &Scope,
        scripts: &[impl AsRef<str>],
    ) -> Result<AST, ParseError> {
        self.compile_with_scope_and_optimization_level(
            scope,
            scripts,
            #[cfg(not(feature = "no_optimize"))]
            self.optimization_level,
        )
    }
    /// Join a list of strings and compile into an [`AST`] using own scope at a specific optimization level.
    ///
    /// ## Constants Propagation
    ///
    /// If not [`OptimizationLevel::None`], constants defined within the scope are propagated
    /// throughout the script _including_ functions. This allows functions to be optimized based on
    /// dynamic global constants.
    #[inline]
    pub(crate) fn compile_with_scope_and_optimization_level(
        &self,
        scope: &Scope,
        scripts: &[impl AsRef<str>],
        #[cfg(not(feature = "no_optimize"))] optimization_level: crate::OptimizationLevel,
    ) -> Result<AST, ParseError> {
        let (stream, tokenizer_control) =
            self.lex_raw(scripts, self.token_mapper.as_ref().map(Box::as_ref));
        let mut state = ParseState::new(self, tokenizer_control);
        self.parse(
            &mut stream.peekable(),
            &mut state,
            scope,
            #[cfg(not(feature = "no_optimize"))]
            optimization_level,
        )
    }
    /// Compile a string containing an expression into an [`AST`],
    /// which can be used later for evaluation.
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
    /// let ast = engine.compile_expression("40 + 2")?;
    ///
    /// for _ in 0..42 {
    ///     assert_eq!(engine.eval_ast::<i64>(&ast)?, 42);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    #[inline(always)]
    pub fn compile_expression(&self, script: impl AsRef<str>) -> Result<AST, ParseError> {
        self.compile_expression_with_scope(&Scope::new(), script)
    }
    /// Compile a string containing an expression into an [`AST`] using own scope,
    /// which can be used later for evaluation.
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() -> Result<(), Box<rhai::EvalAltResult>> {
    /// # #[cfg(not(feature = "no_optimize"))]
    /// # {
    /// use rhai::{Engine, Scope, OptimizationLevel};
    ///
    /// let mut engine = Engine::new();
    ///
    /// // Create initialized scope
    /// let mut scope = Scope::new();
    /// scope.push_constant("x", 10_i64);   // 'x' is a constant
    ///
    /// // Compile a script to an AST and store it for later evaluation.
    /// // Notice that `Full` optimization is on, so constants are folded
    /// // into function calls and operators.
    /// let ast = engine.compile_expression_with_scope(&mut scope,
    ///             "2 + (x + x) * 2"    // all 'x' are replaced with 10
    /// )?;
    ///
    /// // Normally this would have failed because no scope is passed into the 'eval_ast'
    /// // call and so the variable 'x' does not exist.  Here, it passes because the script
    /// // has been optimized and all references to 'x' are already gone.
    /// assert_eq!(engine.eval_ast::<i64>(&ast)?, 42);
    /// # }
    /// # Ok(())
    /// # }
    /// ```
    #[inline]
    pub fn compile_expression_with_scope(
        &self,
        scope: &Scope,
        script: impl AsRef<str>,
    ) -> Result<AST, ParseError> {
        let scripts = [script];
        let (stream, tokenizer_control) =
            self.lex_raw(&scripts, self.token_mapper.as_ref().map(Box::as_ref));

        let mut peekable = stream.peekable();
        let mut state = ParseState::new(self, tokenizer_control);
        self.parse_global_expr(
            &mut peekable,
            &mut state,
            scope,
            #[cfg(not(feature = "no_optimize"))]
            self.optimization_level,
        )
    }
    /// Parse a JSON string into an [object map][`Map`].
    /// This is a light-weight alternative to using, say,
    /// [`serde_json`](https://crates.io/crates/serde_json) to deserialize the JSON.
    ///
    /// Not available under `no_object`.
    ///
    /// The JSON string must be an object hash.  It cannot be a simple scalar value.
    ///
    /// Set `has_null` to `true` in order to map `null` values to `()`.
    /// Setting it to `false` will cause an [`ErrorVariableNotFound`][crate::EvalAltResult::ErrorVariableNotFound] error during parsing.
    ///
    /// # JSON With Sub-Objects
    ///
    /// This method assumes no sub-objects in the JSON string.  That is because the syntax
    /// of a JSON sub-object (or object hash), `{ .. }`, is different from Rhai's syntax, `#{ .. }`.
    /// Parsing a JSON string with sub-objects will cause a syntax error.
    ///
    /// If it is certain that the character `{` never appears in any text string within the JSON object,
    /// which is a valid assumption for many use cases, then globally replace `{` with `#{` before calling this method.
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() -> Result<(), Box<rhai::EvalAltResult>> {
    /// use rhai::{Engine, Map};
    ///
    /// let engine = Engine::new();
    ///
    /// let map = engine.parse_json(
    ///     r#"{"a":123, "b":42, "c":{"x":false, "y":true}, "d":null}"#
    ///         .replace("{", "#{").as_str(),
    /// true)?;
    ///
    /// assert_eq!(map.len(), 4);
    /// assert_eq!(map["a"].as_int().expect("a should exist"), 123);
    /// assert_eq!(map["b"].as_int().expect("b should exist"), 42);
    /// assert!(map["d"].is::<()>());
    ///
    /// let c = map["c"].read_lock::<Map>().expect("c should exist");
    /// assert_eq!(c["x"].as_bool().expect("x should be bool"), false);
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(not(feature = "no_object"))]
    #[inline(always)]
    pub fn parse_json(
        &self,
        json: impl AsRef<str>,
        has_null: bool,
    ) -> Result<Map, Box<crate::EvalAltResult>> {
        use crate::tokenizer::Token;

        fn parse_json_inner(
            engine: &Engine,
            json: &str,
            has_null: bool,
        ) -> Result<Map, Box<crate::EvalAltResult>> {
            let mut scope = Scope::new();
            let json_text = json.trim_start();
            let scripts = if json_text.starts_with(Token::MapStart.literal_syntax()) {
                [json_text, ""]
            } else if json_text.starts_with(Token::LeftBrace.literal_syntax()) {
                ["#", json_text]
            } else {
                return Err(crate::ParseErrorType::MissingToken(
                    Token::LeftBrace.syntax().into(),
                    "to start a JSON object hash".into(),
                )
                .into_err(crate::Position::new(
                    1,
                    (json.len() - json_text.len() + 1) as u16,
                ))
                .into());
            };
            let (stream, tokenizer_control) = engine.lex_raw(
                &scripts,
                if has_null {
                    Some(&|token, _, _| {
                        match token {
                            // If `null` is present, make sure `null` is treated as a variable
                            Token::Reserved(s) if &*s == "null" => Token::Identifier(s),
                            _ => token,
                        }
                    })
                } else {
                    None
                },
            );
            let mut state = ParseState::new(engine, tokenizer_control);
            let ast = engine.parse_global_expr(
                &mut stream.peekable(),
                &mut state,
                &scope,
                #[cfg(not(feature = "no_optimize"))]
                crate::OptimizationLevel::None,
            )?;
            if has_null {
                scope.push_constant("null", ());
            }
            engine.eval_ast_with_scope(&mut scope, &ast)
        }

        parse_json_inner(self, json.as_ref(), has_null)
    }
}
