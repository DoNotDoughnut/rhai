#[cfg(any(not(feature = "serde"), feature = "no_object"))]
fn main() {
    println!("This example requires the 'serde' feature to run.");
    println!("Try: cargo run --features serde --example serde");
}

#[cfg(feature = "serde")]
#[cfg(not(feature = "no_object"))]
fn main() {
    example::ser();
    println!();
    example::de();
}

#[cfg(feature = "serde")]
#[cfg(not(feature = "no_object"))]
mod example {
    use quad_compat_rhai::serde::{from_dynamic, to_dynamic};
    use quad_compat_rhai::{Dynamic, Engine, Map};
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Point {
        x: f64,
        y: f64,
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct MyStruct {
        a: i64,
        b: Vec<String>,
        c: bool,
        d: Point,
    }

    pub fn ser() {
        let x = MyStruct {
            a: 42,
            b: vec!["hello".into(), "world".into()],
            c: true,
            d: Point {
                x: 123.456,
                y: 999.0,
            },
        };

        println!("Source struct: {:#?}", x);

        // Convert the 'MyStruct' into a 'Dynamic'
        let map: Dynamic = to_dynamic(x).unwrap();

        assert!(map.is::<Map>());
        println!("Serialized to Dynamic: {:#?}", map);
    }

    pub fn de() {
        let engine = Engine::new();
        let result: Dynamic = engine
            .eval(
                r#"
                    #{
                        a: 42,
                        b: [ "hello", "world" ],
                        c: true,
                        d: #{ x: 123.456, y: 999.0 }
                    }
                "#,
            )
            .unwrap();

        println!("Source Dynamic: {:#?}", result);

        // Convert the 'Dynamic' object map into 'MyStruct'
        let x: MyStruct = from_dynamic(&result).unwrap();

        assert_eq!(
            x,
            MyStruct {
                a: 42,
                b: vec!["hello".into(), "world".into()],
                c: true,
                d: Point {
                    x: 123.456,
                    y: 999.0,
                },
            }
        );
        println!("Deserialized to struct: {:#?}", x);
    }
}
