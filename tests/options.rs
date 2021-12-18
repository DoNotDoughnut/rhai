use quad_compat_rhai::{Engine, EvalAltResult};

#[test]
fn test_options_allow() -> Result<(), Box<EvalAltResult>> {
    let mut engine = Engine::new();

    engine.compile("let x = if y { z } else { w };")?;

    engine.set_allow_if_expression(false);

    assert!(engine.compile("let x = if y { z } else { w };").is_err());

    engine.compile("let x = { let z = 0; z + 1 };")?;

    engine.set_allow_statement_expression(false);

    assert!(engine.compile("let x = { let z = 0; z + 1 };").is_err());

    #[cfg(not(feature = "no_function"))]
    {
        engine.compile("let x = || 42;")?;

        engine.set_allow_anonymous_fn(false);

        assert!(engine.compile("let x = || 42;").is_err());
    }

    engine.compile("while x > y { foo(z); }")?;

    engine.set_allow_looping(false);

    assert!(engine.compile("while x > y { foo(z); }").is_err());

    Ok(())
}

#[test]
fn test_options_strict_var() -> Result<(), Box<EvalAltResult>> {
    let mut engine = Engine::new();

    engine.compile("let x = if y { z } else { w };")?;

    #[cfg(not(feature = "no_function"))]
    engine.compile("fn foo(x) { x + y }")?;

    #[cfg(not(feature = "no_module"))]
    engine.compile("print(h::y::z);")?;

    #[cfg(not(feature = "no_module"))]
    engine.compile("let x = h::y::foo();")?;

    #[cfg(not(feature = "no_function"))]
    #[cfg(not(feature = "no_module"))]
    engine.compile("fn foo() { h::y::foo() }")?;

    #[cfg(not(feature = "no_function"))]
    engine.compile("let f = |y| x * y;")?;

    engine.set_strict_variables(true);

    assert!(engine.compile("let x = if y { z } else { w };").is_err());
    engine.compile("let y = 42; let x = y;")?;

    #[cfg(not(feature = "no_function"))]
    assert!(engine.compile("fn foo(x) { x + y }").is_err());

    #[cfg(not(feature = "no_module"))]
    {
        assert!(engine.compile("print(h::y::z);").is_err());
        engine.compile(r#"import "hello" as h; print(h::y::z);"#)?;
        assert!(engine.compile("let x = h::y::foo();").is_err());
        engine.compile(r#"import "hello" as h; let x = h::y::foo();"#)?;
    }

    #[cfg(not(feature = "no_function"))]
    #[cfg(not(feature = "no_module"))]
    engine.compile("fn foo() { h::y::foo() }")?;

    #[cfg(not(feature = "no_function"))]
    {
        assert!(engine.compile("let f = |y| x * y;").is_err());
        #[cfg(not(feature = "no_closure"))]
        {
            engine.compile("let x = 42; let f = |y| x * y;")?;
            engine.compile("let x = 42; let f = |y| { || x + y };")?;
            assert!(engine.compile("fn foo() { |y| { || x + y } }").is_err());
        }
    }

    Ok(())
}
