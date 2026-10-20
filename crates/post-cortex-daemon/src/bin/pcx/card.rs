// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! `pcx card` — show or edit the project / user card through the daemon's
//! REST API (`/api/cards`).

use std::path::Path;

use post_cortex_daemon::daemon::DaemonConfig;
use serde::Deserialize;

use super::cli::CardAction;
use super::setup::daemon_url;

/// One card version as the REST API returns it.
#[derive(Deserialize)]
struct CardBody {
    version: u64,
    text: String,
}

#[derive(Deserialize)]
struct CardsBody {
    user: Option<CardBody>,
    workspace: Option<CardBody>,
}

#[derive(Deserialize)]
struct UpdateBody {
    written: bool,
    card: Option<CardBody>,
}

/// Which card a command acts on.
enum Target {
    User,
    Workspace(String),
}

pub async fn handle_card_action(action: CardAction) -> Result<(), String> {
    match action {
        CardAction::Show {
            workspace,
            user,
            url,
        } => {
            let target = target(workspace, user)?;
            let config = DaemonConfig::load();
            let base = url.unwrap_or_else(|| daemon_url(&config));
            match fetch(&config, &base, &target).await? {
                Some(card) => {
                    println!("# {} card, version {}\n", target.label(), card.version);
                    println!("{}", card.text);
                }
                None => println!(
                    "No {} card yet (pcx card edit creates one).",
                    target.label()
                ),
            }
            Ok(())
        }
        CardAction::Edit {
            workspace,
            user,
            url,
        } => {
            let target = target(workspace, user)?;
            let config = DaemonConfig::load();
            let base = url.unwrap_or_else(|| daemon_url(&config));
            edit(&config, &base, &target).await
        }
    }
}

impl Target {
    fn label(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Workspace(_) => "project",
        }
    }

    fn put_path(&self) -> String {
        match self {
            Self::User => "/api/cards/user".to_string(),
            Self::Workspace(id) => format!("/api/cards/workspace/{id}"),
        }
    }
}

/// `--user`, `--workspace`, or the workspace in ./CLAUDE.md.
fn target(workspace: Option<String>, user: bool) -> Result<Target, String> {
    match (workspace, user) {
        (Some(_), true) => Err("give --workspace or --user, not both".to_string()),
        (None, true) => Ok(Target::User),
        (Some(id), false) => Ok(Target::Workspace(id)),
        (None, false) => workspace_from_claude_md(Path::new("CLAUDE.md"))
            .map(Target::Workspace)
            .ok_or_else(|| {
                "no --workspace given and no Workspace ID in ./CLAUDE.md (pcx setup writes it)"
                    .to_string()
            }),
    }
}

/// The UUID on the `Workspace ID` line of a CLAUDE.md written by `pcx setup`.
fn workspace_from_claude_md(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let uuid = regex::Regex::new(r"[0-9a-fA-F]{8}-(?:[0-9a-fA-F]{4}-){3}[0-9a-fA-F]{12}").ok()?;
    text.lines()
        .filter(|line| line.contains("Workspace ID"))
        .find_map(|line| uuid.find(line).map(|m| m.as_str().to_string()))
}

async fn fetch(
    config: &DaemonConfig,
    base: &str,
    target: &Target,
) -> Result<Option<CardBody>, String> {
    let url = match target {
        Target::User => format!("{base}/api/cards"),
        Target::Workspace(id) => format!("{base}/api/cards?workspace_id={id}"),
    };
    let response = config
        .http_client()
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Cannot reach the daemon at {base}: {e}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "GET {url}: {} {}",
            response.status(),
            response.text().await.unwrap_or_default()
        ));
    }
    let cards: CardsBody = response
        .json()
        .await
        .map_err(|e| format!("Unexpected answer from {url}: {e}"))?;
    Ok(match target {
        Target::User => cards.user,
        Target::Workspace(_) => cards.workspace,
    })
}

/// Open the card in `$EDITOR` (default `vi`) and store the result as the
/// next version. If the card changed meanwhile, nothing is overwritten: the
/// edited text stays in a file to merge from.
async fn edit(config: &DaemonConfig, base: &str, target: &Target) -> Result<(), String> {
    let current = fetch(config, base, target).await?;
    let (version, text) = current.map_or((0, String::new()), |c| (c.version, c.text));
    let file = std::env::temp_dir().join(format!(
        "pcx-{}-card-{}.md",
        target.label(),
        std::process::id()
    ));
    std::fs::write(&file, &text).map_err(|e| format!("Cannot write {}: {e}", file.display()))?;

    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string());
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\""))
        .arg("sh")
        .arg(&file)
        .status()
        .map_err(|e| format!("Cannot start {editor}: {e}"))?;
    if !status.success() {
        return Err(format!(
            "{editor} exited with {status}; the card was not changed"
        ));
    }
    let edited = std::fs::read_to_string(&file)
        .map_err(|e| format!("Cannot read {}: {e}", file.display()))?;
    if edited.trim_end() == text.trim_end() {
        let _ = std::fs::remove_file(&file);
        println!("No change.");
        return Ok(());
    }

    let url = format!("{base}{}", target.put_path());
    let response = config
        .http_client()
        .put(&url)
        .json(&serde_json::json!({"content": edited, "expected_version": version}))
        .send()
        .await
        .map_err(|e| format!("Cannot reach the daemon at {base}: {e}"))?;
    let status = response.status();
    if !(status.is_success() || status == reqwest::StatusCode::CONFLICT) {
        return Err(format!(
            "PUT {url}: {status} {} (your text is in {})",
            response.text().await.unwrap_or_default(),
            file.display()
        ));
    }
    let body: UpdateBody = response
        .json()
        .await
        .map_err(|e| format!("Unexpected answer from {url}: {e}"))?;
    match (body.written, body.card) {
        (true, Some(card)) => {
            let _ = std::fs::remove_file(&file);
            println!(
                "Saved the {} card as version {}.",
                target.label(),
                card.version
            );
            Ok(())
        }
        (_, latest) => Err(format!(
            "The {} card changed while you edited it (now version {}); nothing was saved. \
             Your text is in {}; run pcx card edit again and merge it.",
            target.label(),
            latest.map_or(0, |c| c.version),
            file.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::workspace_from_claude_md;

    #[test]
    fn workspace_id_comes_from_claude_md() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("CLAUDE.md");
        std::fs::write(
            &path,
            "| Session ID | `11111111-2222-3333-4444-555555555555` |\n\
             | **Workspace ID** | `ba76f569-a50d-46cd-a5dd-7c5577f12d1f` |\n",
        )
        .unwrap();
        assert_eq!(
            workspace_from_claude_md(&path).as_deref(),
            Some("ba76f569-a50d-46cd-a5dd-7c5577f12d1f")
        );
        assert_eq!(workspace_from_claude_md(&dir.path().join("none.md")), None);
    }
}
