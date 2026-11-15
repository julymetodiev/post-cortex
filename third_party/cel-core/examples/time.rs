//! Working with timestamps and durations in CEL.
//!
//! Run with: cargo run -p cel-core --example time

use cel_core::{CelType, Duration, Env, MapActivation, Timestamp};

fn main() {
    println!("=== CEL Timestamp and Duration Examples ===\n");

    // Create an environment with the standard library
    let env = Env::with_standard_library()
        .with_variable("event_time", CelType::Timestamp)
        .with_variable("timeout", CelType::Duration)
        .with_variable("deadline", CelType::Timestamp);

    // -------------------------------------------------------------------------
    // 1. Creating timestamps and durations from strings
    // -------------------------------------------------------------------------
    println!("1. Parsing timestamps and durations from strings:");

    // Parse a timestamp from an RFC 3339 string
    let ast = env.compile("timestamp('2024-03-15T10:30:00Z')").unwrap();
    let program = env.program(&ast).unwrap();
    let result = program.eval(&MapActivation::new());
    println!("   timestamp('2024-03-15T10:30:00Z') = {}", result);

    // Parse a duration string
    let ast = env.compile("duration('1h30m')").unwrap();
    let program = env.program(&ast).unwrap();
    let result = program.eval(&MapActivation::new());
    println!("   duration('1h30m') = {}", result);

    // Various duration formats
    for expr in [
        "duration('30s')",
        "duration('500ms')",
        "duration('2h30m45s')",
        "duration('-1h')",
    ] {
        let ast = env.compile(expr).unwrap();
        let program = env.program(&ast).unwrap();
        let result = program.eval(&MapActivation::new());
        println!("   {} = {}", expr, result);
    }

    // -------------------------------------------------------------------------
    // 2. Timestamp arithmetic
    // -------------------------------------------------------------------------
    println!("\n2. Timestamp arithmetic:");

    let mut activation = MapActivation::new();
    activation.insert("event_time", Timestamp::new(1710498600, 0)); // 2024-03-15T10:30:00Z
    activation.insert("timeout", Duration::new(3600, 0)); // 1 hour

    // Add duration to timestamp
    let ast = env.compile("event_time + timeout").unwrap();
    let program = env.program(&ast).unwrap();
    let result = program.eval(&activation);
    println!("   event_time + timeout = {}", result);

    // Subtract duration from timestamp
    let ast = env.compile("event_time - duration('30m')").unwrap();
    let program = env.program(&ast).unwrap();
    let result = program.eval(&activation);
    println!("   event_time - duration('30m') = {}", result);

    // Difference between two timestamps yields a duration
    activation.insert("deadline", Timestamp::new(1710505800, 0)); // 2024-03-15T12:30:00Z
    let ast = env.compile("deadline - event_time").unwrap();
    let program = env.program(&ast).unwrap();
    let result = program.eval(&activation);
    println!("   deadline - event_time = {}", result);

    // -------------------------------------------------------------------------
    // 3. Timestamp comparisons
    // -------------------------------------------------------------------------
    println!("\n3. Timestamp comparisons:");

    let ast = env.compile("event_time < deadline").unwrap();
    let program = env.program(&ast).unwrap();
    let result = program.eval(&activation);
    println!("   event_time < deadline = {}", result);

    let ast = env
        .compile("event_time + duration('3h') > deadline")
        .unwrap();
    let program = env.program(&ast).unwrap();
    let result = program.eval(&activation);
    println!("   event_time + duration('3h') > deadline = {}", result);

    // -------------------------------------------------------------------------
    // 4. Timestamp accessors (UTC)
    // -------------------------------------------------------------------------
    println!("\n4. Timestamp accessors (UTC):");

    let accessors = [
        ("getFullYear", "event_time.getFullYear()"),
        ("getMonth (0-11)", "event_time.getMonth()"),
        ("getDate (1-31)", "event_time.getDate()"),
        ("getDayOfWeek (0=Sun)", "event_time.getDayOfWeek()"),
        ("getDayOfYear (0-365)", "event_time.getDayOfYear()"),
        ("getHours", "event_time.getHours()"),
        ("getMinutes", "event_time.getMinutes()"),
        ("getSeconds", "event_time.getSeconds()"),
    ];

    for (name, expr) in accessors {
        let ast = env.compile(expr).unwrap();
        let program = env.program(&ast).unwrap();
        let result = program.eval(&activation);
        println!("   {} = {}", name, result);
    }

    // -------------------------------------------------------------------------
    // 5. Timestamp accessors with timezone
    // -------------------------------------------------------------------------
    println!("\n5. Timestamp accessors with timezone:");

    // Same timestamp in different timezones
    let timezones = [
        ("UTC", "event_time.getHours()"),
        (
            "America/New_York",
            "event_time.getHours('America/New_York')",
        ),
        ("Europe/London", "event_time.getHours('Europe/London')"),
        ("Asia/Tokyo", "event_time.getHours('Asia/Tokyo')"),
        ("+05:30 (India)", "event_time.getHours('+05:30')"),
    ];

    for (tz_name, expr) in timezones {
        let ast = env.compile(expr).unwrap();
        let program = env.program(&ast).unwrap();
        let result = program.eval(&activation);
        println!("   Hours in {} = {}", tz_name, result);
    }

    // -------------------------------------------------------------------------
    // 6. Duration accessors
    // -------------------------------------------------------------------------
    println!("\n6. Duration accessors:");

    let mut activation = MapActivation::new();
    activation.insert("d", Duration::new(5400, 500_000_000)); // 1h30m + 500ms

    let accessors = [
        ("getHours (total)", "d.getHours()"),
        ("getMinutes (total)", "d.getMinutes()"),
        ("getSeconds (total)", "d.getSeconds()"),
        ("getMilliseconds", "d.getMilliseconds()"),
    ];

    for (name, expr) in accessors {
        let env = Env::with_standard_library().with_variable("d", CelType::Duration);
        let ast = env.compile(expr).unwrap();
        let program = env.program(&ast).unwrap();
        let result = program.eval(&activation);
        println!("   {} = {}", name, result);
    }

    // -------------------------------------------------------------------------
    // 7. Converting to string
    // -------------------------------------------------------------------------
    println!("\n7. Converting to string:");

    let mut activation = MapActivation::new();
    activation.insert("ts", Timestamp::new(1710498600, 123000000)); // with nanos
    activation.insert("d", Duration::new(5400, 500000000));

    let env = Env::with_standard_library()
        .with_variable("ts", CelType::Timestamp)
        .with_variable("d", CelType::Duration);

    let ast = env.compile("string(ts)").unwrap();
    let program = env.program(&ast).unwrap();
    let result = program.eval(&activation);
    println!("   string(ts) = {}", result);

    let ast = env.compile("string(d)").unwrap();
    let program = env.program(&ast).unwrap();
    let result = program.eval(&activation);
    println!("   string(d) = {}", result);

    // -------------------------------------------------------------------------
    // 8. Practical example: deadline checking
    // -------------------------------------------------------------------------
    println!("\n8. Practical example - deadline checking:");

    let env = Env::with_standard_library()
        .with_variable("request_time", CelType::Timestamp)
        .with_variable("max_response_time", CelType::Duration)
        .with_variable("current_time", CelType::Timestamp);

    let mut activation = MapActivation::new();
    activation.insert("request_time", Timestamp::new(1710498600, 0));
    activation.insert("max_response_time", Duration::new(30, 0)); // 30 seconds
    activation.insert("current_time", Timestamp::new(1710498620, 0)); // 20 seconds later

    let expr = "current_time < request_time + max_response_time";
    let ast = env.compile(expr).unwrap();
    let program = env.program(&ast).unwrap();
    let result = program.eval(&activation);
    println!(
        "   Is response within deadline? (current < request + 30s) = {}",
        result
    );

    // Simulate timeout
    activation.insert("current_time", Timestamp::new(1710498640, 0)); // 40 seconds later
    let result = program.eval(&activation);
    println!("   Is response within deadline? (40s later) = {}", result);

    // -------------------------------------------------------------------------
    // 9. Creating timestamps from integers (Unix epoch seconds)
    // -------------------------------------------------------------------------
    println!("\n9. Creating timestamps from integers:");

    let env = Env::with_standard_library();
    let ast = env.compile("timestamp(1710498600)").unwrap();
    let program = env.program(&ast).unwrap();
    let result = program.eval(&MapActivation::new());
    println!("   timestamp(1710498600) = {}", result);

    // Convert timestamp to epoch seconds
    let ast = env
        .compile("int(timestamp('2024-03-15T10:30:00Z'))")
        .unwrap();
    let program = env.program(&ast).unwrap();
    let result = program.eval(&MapActivation::new());
    println!("   int(timestamp('2024-03-15T10:30:00Z')) = {}", result);

    println!("\n=== Done ===");
}
