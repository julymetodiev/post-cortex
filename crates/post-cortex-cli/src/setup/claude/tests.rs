// Copyright (c) 2025, 2026 Julius ML
// MIT License

use super::*;

/// The bundled skill must not drift from `skills/post-cortex-memory/`.
/// Skipped when the repository copy is absent (e.g. building from the
/// crates.io package).
#[test]
fn bundled_skill_matches_repository_copy() {
    let repo_skill = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/post-cortex-memory");
    if !repo_skill.exists() {
        return;
    }
    for (rel_path, bundled) in SKILL_FILES {
        let canonical = std::fs::read_to_string(repo_skill.join(rel_path))
            .unwrap_or_else(|e| panic!("read skills/post-cortex-memory/{rel_path}: {e}"));
        assert_eq!(
            *bundled, canonical,
            "templates/skills/post-cortex-memory/{rel_path} differs from skills/post-cortex-memory/{rel_path}; copy it over"
        );
    }
}

#[test]
fn claude_code_setup_points_at_the_plugin() {
    let profile = |name: &str, url: &str| Profile {
        name: name.into(),
        url: url.into(),
        token: None,
    };
    let cloud = claude_plugin_lines(&profile("cloud", profiles::CLOUD_URL)).join("\n");
    assert!(cloud.contains("/plugin marketplace add julymetodiev/post-cortex"));
    assert!(cloud.contains("/plugin install post-cortex@post-cortex"));
    assert!(cloud.contains("keep the url"));
    let local = claude_plugin_lines(&profile("self", "http://127.0.0.1:3737")).join("\n");
    assert!(local.contains("url = http://127.0.0.1:3737"), "{local}");
    assert!(local.contains("auth_token"), "{local}");
}

/// The section as setups before the plugin wrote it.
const SECTION: &str = "## Session\n\n**Session ID**  `S`\n**Workspace ID** | `W`\n\n## Rules\n\n1. Search\n\n### Interaction Types\n\n`qa`\n\n## MCP Tools\n\n| Tool | When |\n|---|---|\n\n## Hooks\n\n| Event | Purpose |\n|---|---|\n| `Stop` | check |\n";

#[test]
fn the_pcx_section_goes_and_the_persons_text_stays() {
    // Appended after the person's own sections.
    let after = format!("# CLAUDE.md\n\n## Law\n\nKeep it.\n\n{SECTION}");
    assert_eq!(
        without_pcx_section(&after).unwrap(),
        "# CLAUDE.md\n\n## Law\n\nKeep it.\n"
    );
    // Put after the first heading, with the `---` setup added, before them.
    let before = format!("# CLAUDE.md\n{SECTION}\n---\n\n## Project\n\nIts text.\n");
    assert_eq!(
        without_pcx_section(&before).unwrap(),
        "# CLAUDE.md\n\n## Project\n\nIts text.\n"
    );
    // A file of the section only is left with its title.
    let only = format!("# CLAUDE.md\n\n{SECTION}");
    assert_eq!(without_pcx_section(&only).unwrap(), "# CLAUDE.md\n");
    // A `## Session` that is no PCX section is the person's.
    assert_eq!(
        without_pcx_section("# CLAUDE.md\n\n## Session\n\nHow we pair.\n"),
        None
    );
}

#[test]
fn retiring_a_project_takes_out_only_what_post_cortex_wrote() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("CLAUDE.md"), format!("# CLAUDE.md\n\n{SECTION}")).unwrap();
    std::fs::create_dir_all(root.join(".claude/hooks")).unwrap();
    for name in [
        "pcx-stop-check.py",
        "pcx-compact-reinject.sh",
        "my-guard.py",
    ] {
        std::fs::write(root.join(".claude/hooks").join(name), "x").unwrap();
    }
    let settings = serde_json::json!({
        "permissions": {"allow": ["Bash(ls)"]},
        "hooks": {
            "Stop": [{"hooks": [
                {"type": "command", "command": "python3 -I \"$CLAUDE_PROJECT_DIR\"/.claude/hooks/pcx-stop-check.py S"},
                {"type": "command", "command": "my-own-hook"}
            ]}],
            "UserPromptSubmit": [{"hooks": [{"type": "command", "command": "echo '[PCX] Session: S'"}]}],
            "SessionStart": [{"matcher": "compact", "hooks": [
                {"type": "command", "command": "\"$CLAUDE_PROJECT_DIR\"/.claude/hooks/pcx-compact-reinject.sh"}
            ]}],
            "PreToolUse": [{"matcher": "Bash", "hooks": [
                {"type": "command", "command": "python3 -I .claude/hooks/my-guard.py"}
            ]}]
        }
    });
    std::fs::write(
        root.join(".claude/settings.json"),
        serde_json::to_string_pretty(&settings).unwrap(),
    )
    .unwrap();

    let said = retire_project(root).unwrap().join("\n");
    assert!(said.contains("Removed: CLAUDE.md"), "{said}");
    assert!(!root.join("CLAUDE.md").exists());
    assert!(!root.join(".claude/hooks/pcx-stop-check.py").exists());
    assert!(!root.join(".claude/hooks/pcx-compact-reinject.sh").exists());
    assert!(root.join(".claude/hooks/my-guard.py").exists());
    let kept: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root.join(".claude/settings.json")).unwrap())
            .unwrap();
    assert_eq!(
        kept,
        serde_json::json!({
            "permissions": {"allow": ["Bash(ls)"]},
            "hooks": {
                "Stop": [{"hooks": [{"type": "command", "command": "my-own-hook"}]}],
                "PreToolUse": [{"matcher": "Bash", "hooks": [
                    {"type": "command", "command": "python3 -I .claude/hooks/my-guard.py"}
                ]}]
            }
        })
    );
    // Nothing more to do the second time.
    assert!(retire_project(root).unwrap().is_empty());
}

#[test]
fn hooks_left_empty_go_with_their_key() {
    let mut settings = serde_json::json!({"hooks": {"Stop": [{"hooks": [
        {"type": "command", "command": "python3 -I x/pcx-card.py W"}
    ]}]}});
    assert!(remove_pcx_hooks(&mut settings).unwrap());
    assert_eq!(settings, serde_json::json!({}));
    assert!(!remove_pcx_hooks(&mut settings).unwrap());
}
