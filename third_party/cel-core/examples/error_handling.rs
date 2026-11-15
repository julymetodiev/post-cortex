//! Handling CEL evaluation errors.
//!
//! Run with: cargo run -p cel-core --example error_handling

use cel_core::{CelType, Env, MapActivation, Value};

fn main() {
    let env = Env::with_standard_library()
        .with_variable("x", CelType::Int)
        .with_variable("items", CelType::list(CelType::Int));

    let mut activation = MapActivation::new();

    // Division by zero returns an error value
    println!("=== Division by zero ===");
    activation.insert("x", 0);
    let ast = env.compile("10 / x").unwrap();
    let program = env.program(&ast).unwrap();
    let result = program.eval(&activation);

    match &result {
        Value::Error(err) => println!("Error: {}", err),
        other => println!("Result: {}", other),
    }

    // Index out of bounds
    println!("\n=== Index out of bounds ===");
    activation.insert("items", Value::list([1, 2, 3]));
    let ast = env.compile("items[10]").unwrap();
    let program = env.program(&ast).unwrap();
    let result = program.eval(&activation);

    match &result {
        Value::Error(err) => println!("Error: {}", err),
        other => println!("Result: {}", other),
    }

    // Key not found in map
    println!("\n=== Key not found ===");
    let env = Env::with_standard_library()
        .with_variable("config", CelType::map(CelType::String, CelType::String));
    let mut activation = MapActivation::new();
    activation.insert("config", Value::map([("host", "localhost")]));

    let ast = env.compile("config.missing_key").unwrap();
    let program = env.program(&ast).unwrap();
    let result = program.eval(&activation);

    match &result {
        Value::Error(err) => println!("Error: {}", err),
        other => println!("Result: {}", other),
    }

    // Use has() to safely check field existence
    println!("\n=== Safe field access with has() ===");
    let ast = env
        .compile("has(config.missing_key) ? config.missing_key : 'default'")
        .unwrap();
    let program = env.program(&ast).unwrap();
    let result = program.eval(&activation);
    println!("Result: {}", result);
}
