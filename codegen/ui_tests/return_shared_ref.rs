use quad_compat_rhai::plugin::*;

#[derive(Clone)]
struct Clonable {
    a: f32,
    b: u32,
    c: char,
    d: bool,
}

#[export_fn]
pub fn test_fn(input: Clonable) -> &'static str {
    "yes"
}

fn main() {
    let n = Clonable {
        a: 0.0,
        b: 10,
        c: 'a',
        d: true,
    };
    println!("{}", test_fn(n));
}
