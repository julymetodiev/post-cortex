// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_documents::InMemoryDocuments;
use post_cortex_proto::board::{
    AddTaskRequest, DecideRequest, Decision, DecisionApproved, DecisionRecord, TaskRecord, Thread,
    WithdrawDecisionRequest,
};
use post_cortex_proto::dynamic::{Check, File, Grants, Language};
use post_cortex_proto::pb::Attachment;
use tokio_util::sync::CancellationToken;

use super::*;

#[derive(Default)]
struct World {
    blobs: Mutex<HashMap<String, Vec<u8>>>,
    prepared: Mutex<Vec<String>>,
    /// What the locked run answers; none: a passing run using nothing.
    check: Mutex<Option<Check>>,
    /// Refuses the dependencies.
    unmet: Mutex<bool>,
    questions: Mutex<Vec<DecideRequest>>,
    policy: Mutex<Option<String>>,
    thread: Mutex<Thread>,
    changed: Mutex<Vec<(String, bool)>>,
    /// Each decision withdrawn, with who withdrew it.
    withdrawn: Mutex<Vec<(String, String)>>,
}

impl World {
    fn withdrawn(&self) -> Vec<(String, String)> {
        self.withdrawn.lock().clone()
    }
}

/// `id`, withdrawn by `dynamic:greet`.
fn by_greet(id: &str) -> (String, String) {
    ("dynamic:greet".to_string(), id.to_string())
}

#[async_trait]
impl Blobs for World {
    async fn put(&self, bytes: Vec<u8>) -> Result<Attachment, ModuleError> {
        let sha256 = domain::sha256(&bytes);
        self.blobs.lock().insert(sha256.clone(), bytes);
        Ok(Attachment {
            sha256,
            ..Default::default()
        })
    }

    async fn get(&self, sha256: &str) -> Result<Vec<u8>, ModuleError> {
        self.blobs
            .lock()
            .get(sha256)
            .cloned()
            .ok_or_else(|| ModuleError::new(ModuleErrorKind::NotFound, sha256.to_string()))
    }
}

#[async_trait]
impl Bundles for World {
    async fn prepare(&self, _: Vec<u8>, sha256: &str) -> Result<Prepared, ModuleError> {
        if *self.unmet.lock() {
            return Err(ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                "numpy 2.0: modules share numpy 2.5.4",
            ));
        }
        self.prepared.lock().push(sha256.to_string());
        Ok(Prepared {
            bundle: PathBuf::from(format!("/store/{sha256}")),
            environment: None,
        })
    }

    async fn check(&self, _: Checking<'_>, _: &Prepared) -> Result<Check, ModuleError> {
        Ok(self.check.lock().clone().unwrap_or(Check {
            tests_passed: true,
            ..Default::default()
        }))
    }
}

#[async_trait]
impl Board for World {
    async fn task(&self, _: &str, _: AddTaskRequest) -> Result<TaskRecord, ModuleError> {
        Ok(TaskRecord {
            id: "task-1".into(),
            ..Default::default()
        })
    }

    async fn decide(
        &self,
        _: &str,
        question: DecideRequest,
    ) -> Result<DecisionRecord, ModuleError> {
        self.questions.lock().push(question);
        Ok(DecisionRecord {
            id: format!("decision-{}", self.questions.lock().len()),
            decision: Some(Decision {
                approved_by: self.policy.lock().clone(),
                ..Default::default()
            }),
            ..Default::default()
        })
    }

    async fn thread(&self, _: &str, _: &str) -> Result<Thread, ModuleError> {
        Ok(self.thread.lock().clone())
    }

    async fn withdraw(
        &self,
        asker: &str,
        request: WithdrawDecisionRequest,
    ) -> Result<DecisionRecord, ModuleError> {
        self.withdrawn
            .lock()
            .push((asker.to_string(), request.decision.clone()));
        Ok(DecisionRecord {
            id: request.decision,
            ..Default::default()
        })
    }
}

#[async_trait]
impl Directory for World {
    async fn files(&self, _: &Scope, dir: &str) -> Result<Vec<File>, ModuleError> {
        Ok(vec![File {
            path: "module.py".into(),
            text: format!("# from {dir}"),
        }])
    }
}

#[async_trait]
impl Reach for World {
    async fn modules(&self, _: &Scope) -> Result<BTreeSet<String>, ModuleError> {
        Ok(BTreeSet::from(["http".to_string()]))
    }
}

#[async_trait]
impl Events for World {
    async fn changed(&self, id: &str, removed: bool) {
        self.changed.lock().push((id.to_string(), removed));
    }
}

const SETTINGS: fn() -> Settings = || Settings {
    workspace: "w".into(),
    board: "b".into(),
    max_modules: None,
};

