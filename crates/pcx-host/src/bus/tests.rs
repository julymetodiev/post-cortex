// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The bus through modules' handles: a declared event stamped and kept, an
//! undeclared or mistyped one refused, a follower fed what it follows after
//! what it acknowledged, woken by a publish; a host without a store keeps
//! none; a module that follows what no module it uses emits is refused.

use std::collections::HashMap;
use std::time::Duration;

use parking_lot::Mutex;
use pcx_module_api::{
    API_VERSION, Event, HostContext, Module, ModuleHost, ModuleKind, Outcome, Resource,
};
use post_cortex_proto::module::EmittedEvent;

use super::*;
use crate::{Host, HostError};

const MOVED: &str = "board.task_moved";
const TASK: &str = "pcx.board.v1.Task";

/// The events kept, in memory.
#[derive(Default)]
struct Kept {
    events: Mutex<Vec<BusEvent>>,
    cursors: Mutex<HashMap<(String, String), u64>>,
}

#[async_trait]
impl EventStore for Kept {
    async fn append(&self, mut event: BusEvent) -> Result<BusEvent, ModuleError> {
        let mut events = self.events.lock();
        event.position = events.len() as u64 + 1;
        events.push(event.clone());
        Ok(event)
    }

    async fn read(
        &self,
        follower: &str,
        name: &str,
        limit: usize,
    ) -> Result<StoredEvents, ModuleError> {
        let cursor = self
            .cursors
            .lock()
            .get(&(follower.to_string(), name.to_string()))
            .copied()
            .unwrap_or(0);
        let events = self
            .events
            .lock()
            .iter()
            .filter(|e| e.name == name && e.position > cursor)
            .take(limit)
            .cloned()
            .collect();
        Ok(StoredEvents {
            events,
            fell_behind: false,
        })
    }

    async fn acknowledge(
        &self,
        follower: &str,
        name: &str,
        position: u64,
    ) -> Result<(), ModuleError> {
        self.cursors
            .lock()
            .insert((follower.to_string(), name.to_string()), position);
        Ok(())
    }
}

/// A module that keeps its handle.
struct Test {
    manifest: Manifest,
    host: Mutex<Option<Arc<dyn ModuleHost>>>,
}

fn module(id: &str, emits: &[&str], uses: &[&str], follows: &[&str]) -> Arc<Test> {
    Arc::new(Test {
        manifest: Manifest {
            id: id.to_string(),
            version: "0.1.0".to_string(),
            api: Some(API_VERSION),
            kind: ModuleKind::Tool.into(),
            descriptors: post_cortex_proto::BOARD_DESCRIPTOR_SET.to_vec(),
            emits: emits
                .iter()
                .map(|name| EmittedEvent {
                    name: (*name).to_string(),
                    r#type: TASK.to_string(),
                    description: String::new(),
                })
                .collect(),
            uses: uses.iter().map(|u| (*u).to_string()).collect(),
            follows: follows.iter().map(|f| (*f).to_string()).collect(),
            ..Manifest::default()
        },
        host: Mutex::new(None),
    })
}

#[async_trait]
impl Module for Test {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    async fn start(&self, ctx: HostContext) -> Result<(), ModuleError> {
        *self.host.lock() = Some(ctx.host);
        Ok(())
    }

    async fn on_event(&self, _: &Scope, _: &Event) -> Result<Outcome, ModuleError> {
        Ok(Outcome::default())
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(ModuleErrorKind::NotFound, uri))
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        Ok(())
    }
}

async fn bus_of(module: &Test) -> Arc<dyn Bus> {
    let host = module.host.lock().clone().unwrap();
    host.bus().await.unwrap()
}

fn task(url: &str) -> Any {
    Any {
        type_url: url.to_string(),
        value: Vec::new().into(),
    }
}

fn scope() -> Scope {
    Scope {
        tenant: String::new(),
        profile: String::new(),
        session_id: None,
        workspace_id: Some("w1".to_string()),
        author: Some("ann".to_string()),
        caller_module: None,
        call_depth: 0,
        turn: None,
        reach: None,
        role: None,
        read_only: false,
        person: None,
        deadline: std::time::Instant::now() + Duration::from_secs(5),
        cancel: tokio_util::sync::CancellationToken::new(),
    }
}

