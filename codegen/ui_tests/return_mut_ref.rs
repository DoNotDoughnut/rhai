use quad_compat_rhai::plugin::*;

#[derive(Clone)]
struct Clonable {
    a: f32,
    b: u32,
    c: char,
    d: bool,
}

#[export_fn]
pub fn test_fn(input: &mut Clonable) -> &mut bool {
    &mut input.d
}

fn main() {
    let n = Clonable {
        a: 0.0,
        b: 10,
        c: 'a',
        d: true,
    };
    if test_fn(n) {
        println!("yes");
    } else {
        println!("no");
    }
}
