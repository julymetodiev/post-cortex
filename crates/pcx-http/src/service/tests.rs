// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The http service over documents held in memory, a clock the test sets,
//! a resolver of fixed addresses and a client that records each request
//! and answers what the test queued.

use std::collections::{HashMap, VecDeque};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use parking_lot::Mutex as Held;
use pcx_documents::InMemoryDocuments;
use post_cortex_proto::http::http_response::Body as Answered;
use post_cortex_proto::http::{
    Auth as AuthKind, Connection as Wire, HttpConfig, ListLogRequest, ListPresetsRequest,
    SavePresetRequest,
};
use post_cortex_proto::workflow::{Field as Wanted, Schema};

use super::*;

struct Fixed(DateTime<Utc>);

impl Clock for Fixed {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

fn at(text: &str) -> Fixed {
    Fixed(
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc),
    )
}

/// Each host's addresses.
struct Addresses(HashMap<String, Vec<SocketAddr>>);

#[async_trait]
impl Resolver for Addresses {
    async fn resolve(&self, host: &str, _port: u16) -> Result<Vec<SocketAddr>, ModuleError> {
        self.0.get(host).cloned().ok_or_else(|| {
            ModuleError::new(
                ModuleErrorKind::Unavailable,
                format!("{host} does not resolve"),
            )
        })
    }
}

fn addresses() -> Addresses {
    let public: SocketAddr = "140.82.112.3:443".parse().unwrap();
    let private: SocketAddr = "10.0.0.7:443".parse().unwrap();
    Addresses(HashMap::from([
        ("api.github.com".to_string(), vec![public]),
        ("objects.example.com".to_string(), vec![public]),
        ("internal.example.com".to_string(), vec![private]),
    ]))
}

/// A client that records each request and answers the queued ones.
#[derive(Default)]
struct Scripted {
    sent: Held<Vec<Planned>>,
    answers: Held<VecDeque<Result<Received, ModuleError>>>,
}

impl Scripted {
    fn answering(answers: Vec<Result<Received, ModuleError>>) -> Self {
        Self {
            sent: Held::default(),
            answers: Held::new(answers.into()),
        }
    }
}

#[async_trait]
impl Client for Scripted {
    async fn send(
        &self,
        request: &Planned,
        _addresses: &[SocketAddr],
        _limit: usize,
    ) -> Result<Received, ModuleError> {
        self.sent.lock().push(request.clone());
        self.answers.lock().pop_front().unwrap_or_else(|| {
            Err(ModuleError::new(
                ModuleErrorKind::Internal,
                "no answer queued",
            ))
        })
    }
}

fn answer(status: u16, headers: &[(&str, &str)], body: &str) -> Result<Received, ModuleError> {
    Ok(Received {
        status,
        reason: "Reason".into(),
        headers: headers
            .iter()
            .map(|(n, v)| ((*n).to_string(), (*v).to_string()))
            .collect(),
        body: body.as_bytes().to_vec(),
        took: Duration::from_millis(12),
    })
}

fn json(body: &str) -> Result<Received, ModuleError> {
    answer(200, &[("Content-Type", "application/json")], body)
}

fn settings(daily_limit: u32) -> Settings {
    domain::settings(
        HttpConfig {
            workspace_id: "6f1c2a3e-0000-4000-8000-000000000001".into(),
            connections: vec![Wire {
                name: "github".into(),
                base_url: "https://api.github.com".into(),
                auth: AuthKind::Bearer.into(),
                secret: "ghp_secret".into(),
                ..Wire::default()
            }],
            allow: vec!["objects.example.com".into(), "internal.example.com".into()],
            daily_limit,
            ..HttpConfig::default()
        },
        false,
    )
    .unwrap()
}

fn docs() -> InMemoryDocuments {
    InMemoryDocuments::new(Some("http"), post_cortex_proto::HTTP_DESCRIPTOR_SET).unwrap()
}

/// What a test runs a service over.
struct World {
    docs: InMemoryDocuments,
    clock: Fixed,
    resolver: Addresses,
    client: Scripted,
    settings: Settings,
    built_in: Vec<Preset>,
    gates: Gates,
}

impl World {
    fn new(client: Scripted) -> Self {
        Self {
            docs: docs(),
            clock: at("2026-11-01T09:00:00Z"),
            resolver: addresses(),
            client,
            settings: settings(0),
            built_in: crate::adapters::built_in().unwrap(),
            gates: Gates::default(),
        }
    }

