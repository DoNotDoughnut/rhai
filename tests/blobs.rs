#![cfg(not(feature = "no_index"))]
use quad_compat_rhai::{Blob, Engine, EvalAltResult, Scope, INT};

#[test]
fn test_blobs() -> Result<(), Box<EvalAltResult>> {
    let mut a = Blob::new();
    a.push(1);
    a.push(2);
    a.push(3);

    let engine = Engine::new();
    let mut orig_scope = Scope::new();
    orig_scope.push("x", a);

    let mut scope = orig_scope.clone();

    assert_eq!(engine.eval_with_scope::<INT>(&mut scope, "x[1]")?, 2);
    assert_eq!(engine.eval_with_scope::<INT>(&mut scope, "x[0]")?, 1);
    assert_eq!(engine.eval_with_scope::<INT>(&mut scope, "x[-1]")?, 3);
    assert_eq!(engine.eval_with_scope::<INT>(&mut scope, "x[-3]")?, 1);
    assert_eq!(
        engine.eval_with_scope::<INT>(&mut scope, "x += 4; x[3]")?,
        4
    );

    #[cfg(not(feature = "no_object"))]
    {
        assert_eq!(
            engine.eval_with_scope::<Blob>(&mut orig_scope.clone(), "x.push(4); x")?,
            [1, 2, 3, 4]
        );
        assert_eq!(
            engine.eval_with_scope::<Blob>(&mut orig_scope.clone(), "x.insert(0, 4); x")?,
            [4, 1, 2, 3]
        );
        assert_eq!(
            engine.eval_with_scope::<Blob>(&mut orig_scope.clone(), "x.insert(999, 4); x")?,
            [1, 2, 3, 4]
        );
        assert_eq!(
            engine.eval_with_scope::<Blob>(&mut orig_scope.clone(), "x.insert(-2, 4); x")?,
            [1, 4, 2, 3]
        );
        assert_eq!(
            engine.eval_with_scope::<Blob>(&mut orig_scope.clone(), "x.insert(-999, 4); x")?,
            [4, 1, 2, 3]
        );
        assert_eq!(
            engine.eval_with_scope::<INT>(&mut orig_scope.clone(), "let z = [42]; x[z.len]")?,
            2
        );
        assert_eq!(
            engine.eval_with_scope::<INT>(&mut orig_scope.clone(), "let z = [2]; x[z[0]]")?,
            3
        );
    }

    assert_eq!(
        engine.eval_with_scope::<Blob>(&mut orig_scope.clone(), "x += x; x")?,
        [1, 2, 3, 1, 2, 3]
    );
    assert_eq!(
        engine.eval_with_scope::<Blob>(&mut orig_scope.clone(), "x + x")?,
        [1, 2, 3, 1, 2, 3]
    );
    assert_eq!(
        engine.eval_with_scope::<Blob>(&mut orig_scope.clone(), "x += 999; x")?,
        [1, 2, 3, 0xe7]
    );
    assert_eq!(
        engine.eval_with_scope::<Blob>(&mut orig_scope.clone(), "x[2] = 999; x")?,
        [1, 2, 0xe7]
    );

    Ok(())
}

