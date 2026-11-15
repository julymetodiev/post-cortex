//! Complete CEL workflow: parse, type-check, and evaluate.
//!
//! Run with: cargo run -p cel-core --example quickstart

use cel_core::{CelType, Env, MapActivation, Value};

fn main() {
    // 1. Create an environment with the standard library and declare variables
    let env = Env::with_standard_library()
        .with_variable("user", CelType::String)
        .with_variable("age", CelType::Int);

    // 2. Compile the expression (parse + type-check)
    let ast = env
        .compile("age >= 21 && user.startsWith('admin')")
        .unwrap();

    // 3. Create a program from the compiled AST
    let program = env.program(&ast).unwrap();

    // 4. Set up variable bindings for evaluation
    let mut activation = MapActivation::new();
    activation.insert("user", "admin_alice"); // &str converts automatically
    activation.insert("age", 25); // integers widen automatically

    // 5. Evaluate the expression
    let result = program.eval(&activation);
    assert_eq!(result, Value::Bool(true));

    println!("Expression: age >= 21 && user.startsWith('admin')");
    println!("Result: {}", result);
}
