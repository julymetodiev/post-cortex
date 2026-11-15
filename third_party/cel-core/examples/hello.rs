//! Minimal CEL example.
//!
//! Run with: cargo run -p cel-core --example hello

use cel_core::{CelType, Env, MapActivation};

fn main() {
    let env = Env::with_standard_library().with_variable("name", CelType::String);

    let ast = env.compile(r#""Hello, " + name + "!""#).unwrap();
    let program = env.program(&ast).unwrap();

    let mut activation = MapActivation::new();
    activation.insert("name", "World");

    let result = program.eval(&activation);
    println!("{}", result);
}