    fn service(&self) -> Service<'_> {
        Service::new(Parts {
            docs: &self.docs,
            clock: &self.clock,
            resolver: &self.resolver,
            client: &self.client,
            settings: &self.settings,
            built_in: &self.built_in,
            gates: &self.gates,
        })
    }

    async fn log(&self) -> Vec<Call> {
        self.service()
            .list_log(ListLogRequest::default())
            .await
            .unwrap()
            .entries
            .into_iter()
            .map(|e| e.call.unwrap())
            .collect()
    }
}

fn get(path: &str) -> HttpRequest {
    HttpRequest {
        connection: "github".into(),
        method: "GET".into(),
        path: path.into(),
        ..HttpRequest::default()
    }
}

#[tokio::test]
async fn a_call_through_a_connection_is_answered_and_logged() {
    let world = World::new(Scripted::answering(vec![json(r#"{"full_name":"a/b"}"#)]));
    let answered = world
        .service()
        .call(get("/repos/a/b"), true, Some("claude"), None)
        .await
        .unwrap();
    assert_eq!(answered.status, 200);
    assert!(matches!(answered.body, Some(Answered::Json(_))));
    assert_eq!(answered.url, "https://api.github.com/repos/a/b");
    let sent = world.client.sent.lock().clone();
    assert!(
        sent[0]
            .headers
            .contains(&("authorization".into(), "Bearer ghp_secret".into()))
    );
    let log = world.log().await;
    assert_eq!(log.len(), 1);
    let entry = &log[0];
    assert_eq!(entry.outcome(), Outcome::Ok);
    assert_eq!(
        (
            entry.title.as_str(),
            entry.author.as_str(),
            entry.path.as_str(),
            entry.status
        ),
        ("GET api.github.com", "claude", "/repos/a/b", 200)
    );
    assert!(!format!("{log:?}").contains("ghp_secret"));
}

#[tokio::test]
async fn a_status_outside_ok_fails_as_its_kind_without_the_secret() {
    let world = World::new(Scripted::answering(vec![answer(
        404,
        &[],
        "no repo for token ghp_secret",
    )]));
    let failed = world
        .service()
        .call(get("/repos/a/x"), true, None, None)
        .await
        .unwrap_err();
    assert_eq!(failed.kind, ModuleErrorKind::NotFound);
    assert!(
        failed
            .message
            .starts_with("HTTP 404 Reason: no repo for token ***")
    );
    let entry = &world.log().await[0];
    assert_eq!(
        (entry.outcome(), entry.failure.as_str()),
        (Outcome::Failed, "not_found")
    );
}

#[tokio::test]
async fn a_redirect_elsewhere_goes_without_the_credentials() {
    let world = World::new(Scripted::answering(vec![
        answer(
            302,
            &[("Location", "https://objects.example.com/t.tgz")],
            "",
        ),
        answer(200, &[("Content-Type", "application/gzip")], "gz"),
    ]));
    let answered = world
        .service()
        .call(get("/repos/a/b/tarball"), true, None, None)
        .await
        .unwrap();
    assert_eq!(
        (answered.redirects, answered.url.as_str()),
        (1, "https://objects.example.com/t.tgz")
    );
    let sent = world.client.sent.lock().clone();
    assert!(!sent[1].headers.iter().any(|(n, _)| n == "authorization"));
}

#[tokio::test]
async fn more_than_five_redirects_fail() {
    let hop = || answer(302, &[("Location", "/again")], "");
    let world = World::new(Scripted::answering((0..6).map(|_| hop()).collect()));
    let failed = world
        .service()
        .call(get("/x"), true, None, None)
        .await
        .unwrap_err();
    assert_eq!(failed.kind, ModuleErrorKind::FailedPrecondition);
    assert_eq!(world.client.sent.lock().len(), 6);
}

#[tokio::test]
async fn a_body_that_does_not_fit_expect_fails() {
    let world = World::new(Scripted::answering(vec![json(r#"{"ok":false}"#)]));
    let mut request = get("/x");
    request.expect = Some(Schema {
        fields: vec![Wanted {
            name: "ts".into(),
            r#type: "string".into(),
            required: true,
            ..Wanted::default()
        }],
    });
    let failed = world
        .service()
        .call(request, true, None, None)
        .await
        .unwrap_err();
    assert_eq!(failed.kind, ModuleErrorKind::FailedPrecondition);
}

#[tokio::test]
async fn the_daily_limit_counts_what_the_log_holds() {
    let mut world = World::new(Scripted::answering(vec![json("{}"), json("{}")]));
    world.settings = settings(1);
    world
        .service()
        .call(get("/x"), true, None, None)
        .await
        .unwrap();
    let refused = world
        .service()
        .call(get("/x"), true, None, None)
        .await
        .unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::ResourceExhausted);
    // A module started again reads the day's count from its log.
    world.gates = Gates::default();
    let again = world
        .service()
        .call(get("/x"), true, None, None)
        .await
        .unwrap_err();
    assert_eq!(again.kind, ModuleErrorKind::ResourceExhausted);
    assert_eq!(world.client.sent.lock().len(), 1);
}

#[tokio::test]
async fn a_failed_send_is_logged_without_the_secret() {
    let world = World::new(Scripted::answering(vec![Err(ModuleError::new(
        ModuleErrorKind::DeadlineExceeded,
        "GET api.github.com?key=ghp_secret: timed out",
    ))]));
    let failed = world
        .service()
        .call(get("/x"), true, None, None)
        .await
        .unwrap_err();
    assert_eq!(failed.kind, ModuleErrorKind::DeadlineExceeded);
    assert!(!failed.message.contains("ghp_secret"));
    assert_eq!(world.log().await[0].failure, "deadline_exceeded");
}

#[tokio::test]
async fn the_test_of_a_connection_answers_any_status() {
    let world = World::new(Scripted::answering(vec![answer(401, &[], "")]));
    let tested = world.service().test("github", None).await.unwrap();
    assert_eq!(tested.status, 401);
    let listed = world.service().connections();
    assert_eq!(listed.connections[0].name, "github");
    assert!(listed.connections[0].secret_set);
}

#[tokio::test]
async fn presets_are_built_in_and_the_organisations() {
    let world = World::new(Scripted::default());
    let service = world.service();
    let mut mine = world.built_in[2].clone();
    mine.name = "my-repo".into();
    let saved = service
        .save_preset(SavePresetRequest {
            preset: Some(mine.clone()),
            version: 0,
        })
        .await
        .unwrap();
    assert!(!saved.built_in);
    let again = service
        .save_preset(SavePresetRequest {
            preset: Some(mine.clone()),
            version: 0,
        })
        .await
        .unwrap_err();
    assert_eq!(again.kind, ModuleErrorKind::Conflict);
    let changed = service
        .save_preset(SavePresetRequest {
            preset: Some(mine),
            version: saved.version,
        })
        .await
        .unwrap();
    assert_eq!(changed.version, saved.version + 1);
    let mut taken = world.built_in[0].clone();
    taken.description = "mine".into();
    let refused = service
        .save_preset(SavePresetRequest {
            preset: Some(taken),
            version: 0,
        })
        .await
        .unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::InvalidArgument);
    let github = service
        .list_presets(&ListPresetsRequest {
            service: "github".into(),
        })
        .await
        .unwrap();
    assert!(
        github
            .presets
            .iter()
            .any(|p| p.preset.as_ref().unwrap().name == "my-repo")
    );
    assert!(
        github
            .presets
            .iter()
            .all(|p| p.preset.as_ref().unwrap().service == "github")
    );
    assert!(
        service
            .get_preset("github-repo-get")
            .await
            .unwrap()
            .built_in
    );
    assert_eq!(
        service.get_preset("nope").await.unwrap_err().kind,
        ModuleErrorKind::NotFound
    );
    let stale = service
        .delete_preset("my-repo", saved.version)
        .await
        .unwrap_err();
    assert_eq!(stale.kind, ModuleErrorKind::Conflict);
    let built = service
        .delete_preset("github-repo-get", 0)
        .await
        .unwrap_err();
    assert_eq!(built.kind, ModuleErrorKind::InvalidArgument);
    service
        .delete_preset("my-repo", changed.version)
        .await
        .unwrap();
    assert_eq!(
        service.get_preset("my-repo").await.unwrap_err().kind,
        ModuleErrorKind::NotFound
    );
}

#[tokio::test]
async fn the_log_keeps_thirty_days() {
    let mut world = World::new(Scripted::answering(vec![json("{}")]));
    world
        .service()
        .call(get("/x"), true, None, None)
        .await
        .unwrap();
    world.clock = at("2026-12-01T09:00:00Z");
    assert_eq!(world.service().prune().await.unwrap(), 0);
    world.clock = at("2026-12-02T09:00:00Z");
    assert_eq!(world.service().prune().await.unwrap(), 1);
    assert!(world.log().await.is_empty());
}

mod refused;