#[cfg(not(feature = "only_i32"))]
#[test]
fn test_blobs_parse() -> Result<(), Box<EvalAltResult>> {
    let engine = Engine::new();

    assert_eq!(
        engine
            .eval::<INT>("let x = blob(16, 0); for n in 0..16 { x[n] = n; } parse_le_int(x,2,0)")?,
        0
    );

    assert_eq!(
        engine
            .eval::<INT>("let x = blob(16, 0); for n in 0..16 { x[n] = n; } parse_le_int(x,2,9)")?,
        0x0908070605040302
    );

    assert_eq!(
        engine.eval::<INT>(
            "let x = blob(16, 0); for n in 0..16 { x[n] = n; } parse_le_int(x,2..=11)"
        )?,
        0x0908070605040302
    );

    assert_eq!(
        engine.eval::<INT>(
            "let x = blob(16, 0); for n in 0..16 { x[n] = n; } parse_le_int(x,2..11)"
        )?,
        0x0908070605040302
    );

    assert_eq!(
        engine.eval::<INT>(
            "let x = blob(16, 0); for n in 0..16 { x[n] = n; } parse_be_int(x,2,10)"
        )?,
        0x0203040506070809
    );

    assert_eq!(
        engine.eval::<INT>(
            "let x = blob(16, 0); for n in 0..16 { x[n] = n; } parse_be_int(x,2..12)"
        )?,
        0x0203040506070809
    );

    assert_eq!(
        engine.eval::<INT>(
            "let x = blob(16, 0); for n in 0..16 { x[n] = n; } parse_le_int(x,-5,99)"
        )?,
        0x0f0e0d0c0b
    );

    assert_eq!(
        engine.eval::<INT>(
            "let x = blob(16, 0); for n in 0..16 { x[n] = n; } parse_le_int(x,-5,2)"
        )?,
        0x0c0b
    );

    assert_eq!(
        engine.eval::<INT>(
            "let x = blob(16, 0); for n in 0..16 { x[n] = n; } parse_le_int(x,-99,99)"
        )?,
        0x0706050403020100
    );

    assert_eq!(
        engine.eval::<INT>(
            "let x = blob(16, 0); for n in 0..16 { x[n] = n; } write_be(x, 3, 3, -98765432); parse_be_int(x, 3, 3)"
        )?,
        0xffffff0000000000_u64 as INT
    );

    assert_eq!(
        engine.eval::<INT>(
            "let x = blob(16, 0); for n in 0..16 { x[n] = n; } write_be(x, 3..=5, -98765432); parse_be_int(x, 3..6)"
        )?,
        0xffffff0000000000_u64 as INT
    );

    assert_eq!(
        engine.eval::<INT>(
            "let x = blob(16, 0); for n in 0..16 { x[n] = n; } write_le(x, 3, 3, -98765432); parse_le_int(x, 3, 3)"
        )?,
        0x1cf588
    );

    Ok(())
}

#[cfg(feature = "only_i32")]
#[test]
fn test_blobs_parse() -> Result<(), Box<EvalAltResult>> {
    let engine = Engine::new();

    assert_eq!(
        engine
            .eval::<INT>("let x = blob(16, 0); for n in 0..16 { x[n] = n; } parse_le_int(x,2,0)")?,
        0
    );

    assert_eq!(
        engine
            .eval::<INT>("let x = blob(16, 0); for n in 0..16 { x[n] = n; } parse_le_int(x,2,9)")?,
        0x05040302
    );

    assert_eq!(
        engine.eval::<INT>(
            "let x = blob(16, 0); for n in 0..16 { x[n] = n; } parse_be_int(x,2,10)"
        )?,
        0x02030405
    );

    assert_eq!(
        engine.eval::<INT>(
            "let x = blob(16, 0); for n in 0..16 { x[n] = n; } parse_le_int(x,-5,99)"
        )?,
        0x0e0d0c0b
    );

    assert_eq!(
        engine.eval::<INT>(
            "let x = blob(16, 0); for n in 0..16 { x[n] = n; } parse_le_int(x,-5,2)"
        )?,
        0x0c0b
    );

    assert_eq!(
        engine.eval::<INT>(
            "let x = blob(16, 0); for n in 0..16 { x[n] = n; } parse_le_int(x,-99,99)"
        )?,
        0x03020100
    );

    assert_eq!(
        engine.eval::<INT>(
            "let x = blob(16, 0); for n in 0..16 { x[n] = n; } write_be(x, 3, 3, -98765432); parse_be_int(x, 3, 3)"
        )?,
        0xfa1cf500_u32 as INT
    );

    assert_eq!(
        engine.eval::<INT>(
            "let x = blob(16, 0); for n in 0..16 { x[n] = n; } write_be(x, 3..=5, -98765432); parse_be_int(x, 3..6)"
        )?,
        0xfa1cf500_u32 as INT
    );

    assert_eq!(
        engine.eval::<INT>(
            "let x = blob(16, 0); for n in 0..16 { x[n] = n; } write_le(x, 3, 3, -98765432); parse_le_int(x, 3, 3)"
        )?,
        0x1cf588
    );

    Ok(())
}
