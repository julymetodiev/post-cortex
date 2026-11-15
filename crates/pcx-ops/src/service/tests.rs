// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Ops' service over documents, a cluster, a board and a channel held in
//! memory.

use std::collections::HashMap;

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_documents::InMemoryDocuments;
use post_cortex_proto::board::{Decision as Decided, DecisionRecord};
use post_cortex_proto::ops::{
    KubeThing, NomadAlloc, NomadChildren, NomadDeployment, NomadJob, OwnerRef, Revert, ThingList,
    ThingMeta, ThingSpec,
};

use super::*;

use crate::ports::{Decision, Patch};

/// A cluster: objects by path; what was patched and deleted. And Nomad:
/// each job's versions, the newest first (the one it runs), its launches
/// and its allocations; what was stopped, launched and reverted.
#[derive(Default)]
struct World {
    objects: Mutex<HashMap<String, KubeThing>>,
    lists: Mutex<HashMap<String, ThingList>>,
    patched: Mutex<Vec<(String, Patch)>>,
    deleted: Mutex<Vec<String>>,
    versions: Mutex<HashMap<String, Vec<NomadJob>>>,
    deploying: Mutex<Vec<String>>,
    launches: Mutex<HashMap<String, NomadChildren>>,
    allocations: Mutex<Vec<NomadAlloc>>,
    stopped: Mutex<Vec<String>>,
    launched: Mutex<Vec<String>>,
    reverted: Mutex<Vec<Revert>>,
    /// A job Nomad refuses to revert.
    unrevertible: Option<&'static str>,
    /// Whether the team's trust approves at once.
    trusted: bool,
    decisions: Mutex<Vec<(String, Decision, DecisionRecord)>>,
    comments: Mutex<Vec<String>>,
    told: Mutex<Vec<String>>,
}

#[async_trait]
impl Cluster for World {
    async fn get(&self, _env: &Environment, path: &str) -> Result<KubeThing, ModuleError> {
        self.objects
            .lock()
            .get(path)
            .cloned()
            .ok_or_else(|| ModuleError::new(ModuleErrorKind::NotFound, format!("no {path}")))
    }
    async fn list(&self, _env: &Environment, path: &str) -> Result<ThingList, ModuleError> {
        Ok(self.lists.lock().get(path).cloned().unwrap_or_default())
    }
    async fn patch(&self, _env: &Environment, path: &str, patch: Patch) -> Result<(), ModuleError> {
        self.patched.lock().push((path.to_string(), patch));
        Ok(())
    }
    async fn delete(&self, _env: &Environment, path: &str) -> Result<(), ModuleError> {
        self.deleted.lock().push(path.to_string());
        Ok(())
    }
}

#[async_trait]
impl Nomad for World {
    async fn job(&self, _env: &Environment, name: &str) -> Result<NomadJob, ModuleError> {
        self.versions
            .lock()
            .get(name)
            .and_then(|kept| kept.first().cloned())
            .ok_or_else(|| ModuleError::new(ModuleErrorKind::NotFound, format!("no job {name}")))
    }
    async fn versions(&self, _env: &Environment, name: &str) -> Result<Vec<NomadJob>, ModuleError> {
        Ok(self.versions.lock().get(name).cloned().unwrap_or_default())
    }
    async fn deployment(
        &self,
        _env: &Environment,
        name: &str,
    ) -> Result<Option<NomadDeployment>, ModuleError> {
        let running = self.deploying.lock().iter().any(|job| job == name);
        Ok(Some(NomadDeployment {
            status: if running { "running" } else { "successful" }.into(),
        }))
    }
    async fn launches(&self, _env: &Environment, name: &str) -> Result<NomadChildren, ModuleError> {
        Ok(self.launches.lock().get(name).cloned().unwrap_or_default())
    }
    async fn allocations(
        &self,
        _env: &Environment,
        start: &str,
    ) -> Result<Vec<NomadAlloc>, ModuleError> {
        let all = self.allocations.lock();
        Ok(all
            .iter()
            .filter(|a| a.id.starts_with(start))
            .cloned()
            .collect())
    }
    async fn stop(&self, _env: &Environment, id: &str) -> Result<(), ModuleError> {
        self.stopped.lock().push(id.to_string());
        Ok(())
    }
    async fn launch(&self, _env: &Environment, name: &str) -> Result<(), ModuleError> {
        self.launched.lock().push(name.to_string());
        Ok(())
    }
    async fn revert(&self, _env: &Environment, revert: &Revert) -> Result<(), ModuleError> {
        if self.unrevertible == Some(revert.job.as_str()) {
            return Err(ModuleError::new(
                ModuleErrorKind::Denied,
                "Nomad refused /v1/job/x/revert: Permission denied",
            ));
        }
        self.reverted.lock().push(revert.clone());
        Ok(())
    }
}

