// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! `pcx login`, `pcx logout` and `pcx profile`.

use crate::profiles::{CLOUD, KEYS_URL, Store, clean};

/// `pcx login`: ask for (or take) the key of `profile` (default: the active
/// one), check it against the backend and store it.
pub async fn login(
    store: &Store,
    profile: Option<String>,
    token: Option<String>,
) -> Result<(), String> {
    let name = match profile {
        Some(name) => name,
        None => store.active()?,
    };
    login_to(store, &name, token).await
}

/// [`login`] for a named profile of `store` (also run by `pcx setup` for a
/// remote profile without a key).
pub async fn login_to(store: &Store, name: &str, token: Option<String>) -> Result<(), String> {
    let url = store.resolve(Some(name))?.url;
    let token = match clean(token) {
        Some(token) => token,
        None => ask_token(name, &url)?,
    };
    check_token(&url, &token).await?;
    store.store_token(name, &token)?;
    println!(
        "Logged in to '{name}' ({url}); key stored in {}",
        store.credentials_display()
    );
    Ok(())
}

/// The key, asked for without echo on a terminal, else read as one line
/// from stdin.
fn ask_token(name: &str, url: &str) -> Result<String, String> {
    use std::io::IsTerminal;
    if name == CLOUD {
        eprintln!("Create an API key at {KEYS_URL}");
    } else {
        eprintln!("The key of '{name}' ({url}): the daemon's auth_token");
    }
    let token = if std::io::stdin().is_terminal() {
        dialoguer::Password::new()
            .with_prompt("API key")
            .interact()
            .map_err(|e| format!("Input error: {e}"))?
    } else {
        eprintln!("Reading the key from stdin (one line)");
        let mut line = String::new();
        std::io::stdin()
            .read_line(&mut line)
            .map_err(|e| format!("Failed to read stdin: {e}"))?;
        line
    };
    clean(Some(token)).ok_or_else(|| "No key given".to_string())
}

/// Check `token` with an authenticated call that reads almost nothing
/// (`GET /api/cards`, the user card).
async fn check_token(url: &str, token: &str) -> Result<(), String> {
    let response = crate::local::bearer_client(Some(token), None, None)
        .map_err(|e| e.to_string())?
        .get(format!("{url}/api/cards"))
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("Cannot reach {url}: {e}"))?;
    match response.status() {
        s if s.is_success() => Ok(()),
        s if s == reqwest::StatusCode::UNAUTHORIZED || s == reqwest::StatusCode::FORBIDDEN => {
            Err(format!("{url} refused the key (HTTP {s}); nothing stored"))
        }
        s => Err(format!("{url} answered HTTP {s}; nothing stored")),
    }
}

/// `pcx logout`: forget the stored key of `profile` (default: the active
/// one).
pub fn logout(store: &Store, profile: Option<String>) -> Result<(), String> {
    let name = match profile {
        Some(name) => name,
        None => store.active()?,
    };
    if store.forget_token(&name)? {
        println!("Logged out of '{name}'");
    } else {
        println!("No key stored for '{name}'");
    }
    if std::env::var("PC_AUTH_TOKEN").is_ok_and(|t| !t.trim().is_empty()) {
        println!("PC_AUTH_TOKEN is set and is still used");
    }
    Ok(())
}

/// `pcx profile ...`.
pub fn handle_profile_action(
    store: &Store,
    action: crate::cli::ProfileAction,
) -> Result<(), String> {
    use crate::cli::ProfileAction;
    match action {
        ProfileAction::List => {
            for line in store.list_lines()? {
                println!("{line}");
            }
        }
        ProfileAction::Use { name } => {
            store.use_profile(&name)?;
            println!("Active profile: {name}");
        }
        ProfileAction::Add { name, url } => {
            store.add(&name, &url)?;
            println!("Profile '{name}': {}", store.resolve(Some(&name))?.url);
        }
        ProfileAction::Remove { name } => {
            store.remove(&name)?;
            println!("Removed profile '{name}'");
        }
    }
    Ok(())
}
