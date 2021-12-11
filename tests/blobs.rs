#![cfg(not(feature = "no_index"))]
use rhai::{Blob, Engine, EvalAltResult, Scope, INT};

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

    Ok(())
}