#[async_trait]
impl Board for World {
    async fn decide(
        &self,
        author: &str,
        decision: Decision,
    ) -> Result<DecisionRecord, ModuleError> {
        // As the team's policy: a decision only a person approves waits.
        let trusted = self.trusted && decision.stakes.person != Some(true);
        let mut decisions = self.decisions.lock();
        let record = DecisionRecord {
            id: format!("D-{}", decisions.len() + 1),
            version: 1,
            pending: !trusted,
            decision: Some(Decided {
                task: decision.task.clone(),
                what: decision.what.clone(),
                approved_by: trusted.then(|| "policy:1".to_string()),
                ..Decided::default()
            }),
            ..DecisionRecord::default()
        };
        decisions.push((author.to_string(), decision, record.clone()));
        Ok(record)
    }
    async fn decision(
        &self,
        _ws: &str,
        _task: &str,
        id: &str,
    ) -> Result<Option<DecisionRecord>, ModuleError> {
        Ok(self
            .decisions
            .lock()
            .iter()
            .find(|(_, _, r)| r.id == id)
            .map(|(_, _, r)| r.clone()))
    }
    async fn withdraw(
        &self,
        author: &str,
        _ws: &str,
        id: &str,
        why: &str,
    ) -> Result<(), ModuleError> {
        let mut decisions = self.decisions.lock();
        let Some((asker, _, record)) = decisions.iter_mut().find(|(_, _, r)| r.id == id) else {
            return Err(ModuleError::new(ModuleErrorKind::NotFound, "no decision"));
        };
        if asker != author || !record.pending {
            return Err(ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                "only its author withdraws a decision that waits",
            ));
        }
        record.pending = false;
        record.decision.get_or_insert_default().withdrawn = Some(why.to_string());
        Ok(())
    }
    async fn comment(&self, _ws: &str, _task: &str, text: &str) -> Result<(), ModuleError> {
        self.comments.lock().push(text.to_string());
        Ok(())
    }
}

#[async_trait]
impl Teller for World {
    async fn tell(&self, _destination: &str, text: &str) -> Result<(), ModuleError> {
        self.told.lock().push(text.to_string());
        Ok(())
    }
}

struct At;

impl Clock for At {
    fn now(&self) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-27T08:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }
}

fn config() -> OpsConfig {
    OpsConfig {
        workspace_id: "ops-ws".into(),
        environments: vec![
            Environment {
                name: "prod".into(),
                kube_api: "https://k8s".into(),
                kube_token_file: "/creds/t".into(),
                ..Environment::default()
            },
            Environment {
                name: "pcx".into(),
                nomad_api: "https://nomad:4646".into(),
                nomad_token_file: "/creds/n".into(),
                nomad_jobs: ["pcx-app", "pcx-cloud", "surrealdb-backup"]
                    .map(String::from)
                    .to_vec(),
                ..Environment::default()
            },
        ],
        notify: "ops".into(),
        ..OpsConfig::default()
    }
}

fn documents() -> InMemoryDocuments {
    InMemoryDocuments::new(None, post_cortex_proto::OPS_DESCRIPTOR_SET)
        .unwrap()
        .written_at(At.now())
}

