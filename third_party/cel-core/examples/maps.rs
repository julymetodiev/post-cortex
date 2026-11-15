//! Map operations in CEL.
//!
//! Run with: cargo run -p cel-core --example maps

use cel_core::{CelType, Env, MapActivation, Value};

fn main() {
    let env = Env::with_standard_library()
        .with_variable("user", CelType::map(CelType::String, CelType::Dyn));

    // Mixed value types require explicit Value::from()
    let user = Value::map([
        ("name", Value::from("Alice")),
        ("age", Value::from(30)), // i32 automatically widens to i64
        ("active", Value::from(true)),
    ]);

    let mut activation = MapActivation::new();
    activation.insert("user", user);

    // Field access
    let ast = env.compile("user.name").unwrap();
    let program = env.program(&ast).unwrap();
    let result = program.eval(&activation);
    println!("user.name:     {}", result);

    // Index access
    let ast = env.compile(r#"user["age"]"#).unwrap();
    let program = env.program(&ast).unwrap();
    let result = program.eval(&activation);
    println!(r#"user["age"]:   {}"#, result);

    // Check field existence with 'in'
    let ast = env.compile(r#""name" in user"#).unwrap();
    let program = env.program(&ast).unwrap();
    let result = program.eval(&activation);
    println!(r#""name" in user: {}"#, result);

    // Check field existence with has()
    let ast = env.compile("has(user.email)").unwrap();
    let program = env.program(&ast).unwrap();
    let result = program.eval(&activation);
    println!("has(user.email): {}", result);

    // Combine conditions
    let ast = env.compile(r#"user.active && user.age >= 21"#).unwrap();
    let program = env.program(&ast).unwrap();
    let result = program.eval(&activation);
    println!("active && age >= 21: {}", result);
}
