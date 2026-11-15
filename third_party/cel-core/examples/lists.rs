//! List operations in CEL.
//!
//! Run with: cargo run -p cel-core --example lists

use cel_core::{CelType, Env, MapActivation, Value};

fn main() {
    let env = Env::with_standard_library().with_variable("numbers", CelType::list(CelType::Int));

    let mut activation = MapActivation::new();
    activation.insert("numbers", Value::list([1, 5, 3, 8, 2]));

    // Filter: keep only values > 3
    let ast = env.compile("numbers.filter(x, x > 3)").unwrap();
    let program = env.program(&ast).unwrap();
    let result = program.eval(&activation);
    println!("filter(x, x > 3): {}", result);

    // Map: double each value
    let ast = env.compile("numbers.map(x, x * 2)").unwrap();
    let program = env.program(&ast).unwrap();
    let result = program.eval(&activation);
    println!("map(x, x * 2):    {}", result);

    // Exists: any value > 7?
    let ast = env.compile("numbers.exists(x, x > 7)").unwrap();
    let program = env.program(&ast).unwrap();
    let result = program.eval(&activation);
    println!("exists(x, x > 7): {}", result);

    // All: all values > 0?
    let ast = env.compile("numbers.all(x, x > 0)").unwrap();
    let program = env.program(&ast).unwrap();
    let result = program.eval(&activation);
    println!("all(x, x > 0):    {}", result);

    // Size
    let ast = env.compile("numbers.size()").unwrap();
    let program = env.program(&ast).unwrap();
    let result = program.eval(&activation);
    println!("size():           {}", result);

    // Contains (using 'in' operator)
    let ast = env.compile("5 in numbers").unwrap();
    let program = env.program(&ast).unwrap();
    let result = program.eval(&activation);
    println!("5 in numbers:     {}", result);
}
