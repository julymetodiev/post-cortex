// Copyright (c) 2025, 2026 Julius ML
// MIT License

use super::*;

fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
    Cli::try_parse_from(std::iter::once("pcx").chain(args.iter().copied()))
}

#[test]
fn setup_backend_flags() {
    let Some(Commands::Client(ClientCommand::Setup {
        profile,
        self_hosted,
        ..
    })) = parse(&["setup", "--profile", "work"]).unwrap().command
    else {
        panic!("setup")
    };
    assert_eq!(profile.as_deref(), Some("work"));
    assert!(!self_hosted);

    let Some(Commands::Client(ClientCommand::Setup {
        self_hosted,
        capture_url,
        ..
    })) = parse(&["setup", "--self-hosted", "--capture-url", "http://h:1"])
        .unwrap()
        .command
    else {
        panic!("setup")
    };
    assert!(self_hosted);
    assert_eq!(capture_url.as_deref(), Some("http://h:1"));

    assert!(parse(&["setup", "--self-hosted", "--profile", "cloud"]).is_err());
    assert!(parse(&["setup", "--profile", "cloud", "--capture-url", "http://h:1"]).is_err());
    assert!(parse(&["setup", "--profile", "cloud", "--daemon-url", "http://h:1"]).is_err());
}

#[test]
fn connect_command() {
    assert!(matches!(
        parse(&["connect"]).unwrap().command,
        Some(Commands::Client(ClientCommand::Connect {
            cloud: None,
            name: None,
            runner: false,
            off: false
        }))
    ));
    assert!(matches!(
        parse(&["connect", "--cloud", "https://x", "--name", "ops-ml"]).unwrap().command,
        Some(Commands::Client(ClientCommand::Connect { cloud: Some(c), name: Some(n), runner: false, off: false })) if c == "https://x" && n == "ops-ml"
    ));
    assert!(matches!(
        parse(&["connect", "--off"]).unwrap().command,
        Some(Commands::Client(ClientCommand::Connect { off: true, .. }))
    ));
    assert!(parse(&["connect", "--off", "--name", "x"]).is_err());
    assert!(matches!(
        parse(&["connect", "--runner"]).unwrap().command,
        Some(Commands::Client(ClientCommand::Connect {
            runner: true,
            ..
        }))
    ));
    assert!(parse(&["connect", "--off", "--runner"]).is_err());
}

#[test]
fn profile_login_and_mcp_commands() {
    assert!(matches!(
        parse(&["mcp", "--profile", "cloud", "--author", "zed"]).unwrap().command,
        Some(Commands::Client(ClientCommand::Mcp { profile: Some(p), author: Some(a) })) if p == "cloud" && a == "zed"
    ));
    assert!(matches!(
        parse(&["login", "--token", "t"]).unwrap().command,
        Some(Commands::Client(ClientCommand::Login { profile: None, token: Some(t) })) if t == "t"
    ));
    assert!(matches!(
        parse(&["profile", "add", "work", "--url", "https://x"]).unwrap().command,
        Some(Commands::Client(ClientCommand::Profile { action: ProfileAction::Add { name, url } })) if name == "work" && url == "https://x"
    ));
    assert!(
        parse(&[
            "mcp-config",
            "zed",
            "--profile",
            "cloud",
            "--url",
            "http://h:1"
        ])
        .is_err()
    );
    // Hooks from older setups pass only --url.
    assert!(matches!(
        parse(&[
            "hook",
            "cursor",
            "stop",
            "--session",
            "S",
            "--workspace",
            "W",
            "--url",
            "http://h:1"
        ])
        .unwrap()
        .command,
        Some(Commands::Client(ClientCommand::Hook {
            profile: None,
            url: Some(_),
            ..
        }))
    ));
    // Global hooks pass no ids; one id alone is refused.
    assert!(matches!(
        parse(&["hook", "codex", "session-start", "--profile", "cloud"])
            .unwrap()
            .command,
        Some(Commands::Client(ClientCommand::Hook {
            session: None,
            workspace: None,
            ..
        }))
    ));
    assert!(parse(&["hook", "codex", "stop", "--session", "S"]).is_err());
    assert!(matches!(
        parse(&["setup", "--project"]).unwrap().command,
        Some(Commands::Client(ClientCommand::Setup { project: true, .. }))
    ));
    assert!(parse(&[]).unwrap().command.is_none());
}