fn deployment(replicas: u32) -> KubeThing {
    KubeThing {
        spec: Some(ThingSpec {
            replicas,
            resources: None,
        }),
        ..KubeThing::default()
    }
}

fn world(trusted: bool) -> World {
    let world = World {
        trusted,
        ..World::default()
    };
    world
        .objects
        .lock()
        .insert(paths::deployment("equal-rust", "svc-music"), deployment(2));
    world.objects.lock().insert(
        paths::pod("equal-rust", "svc-music-7d9"),
        KubeThing {
            metadata: Some(ThingMeta {
                name: "svc-music-7d9".into(),
                owner_references: vec![OwnerRef {
                    kind: "ReplicaSet".into(),
                    controller: true,
                }],
                ..ThingMeta::default()
            }),
            ..KubeThing::default()
        },
    );
    world
}

fn service<'a>(
    config: &'a OpsConfig,
    docs: &'a InMemoryDocuments,
    world: &'a World,
) -> Service<'a> {
    Service {
        config,
        docs,
        cluster: world,
        nomad: world,
        board: world,
        teller: world,
        clock: &At,
    }
}

fn restart(dry_run: bool) -> Asked {
    Asked {
        ask: Ask {
            environment: "prod".into(),
            workspace_id: "equal".into(),
            task: "t-1".into(),
            why: "svc-music crash-loops: 7 restarts".into(),
            dry_run,
        },
        wanted: Wanted::Restart {
            namespace: "equal-rust".into(),
            name: "svc-music".into(),
        },
    }
}

fn scale(replicas: u32) -> Asked {
    Asked {
        wanted: Wanted::Scale {
            namespace: "equal-rust".into(),
            name: "svc-music".into(),
            replicas,
        },
        ..restart(false)
    }
}

#[tokio::test]
async fn a_dry_run_says_what_it_would_do_and_decides_nothing() {
    let (config, docs, world) = (config(), documents(), world(true));
    let answer = service(&config, &docs, &world)
        .act("eq-medic", restart(true))
        .await
        .unwrap();
    assert_eq!(
        answer.would,
        "would restart deployment equal-rust/svc-music; now: 2 replicas"
    );
    assert!(world.decisions.lock().is_empty() && world.patched.lock().is_empty());
}

#[tokio::test]
async fn an_action_the_trust_approves_runs_at_once_and_is_said() {
    let (config, docs, world) = (config(), documents(), world(true));
    let answer = service(&config, &docs, &world)
        .act("eq-medic", restart(false))
        .await
        .unwrap();
    let action = answer.action.unwrap();
    assert_eq!(action.state(), State::Done);
    assert_eq!(action.decision, "D-1");
    let (author, decision, _) = world.decisions.lock()[0].clone();
    assert_eq!(author, "eq-medic", "the asker decides");
    assert_eq!(decision.stakes.action.as_deref(), Some("restart"));
    assert_eq!(decision.stakes.environment.as_deref(), Some("prod"));
    let (path, patch) = world.patched.lock()[0].clone();
    assert_eq!(
        path,
        "/apis/apps/v1/namespaces/equal-rust/deployments/svc-music"
    );
    assert!(matches!(patch, Patch::Deployment(_)));
    let comment = world.comments.lock()[0].clone();
    assert!(comment.starts_with("Done: restart deployment equal-rust/svc-music"));
    assert!(!comment.contains('@'), "its asker checks in its own turn");
    assert_eq!(world.told.lock().len(), 1);

    let again = service(&config, &docs, &world)
        .act("eq-medic", restart(false))
        .await
        .unwrap_err();
    assert_eq!(again.kind, ModuleErrorKind::FailedPrecondition, "a brake");
    assert_eq!(world.patched.lock().len(), 1);
}