async fn host(store: Option<Arc<dyn EventStore>>) -> (Host, Arc<Test>, Arc<Test>) {
    let board = module("board", &[MOVED], &[], &[]);
    let team = module("team", &[], &["board"], &[MOVED]);
    let modules: Vec<Arc<dyn Module>> = vec![board.clone(), team.clone()];
    let host = Host::start_with(modules, &HashMap::new(), &HashMap::new(), store, "")
        .await
        .unwrap();
    (host, board, team)
}

#[tokio::test]
async fn a_declared_event_is_stamped_kept_and_fed_to_its_follower() {
    let kept = Arc::new(Kept::default());
    let (_host, board, team) = host(Some(kept.clone())).await;
    let board_bus = bus_of(&board).await;
    let url = format!("type.googleapis.com/{TASK}");

    let id = board_bus
        .publish(&scope(), MOVED, task(&url))
        .await
        .unwrap();
    let stored = kept.events.lock()[0].clone();
    assert_eq!(stored.id, id);
    assert_eq!(stored.module, "board");
    assert_eq!(stored.workspace_id, "w1");
    assert_eq!(stored.author, "ann");
    assert!(stored.at.is_some());

    let refused = board_bus.publish(&scope(), "board.other", task(&url)).await;
    assert_eq!(refused.unwrap_err().kind, ModuleErrorKind::InvalidArgument);
    let mistyped = board_bus
        .publish(
            &scope(),
            MOVED,
            task("type.googleapis.com/pcx.board.v1.Board"),
        )
        .await;
    assert_eq!(mistyped.unwrap_err().kind, ModuleErrorKind::InvalidArgument);

    let team_bus = bus_of(&team).await;
    let mut feed = team_bus.follow().await.unwrap();
    let Some(Ok(Followed::Event(first))) = feed.next().await else {
        panic!("the follower was not fed the event");
    };
    assert_eq!(first.id, id);
    team_bus.ack(MOVED, first.position).await.unwrap();
    let foreign = team_bus.ack("board.other", 1).await.unwrap_err();
    assert_eq!(foreign.kind, ModuleErrorKind::InvalidArgument);

    // A publish wakes the feed long before it would read again.
    board_bus
        .publish(&scope(), MOVED, task(&url))
        .await
        .unwrap();
    let next = tokio::time::timeout(Duration::from_secs(2), feed.next())
        .await
        .unwrap();
    let Some(Ok(Followed::Event(second))) = next else {
        panic!("the follower was not woken: {next:?}");
    };
    assert!(second.position > first.position);

    // A new feed starts after what was acknowledged.
    drop(feed);
    let mut again = team_bus.follow().await.unwrap();
    let Some(Ok(Followed::Event(resent))) = again.next().await else {
        panic!("the unacknowledged event was not fed again");
    };
    assert_eq!(resent.id, second.id);
}

#[tokio::test]
async fn a_host_without_a_store_keeps_no_events() {
    let (_host, board, _) = host(None).await;
    let refused = bus_of(&board)
        .await
        .publish(&scope(), MOVED, task(&format!("x/{TASK}")))
        .await;
    assert_eq!(
        refused.unwrap_err().kind,
        ModuleErrorKind::FailedPrecondition
    );
}

#[tokio::test]
async fn a_module_that_follows_what_no_module_it_uses_emits_is_refused() {
    let board = module("board", &[MOVED], &[], &[]);
    for follower in [
        module("team", &[], &[], &[MOVED]),
        module("team", &[], &["board"], &["board.other"]),
    ] {
        let modules: Vec<Arc<dyn Module>> = vec![board.clone(), follower];
        let refused = Host::start(modules, &HashMap::new(), &HashMap::new()).await;
        assert!(matches!(refused, Err(HostError::InvalidManifest { .. })));
    }
}