fn scope() -> Scope {
    Scope::own(
        "tester",
        "w",
        Duration::from_secs(5),
        CancellationToken::new(),
    )
}

fn request(text: &str, uses: &[&str]) -> InstallRequest {
    InstallRequest {
        id: "greet".into(),
        version: "1".into(),
        language: Language::Python.into(),
        files: vec![File {
            path: "module.py".into(),
            text: text.into(),
        }],
        grants: Some(Grants {
            uses: uses.iter().map(|u| (*u).to_string()).collect(),
            ..Default::default()
        }),
        dir: String::new(),
    }
}

/// Runs `body` with the service over a fresh world.
macro_rules! service {
    ($world:ident, $docs:ident, $settings:ident, $service:ident) => {
        let $world = World::default();
        let $docs =
            InMemoryDocuments::new(Some("dynamic"), post_cortex_proto::DYNAMIC_DESCRIPTOR_SET)
                .unwrap();
        let $settings = SETTINGS();
        let $service = Service::new(
            Ports {
                documents: &$docs,
                blobs: &$world,
                bundles: &$world,
                board: &$world,
                reach: &$world,
                directory: &$world,
                events: &$world,
            },
            &$settings,
        );
    };
}

#[tokio::test]
async fn a_bundle_comes_from_a_directory_as_it_is_there_and_not_from_both() {
    service!(world, docs, settings, service);
    let from_dir = InstallRequest {
        files: Vec::new(),
        dir: "/scratch/greet".into(),
        ..request("", &[])
    };
    let installed = service.install(&scope(), from_dir.clone()).await.unwrap();
    assert!(installed.module.unwrap().asked.is_some());

    let both = InstallRequest {
        files: request("print(1)", &[]).files,
        ..from_dir
    };
    let refused = service.install(&scope(), both).await.unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::InvalidArgument);
    assert!(refused.message.contains("not both"), "{}", refused.message);
}

#[tokio::test]
async fn a_version_waits_for_a_person_and_runs_once_approved() {
    service!(world, docs, settings, service);
    let installed = service
        .install(&scope(), request("print(1)", &["http"]))
        .await
        .unwrap();
    let module = installed.module.unwrap();
    assert!(module.approved.is_none());
    assert_eq!(module.asked.as_ref().unwrap().decision, "decision-1");
    assert_eq!(
        world.questions.lock()[0].risk,
        i32::from(post_cortex_proto::board::Risk::Code)
    );
    assert!(service.approved().await.unwrap().modules.is_empty());
    assert!(world.changed.lock().is_empty());

    service
        .decided(&DecisionApproved {
            decision: "decision-1".into(),
            task: "task-1".into(),
            approved_by: "ana".into(),
        })
        .await
        .unwrap();
    let runs = service.approved().await.unwrap().modules;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].id, "greet");
    assert_eq!(
        world.changed.lock().as_slice(),
        [("greet".to_string(), false)]
    );

    // New code asks again, and the approved version goes on meanwhile.
    let hash = runs[0].sha256.clone();
    service
        .install(&scope(), request("print(2)", &["http"]))
        .await
        .unwrap();
    assert_eq!(world.questions.lock().len(), 2);
    let runs = service.approved().await.unwrap().modules;
    assert_eq!(runs[0].sha256, hash);
    let got = service.get("greet").await.unwrap();
    assert!(got.difference.contains("+print(2)"), "{}", got.difference);

    // Removed while version 2 waits: no one is asked about it any more.
    assert!(world.withdrawn().is_empty());
    service.remove("greet").await.unwrap();
    assert_eq!(world.withdrawn(), [by_greet("decision-2")]);
    assert!(service.approved().await.unwrap().modules.is_empty());
    assert!(matches!(
        service.get("greet").await,
        Err(e) if e.kind == ModuleErrorKind::NotFound
    ));
    assert_eq!(
        world.changed.lock().last().unwrap(),
        &("greet".to_string(), true)
    );
}

