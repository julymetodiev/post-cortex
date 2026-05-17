// Copyright (c) 2025 Julius ML
// MIT License

//! Daemon lifecycle commands: status, stop, init-config.

use post_cortex_daemon::daemon::DaemonConfig;
use tokio::net::TcpStream;
use tokio::time::{Duration, timeout};

pub async fn check_status(config: &DaemonConfig) -> Result<(), String> {
    println!(
        "Checking daemon status at {}:{}...",
        config.host, config.port
    );

    let addr = format!("{}:{}", config.host, config.port);

    match timeout(Duration::from_secs(2), TcpStream::connect(&addr)).await {
        Ok(Ok(_)) => {
            println!("Daemon is running");
            println!("  MCP endpoint: http://{}:{}/mcp", config.host, config.port);
            println!(
                "  Health endpoint: http://{}:{}/health",
                config.host, config.port
            );
            Ok(())
        }
        Ok(Err(e)) => {
            println!("Daemon is not running");
            println!("  Error: {}", e);
            Err("Daemon not running".to_string())
        }
        Err(_) => {
            println!("Daemon is not running");
            println!("  Error: Connection timeout");
            Err("Daemon not running".to_string())
        }
    }
}

pub fn stop_daemon(config: &DaemonConfig) -> Result<(), String> {
    println!("Stopping daemon at {}:{}...", config.host, config.port);
    println!();
    println!("To stop the daemon:");
    println!("1. Press Ctrl+C in the terminal where daemon is running");
    println!("2. Or use: kill $(lsof -t -i:{})", config.port);
    println!("3. Or use: pkill -f 'pcx start'");
    Ok(())
}

pub fn init_config() -> Result<(), String> {
    println!("Creating example config file...");

    match DaemonConfig::create_example_config() {
        Ok(path) => {
            println!("Config file created at: {:?}", path);
            println!();
            println!("To start the daemon:");
            println!("  pcx start");
            Ok(())
        }
        Err(e) => {
            eprintln!("Failed to create config file: {}", e);
            Err(e)
        }
    }
}
