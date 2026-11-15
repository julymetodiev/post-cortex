// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A module's data and the reach (`docs/connect.md` section 4a), composed
//! over memory: a person limited to one workspace uses a module whose store
//! is the organisation's and changes nothing of it, and a module that keeps
//! its work by workspace answers with the work of the workspace it is asked
//! from.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pcx_host::Host;
use pcx_module_api::{Module, ModuleErrorKind, Scope};
use pcx_scheduler::SchedulerModule;
use pcx_skills::SkillsModule;
use post_cortex_memory::capabilities::CreateWorkspace;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::memory_tests::as_person;

/// A person's call asked from `workspace`, reaching `reach` (none: every
/// workspace).
fn asked_from(workspace: &str, reach: Option<Vec<String>>) -> Scope {
    Scope {
        tenant: String::new(),
        profile: String::new(),
        session_id: None,
        workspace_id: Some(workspace.to_string()),
        author: Some("martin".to_string()),
        caller_module: None,
        call_depth: 0,
        turn: None,
        reach,
        role: None,
        read_only: false,
        person: None,
        deadline: Instant::now() + Duration::from_secs(20),
        cancel: CancellationToken::new(),
    }
}

/// A host with the skills and the scheduler, both set to workspace `own`,
/// and another workspace `rg`: the host, `own`, `rg`.
async fn host() -> (Host, String, String, tempfile::TempDir) {
    let data = tempfile::tempdir().unwrap();
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
    let mut ids = Vec::new();
    for name in ["own", "rg"] {
        let w = memory
            .workspaces
            .create(CreateWorkspace {
                name: name.into(),
                description: String::new(),
            })
            .await
            .unwrap();
        ids.push(w.id.to_string());
    }
    let (own, rg) = (ids[0].clone(), ids[1].clone());
    let modules: Vec<Arc<dyn Module>> = vec![
        crate::memory_tests::memory_module(&memory).0,
        Arc::new(SkillsModule::new()),
        Arc::new(SchedulerModule::new()),
    ];
    let config = HashMap::from([
        (SkillsModule::ID.to_string(), json!({ "workspace_id": own })),
        (
            SchedulerModule::ID.to_string(),
            json!({ "workspace_id": own }),
        ),
    ]);
    let host = Host::start(modules, &config, &HashMap::new())
        .await
        .unwrap();
    (host, own, rg, data)
}

fn skill(name: &str) -> Value {
    json!({ "name": name, "description": "A check.", "instructions": "Say done." })
}

fn job(name: &str) -> Value {
    json!({ "job": {
        "name": name,
        "cron": { "expression": "0 9 * * 1-5" },
        "script": { "program": "true" },
    } })
}

fn names(listed: &Value) -> Vec<String> {
    listed["jobs"]
        .as_array()
        .map(|jobs| {
            jobs.iter()
                .filter_map(|j| j["job"]["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_person_limited_to_a_workspace_uses_the_organisations_store_and_does_not_change_it() {
    let (host, own, rg, _data) = host().await;
    let limited = asked_from(&rg, Some(vec![rg.clone()]));
    // The library is the organisation's: read from a workspace it is not in.
    as_person(&host, &limited, "skill_list", json!({}))
        .await
        .expect("a limited person reads the organisation's skills");
    // A change is the caller's write, and the library's workspace is
    // outside their reach.
    let refused = as_person(&host, &limited, "skill_add", skill("limited-one"))
        .await
        .expect_err("a limited person does not change the organisation's skills");
    assert_eq!(refused.kind, ModuleErrorKind::Denied, "{}", refused.message);
    assert!(
        refused.message.contains("the organisation's"),
        "{}",
        refused.message
    );
    // A person who reaches every workspace, asked from the same one, does.
    let whole = asked_from(&rg, None);
    as_person(&host, &whole, "skill_add", skill("whole-one"))
        .await
        .expect("a person who reaches the library's workspace adds a skill");
    // And one limited to the library's own workspace.
    let of_own = asked_from(&own, Some(vec![own.clone()]));
    as_person(&host, &of_own, "skill_add", skill("own-one"))
        .await
        .expect("a person limited to the library's workspace adds a skill");
}

#[tokio::test]
async fn a_schedulers_jobs_are_those_of_the_workspace_it_is_asked_from() {
    let (host, own, rg, _data) = host().await;
    let limited = asked_from(&rg, Some(vec![rg.clone()]));
    let whole = asked_from(&own, None);
    as_person(&host, &limited, "job_add", job("rg-daily"))
        .await
        .expect("a limited person adds a job in their workspace");
    as_person(&host, &whole, "job_add", job("own-daily"))
        .await
        .unwrap();
    let in_rg = as_person(&host, &limited, "job_list", json!({}))
        .await
        .unwrap();
    assert_eq!(names(&in_rg), ["rg-daily"]);
    let in_own = as_person(&host, &whole, "job_list", json!({}))
        .await
        .unwrap();
    assert_eq!(names(&in_own), ["own-daily"]);
    // The same name is free in another workspace.
    as_person(&host, &asked_from(&rg, None), "job_add", job("own-daily"))
        .await
        .expect("a job's name is unique in its workspace only");
    // Asked from a workspace outside the reach: refused before it is read.
    let outside = asked_from(&own, Some(vec![rg.clone()]));
    let refused = as_person(&host, &outside, "job_list", json!({}))
        .await
        .expect_err("a workspace outside the reach is refused");
    assert_eq!(refused.kind, ModuleErrorKind::Denied, "{}", refused.message);
}
