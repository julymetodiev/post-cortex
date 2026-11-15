// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A ticket worked by two members in a git repository, with Z.AI, only
//! when asked: the one assigned writes and edits files, commits them on a
//! branch, links the branch, describes the work, moves the task to review
//! and asks the reviewer; the reviewer reviews the branch's commits and,
//! approving, moves the task to testing. What they did is read from the
//! disk, git and the board, not from what they say.
//!
//! ```sh
//! PCX_ZAI_KEY_FILE=~/.axon/zai_auth.json cargo test -p post-cortex-daemon --lib live_member_works -- --ignored --nocapture
//! ```

use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pcx_host::{Grant, RootGrant};
use pcx_module_api::Scope;
use post_cortex_memory::capabilities::CreateWorkspace;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::compose::{Builtin, BuiltinSpec, Modules, host};

fn scope(author: &str) -> Scope {
    Scope {
        tenant: String::new(),
        profile: String::new(),
        session_id: None,
        workspace_id: None,
        author: Some(author.to_string()),
        caller_module: None,
        call_depth: 0,
        turn: None,
        reach: None,
        role: None,
        read_only: false,
        person: None,
        deadline: Instant::now() + Duration::from_secs(30),
        cancel: CancellationToken::new(),
    }
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn builtin(id: Builtin, grants: Vec<Grant>, root: &Path, config: Option<Value>) -> BuiltinSpec {
    BuiltinSpec {
        id,
        grants,
        filesystem: vec![RootGrant {
            path: root.to_path_buf(),
            write: true,
        }],
        config,
    }
}

#[tokio::test]
#[ignore = "calls Z.AI: set PCX_ZAI_KEY_FILE and pass --ignored"]
async fn live_member_works_a_ticket_in_a_repo() {
    let file = std::env::var("PCX_ZAI_KEY_FILE").expect("PCX_ZAI_KEY_FILE names the key file");
    let stored: Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
    let data = tempfile::tempdir().unwrap();
    let repo_dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let repo = repo_dir.path().canonicalize().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "not yours").unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.name", "julius"]);
    git(&repo, &["config", "user.email", "julius@example.com"]);
    std::fs::write(repo.join("README.md"), "# Demo\n\nA small project.\n").unwrap();
    git(&repo, &["add", "README.md"]);
    git(&repo, &["commit", "-q", "-m", "chore: start"]);

    let memory = Arc::new(
        crate::memory_tests::rocksdb_memory(
            &data,
            post_cortex_memory::SystemConfig {
                enable_embeddings: false,
                ..Default::default()
            },
        )
        .await
        .unwrap(),
    );
    let ws = memory
        .workspaces
        .create(CreateWorkspace {
            name: "demo".into(),
            description: String::new(),
        })
        .await
        .unwrap()
        .id
        .to_string();
    let author = json!({ "author_name": "dev", "author_email": "dev@example.com" });
    let builtins = [
        builtin(Builtin::ToolsFs, vec![Grant::Filesystem], &repo, None),
        builtin(
            Builtin::ToolsGit,
            vec![Grant::Filesystem, Grant::Spawn],
            &repo,
            Some(author),
        ),
        builtin(Builtin::Board, Vec::new(), &repo, None),
        builtin(
            Builtin::Skills,
            Vec::new(),
            &repo,
            Some(json!({ "workspace_id": ws })),
        ),
        builtin(
            Builtin::Models,
            vec![Grant::Network],
            &repo,
            Some(json!({ "zai": { "api_key": stored["api_key"] } })),
        ),
        builtin(Builtin::Context, Vec::new(), &repo, None),
        builtin(Builtin::Agent, Vec::new(), &repo, None),
        builtin(Builtin::Team, Vec::new(), &repo, None),
    ];
    let modules = Modules::memory(Arc::clone(&memory))
        .unwrap()
        .sandboxed(&data.path().join("scratch"), Vec::new(), None)
        .unwrap()
        .configured(&[], &[], &builtins)
        .await
        .unwrap();
    let host = Arc::new(host(&modules).await.unwrap());
    let stop = CancellationToken::new();
    let feed = tokio::spawn(crate::compose::feed_memory_writes(
        Arc::clone(&host),
        Arc::clone(&memory),
        stop.clone(),
    ));
    let call = |tool: &'static str, input: Value| {
        let host = Arc::clone(&host);
        async move {
            host.call_for_agent(&scope("julius"), tool, input)
                .await
                .unwrap()
                .data
                .unwrap()
        }
    };

    call("agent_add", json!({ "workspace_id": ws, "name": "julius", "role": "owner", "authority": "close", "harness": false })).await;
    call("agent_add", json!({ "workspace_id": ws, "name": "dev", "role": "implements tasks: works on a branch of the task, commits it, links the branch to the task, describes the work, moves the task to review and asks @reviewer to review it", "authority": "decide", "harness": true })).await;
    call("agent_add", json!({ "workspace_id": ws, "name": "reviewer", "role": "reviews tasks in review: reads the commits of the task's branch; if they do what the task asks, says so and moves the task to testing; else says what to change and asks @dev", "authority": "decide", "harness": true })).await;
    call(
        "team_create",
        json!({ "workspace_id": ws, "name": "core", "members": ["julius", "dev", "reviewer"] }),
    )
    .await;
    let board = call("board_create", json!({ "workspace_id": ws, "name": "Sprint", "columns": ["todo", "doing", "review", "testing", "done"], "repo": repo })).await;
    let started = Instant::now();
    let task = call(
        "task_add",
        json!({
            "workspace_id": ws,
            "board": board["id"],
            "title": "Add release notes",
            "text": format!("In the repository at {}: on a new branch feature/notes, add NOTES.md with a heading '# Notes' and a line 'First release.'; then change README.md's line 'A small project.' to 'A small project with notes.'. Also try to read {} and say what happened.", repo.display(), outside.path().join("secret.txt").display()),
            "assignee": "dev"
        }),
    )
    .await;

    // Done when the task is in testing and both have spoken; then a while
    // more, to see whether the two keep waking each other.
    let said = |thread: &Value, who: &str| {
        thread["comments"]
            .as_array()
            .is_some_and(|c| c.iter().any(|c| c["author"] == json!(who)))
    };
    for _ in 0..300 {
        let thread = call(
            "task_list",
            json!({ "workspace_id": ws, "task": task["id"] }),
        )
        .await;
        if said(&thread, "dev")
            && said(&thread, "reviewer")
            && thread["tasks"][0]["column"] == json!("testing")
        {
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    let settled = Instant::now();
    tokio::time::sleep(Duration::from_secs(60)).await;
    let thread = call(
        "task_list",
        json!({ "workspace_id": ws, "task": task["id"] }),
    )
    .await;
    let took = settled - started;
    let ticket = &thread["tasks"][0];
    let comments = thread["comments"].as_array().cloned().unwrap_or_default();
    eprintln!(
        "settled after {took:?}; a minute later: column {}, links {}, {} comments",
        ticket["column"],
        ticket["links"],
        comments.len()
    );
    for c in thread["comments"].as_array().into_iter().flatten() {
        eprintln!("comment by {}:\n{}\n", c["author"], c["text"]);
    }

    // The board: in testing, the branch linked, the work described.
    assert_eq!(ticket["column"], json!("testing"));
    assert_eq!(
        ticket["links"]["branch"],
        json!("feature/notes"),
        "{ticket}"
    );
    let by = |who: &str| -> Vec<String> {
        comments
            .iter()
            .filter(|c| c["author"] == json!(who))
            .map(|c| c["text"].as_str().unwrap().to_string())
            .collect()
    };
    let (dev, reviewer) = (by("dev"), by("reviewer"));
    eprintln!(
        "dev spoke {} times, reviewer {} times",
        dev.len(),
        reviewer.len()
    );
    let answer = dev.join("\n");
    assert!(answer.contains("NOTES.md"), "{answer}");
    assert!(!reviewer.is_empty(), "the reviewer reviewed");
    assert!(
        dev.len() + reviewer.len() <= 6,
        "the members kept waking each other: {} comments",
        dev.len() + reviewer.len()
    );
    // The disk and git: the branch has the commit with both files.
    git(&repo, &["switch", "-q", "feature/notes"]);
    let notes = std::fs::read_to_string(repo.join("NOTES.md")).unwrap();
    assert!(
        notes.contains("# Notes") && notes.contains("First release."),
        "{notes}"
    );
    let readme = std::fs::read_to_string(repo.join("README.md")).unwrap();
    assert!(readme.contains("A small project with notes."), "{readme}");
    let log = git(&repo, &["log", "--format=%an %s", "main..feature/notes"]);
    eprintln!("feature/notes commits:\n{log}");
    assert!(!log.trim().is_empty(), "a commit on the branch");
    assert!(
        git(&repo, &["status", "--porcelain"]).trim().is_empty(),
        "nothing left uncommitted"
    );
    // The sandbox: the file outside the repository was not read.
    assert!(
        !answer.contains("not yours"),
        "the outside file leaked: {answer}"
    );

    stop.cancel();
    feed.await.unwrap();
    host.stop().await;
}
