//! Using CEL extension libraries for type checking.
//!
//! Note: Extension function evaluation is in progress. This example demonstrates
//! that expressions using extension functions type-check correctly.
//!
//! Run with: cargo run -p cel-core --example extensions

use cel_core::{CelType, Env};

fn main() {
    // Enable all extensions (strings, math, encoders, optionals)
    // Extensions currently provide type declarations for the checker
    let env = Env::with_standard_library()
        .with_all_extensions()
        .with_variable("values", CelType::list(CelType::Int))
        .with_variable("text", CelType::String);

    // Math extension functions type-check correctly
    let ast = env.compile("math.greatest(values)").unwrap();
    println!("math.greatest(values) type: {:?}", ast.result_type());

    let ast = env.compile("math.least(values)").unwrap();
    println!("math.least(values) type:    {:?}", ast.result_type());

    let ast = env.compile("math.abs(-42)").unwrap();
    println!("math.abs(-42) type:         {:?}", ast.result_type());

    // String extension functions type-check correctly
    let ast = env.compile("text.split(' ')").unwrap();
    println!("text.split(' ') type:       {:?}", ast.result_type());

    let ast = env.compile("['a', 'b'].join('-')").unwrap();
    println!("['a','b'].join('-') type:   {:?}", ast.result_type());

    // Note: Runtime evaluation of extension functions is in progress.
    // For now, use standard library functions that are fully implemented:
    println!("\n=== Standard library (fully implemented) ===");

    let ast = env.compile("size(values)").unwrap();
    println!("size(values) type: {:?}", ast.result_type());

    let ast = env.compile("text.contains('hello')").unwrap();
    println!("text.contains type: {:?}", ast.result_type());

    let ast = env.compile("text.startsWith('h')").unwrap();
    println!("text.startsWith type: {:?}", ast.result_type());
}