#[tokio::test]
async fn an_action_waits_for_a_person_then_runs_when_approved() {
    let (config, docs, world) = (config(), documents(), world(false));
    let svc = service(&config, &docs, &world);
    let waiting = svc.act("eq-medic", restart(false)).await.unwrap();
    assert_eq!(waiting.action.unwrap().state(), State::Waiting);
    assert!(
        world.patched.lock().is_empty(),
        "nothing runs before approval"
    );
    assert_eq!(svc.decided("D-1").await.unwrap(), None, "not approved yet");

    let held = svc.act("eq-medic", restart(false)).await.unwrap_err();
    assert!(held.message.contains("waits for approval"));

    {
        let mut decisions = world.decisions.lock();
        let record = &mut decisions[0].2;
        record.pending = false;
        record.decision.as_mut().unwrap().approved_by = Some("julius".into());
    }
    let ran = svc.decided("D-1").await.unwrap().unwrap();
    assert_eq!(ran.action.unwrap().state(), State::Done);
    assert_eq!(world.patched.lock().len(), 1);
    let comment = world.comments.lock()[0].clone();
    assert!(comment.starts_with("Done: restart deployment equal-rust/svc-music"));
    assert!(
        comment.contains("\n\n@eq-medic: "),
        "run after its asker's turn, it names the asker to check: {comment}"
    );
    assert!(
        !world.told.lock()[0].contains('@'),
        "the channel is no member"
    );
    assert_eq!(
        svc.decided("D-1").await.unwrap(),
        None,
        "an action runs once"
    );
    let listed = svc.list(0).await.unwrap();
    assert_eq!(listed.actions.len(), 1);
}

#[tokio::test]
async fn a_target_that_changed_before_its_approval_is_a_failed_action() {
    let (config, docs, world) = (config(), documents(), world(false));
    let svc = service(&config, &docs, &world);
    let delete = Asked {
        wanted: Wanted::DeletePod {
            namespace: "equal-rust".into(),
            name: "svc-music-7d9".into(),
        },
        ..restart(false)
    };
    svc.act("eq-medic", delete).await.unwrap();
    world
        .objects
        .lock()
        .remove(&paths::pod("equal-rust", "svc-music-7d9"));
    {
        let mut decisions = world.decisions.lock();
        decisions[0].2.pending = false;
        decisions[0].2.decision.as_mut().unwrap().approved_by = Some("julius".into());
    }
    let ran = svc.decided("D-1").await.unwrap().unwrap().action.unwrap();
    assert_eq!(ran.state(), State::Failed);
    assert!(
        ran.result
            .contains("no /api/v1/namespaces/equal-rust/pods/svc-music-7d9")
    );
    assert!(world.deleted.lock().is_empty());
    assert!(world.comments.lock()[0].starts_with("Not done"));
}

#[tokio::test]
async fn requests_the_rules_refuse_reach_neither_the_board_nor_the_cluster() {
    let (config, docs, world) = (config(), documents(), world(true));
    let svc = service(&config, &docs, &world);
    assert_eq!(
        svc.act("eq-medic", scale(9)).await.unwrap_err().kind,
        ModuleErrorKind::InvalidArgument
    );
    assert_eq!(
        svc.act("eq-medic", scale(2)).await.unwrap_err().kind,
        ModuleErrorKind::FailedPrecondition,
        "it has 2 already"
    );
    let elsewhere = Asked {
        ask: Ask {
            environment: "dev".into(),
            ..restart(false).ask
        },
        ..restart(false)
    };
    assert_eq!(
        svc.act("eq-medic", elsewhere).await.unwrap_err().kind,
        ModuleErrorKind::NotFound
    );
    let traversal = Asked {
        wanted: Wanted::Restart {
            namespace: "equal-rust".into(),
            name: "../../secrets".into(),
        },
        ..restart(false)
    };
    assert_eq!(
        svc.act("eq-medic", traversal).await.unwrap_err().kind,
        ModuleErrorKind::InvalidArgument
    );
    assert!(world.decisions.lock().is_empty() && world.patched.lock().is_empty());
}

mod nomad;
mod waiting;