#[tokio::test]
async fn what_breaks_a_rule_is_refused_and_keeps_nothing() {
    service!(world, docs, settings, service);
    let kept_nothing = |e: ModuleError, kind, says: &str| {
        assert_eq!(e.kind, kind, "{e}");
        assert!(e.message.contains(says), "{e}");
    };
    kept_nothing(
        service
            .install(&scope(), request("a", &["mail"]))
            .await
            .unwrap_err(),
        ModuleErrorKind::Denied,
        "mail",
    );
    kept_nothing(
        service
            .install(&scope(), request("a", &["dynamic"]))
            .await
            .unwrap_err(),
        ModuleErrorKind::InvalidArgument,
        "dynamic",
    );
    *world.check.lock() = Some(Check {
        tests_passed: false,
        log: "assert failed".into(),
        ..Default::default()
    });
    kept_nothing(
        service
            .install(&scope(), request("a", &[]))
            .await
            .unwrap_err(),
        ModuleErrorKind::FailedPrecondition,
        "assert failed",
    );
    *world.check.lock() = Some(Check {
        tests_passed: true,
        uses: vec!["http".into()],
        ..Default::default()
    });
    kept_nothing(
        service
            .install(&scope(), request("a", &[]))
            .await
            .unwrap_err(),
        ModuleErrorKind::FailedPrecondition,
        "http",
    );
    *world.check.lock() = None;
    *world.unmet.lock() = true;
    kept_nothing(
        service
            .install(&scope(), request("a", &[]))
            .await
            .unwrap_err(),
        ModuleErrorKind::FailedPrecondition,
        "numpy",
    );
    assert!(service.list().await.unwrap().modules.is_empty());
    assert!(world.questions.lock().is_empty());
}

#[tokio::test]
async fn the_plan_limits_new_modules_and_never_a_new_version() {
    let world = World::default();
    let docs =
        InMemoryDocuments::new(Some("dynamic"), post_cortex_proto::DYNAMIC_DESCRIPTOR_SET).unwrap();
    let settings = Settings {
        max_modules: Some(1),
        ..SETTINGS()
    };
    let service = Service::new(
        Ports {
            documents: &docs,
            blobs: &world,
            bundles: &world,
            board: &world,
            reach: &world,
            directory: &world,
            events: &world,
        },
        &settings,
    );
    let named = |id: &str, code: &str| InstallRequest {
        id: id.into(),
        ..request(code, &[])
    };
    service
        .install(&scope(), named("first", "a"))
        .await
        .unwrap();
    service
        .install(&scope(), named("first", "b"))
        .await
        .unwrap();
    let refused = service
        .install(&scope(), named("second", "a"))
        .await
        .unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::ResourceExhausted);
    assert!(
        refused.message.contains("plan holds 1"),
        "{}",
        refused.message
    );
}

#[tokio::test]
async fn a_person_edits_a_file_and_it_waits_as_an_install_does() {
    service!(world, docs, settings, service);
    service
        .install(&scope(), request("print(1)", &["http"]))
        .await
        .unwrap();
    service
        .decided(&DecisionApproved {
            decision: "decision-1".into(),
            task: "task-1".into(),
            approved_by: "ana".into(),
        })
        .await
        .unwrap();
    let approved = service.get("greet").await.unwrap();
    assert_eq!(approved.files[0].text, "print(1)");
    assert_eq!(approved.difference, domain::NO_DIFFERENCE);
    let hash = service.approved().await.unwrap().modules[0].sha256.clone();

    let rows = service.module_files("greet").await.unwrap().files;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].path, "module.py");
    assert_eq!(rows[0].shown, "`module.py`\n\n```py\nprint(1)\n```");
    let edit = |revision, version: &str, path: &str, text: &str| EditRequest {
        id: "greet".into(),
        revision,
        version: version.into(),
        path: path.into(),
        text: text.into(),
    };
    let edited = service
        .edit(
            &scope(),
            edit(rows[0].revision, "2", "module.py", "print(2)"),
        )
        .await
        .unwrap();
    let module = edited.module.unwrap();
    assert_eq!(module.version, "2");
    assert_eq!(module.language(), Language::Python);
    assert_eq!(module.grants.unwrap().uses, ["http"]);
    assert_eq!(world.questions.lock().len(), 2);
    // The approved version goes on while the edit waits.
    assert_eq!(service.approved().await.unwrap().modules[0].sha256, hash);
    let got = service.get("greet").await.unwrap();
    assert_eq!(got.files[0].text, "print(2)");
    assert!(got.difference.contains("+print(2)"), "{}", got.difference);

    // What it read is old: it does not overwrite what came since.
    let stale = service
        .edit(
            &scope(),
            edit(rows[0].revision, "3", "module.py", "print(3)"),
        )
        .await;
    assert!(matches!(stale, Err(e) if e.kind == ModuleErrorKind::Conflict));

    // A path it does not have adds a file, under the same checks.
    let added = service
        .edit(&scope(), edit(got.version, "3", "notes.txt", "hello"))
        .await
        .unwrap();
    assert_eq!(service.get("greet").await.unwrap().files.len(), 2);
    let refused = service
        .edit(&scope(), edit(added.version, "4", "../out", "x"))
        .await;
    assert!(matches!(refused, Err(e) if e.kind == ModuleErrorKind::InvalidArgument));
    assert!(matches!(
        service.edit(&scope(), EditRequest { id: "none".into(), ..Default::default() }).await,
        Err(e) if e.kind == ModuleErrorKind::NotFound
    ));
}

mod approvals;
