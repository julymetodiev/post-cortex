//! Extracting Rust types from CEL Values using TryFrom.
//!
//! Run with: cargo run -p cel-core --example extract_values

use cel_core::{CelType, Env, MapActivation, OptionalValue, Value, ValueMap};

fn main() {
    let env = Env::with_standard_library()
        .with_variable("count", CelType::Int)
        .with_variable("items", CelType::list(CelType::Int))
        .with_variable("config", CelType::map(CelType::String, CelType::String));

    let mut activation = MapActivation::new();
    activation.insert("count", 42); // i32 automatically widens to i64
    activation.insert("items", Value::list([1, 2, 3]));
    activation.insert(
        "config",
        Value::map([("host", "localhost"), ("port", "8080")]),
    );

    // Extract i64
    let ast = env.compile("count * 2").unwrap();
    let program = env.program(&ast).unwrap();
    let result = program.eval(&activation);

    let value: i64 = (&result).try_into().expect("expected int");
    println!("i64: {}", value);

    // Extract bool
    let ast = env.compile("count > 10").unwrap();
    let program = env.program(&ast).unwrap();
    let result = program.eval(&activation);

    let value: bool = (&result).try_into().expect("expected bool");
    println!("bool: {}", value);

    // Extract &str
    let ast = env.compile("config.host").unwrap();
    let program = env.program(&ast).unwrap();
    let result = program.eval(&activation);

    let value: &str = (&result).try_into().expect("expected string");
    println!("&str: {}", value);

    // Extract &[Value] (list)
    let ast = env.compile("items.filter(x, x > 1)").unwrap();
    let program = env.program(&ast).unwrap();
    let result = program.eval(&activation);

    let list: &[Value] = (&result).try_into().expect("expected list");
    println!("list length: {}", list.len());
    for (i, v) in list.iter().enumerate() {
        let n: i64 = v.try_into().expect("expected int");
        println!("  [{}] = {}", i, n);
    }

    // Extract &ValueMap
    let ast = env.compile("config").unwrap();
    let program = env.program(&ast).unwrap();
    let result = program.eval(&activation);

    let map: &ValueMap = (&result).try_into().expect("expected map");
    println!("map size: {}", map.len());
    for (k, v) in map.iter() {
        println!("  {:?} = {}", k, v);
    }

    // Handle errors gracefully
    let result = Value::from("not an int");
    let attempt: Result<i64, _> = (&result).try_into();
    match attempt {
        Ok(v) => println!("got: {}", v),
        Err(e) => println!("conversion error: {}", e),
    }

    // Extract &OptionalValue
    let opt_result = Value::optional_some(Value::from(99));
    let opt: &OptionalValue = (&opt_result).try_into().expect("expected optional");
    if opt.is_present() {
        let inner: i64 = opt.as_value().unwrap().try_into().expect("expected int");
        println!("optional value: {}", inner);
    }
}
