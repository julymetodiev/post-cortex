// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A connection to SurrealDB reset under the client (the prod outage of
//! 2026-11-08: a pf load reset pcx-cloud's open connection, and its calls
//! hung until it was restarted). Needs a SurrealDB at
//! `PCX_SURREAL_RESET_URL` (host:port, root/root):
//! `surreal start --user root --pass root --bind 127.0.0.1:18000 memory`.
#![cfg(feature = "surrealdb-storage")]

use std::sync::Arc;
use std::time::Duration;

use post_cortex_ports::WorkspaceStore;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

/// A TCP proxy to `upstream` that resets every connection it carries when
/// `reset` is notified: each side gets an RST, as from a firewall.
async fn proxy(upstream: String, reset: Arc<Notify>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (mut client, _) = listener.accept().await.unwrap();
            let mut server = TcpStream::connect(&upstream).await.unwrap();
            let reset = Arc::clone(&reset);
            tokio::spawn(async move {
                tokio::select! {
                    _ = tokio::io::copy_bidirectional(&mut client, &mut server) => {}
                    () = reset.notified() => {
                        client.set_zero_linger().unwrap();
                        server.set_zero_linger().unwrap();
                    }
                }
            });
        }
    });
    port
}

#[tokio::test]
#[ignore = "needs a SurrealDB at PCX_SURREAL_RESET_URL"]
async fn a_reset_connection_answers_again() {
    let upstream = std::env::var("PCX_SURREAL_RESET_URL").unwrap();
    let reset = Arc::new(Notify::new());
    let port = proxy(upstream, Arc::clone(&reset)).await;
    let storage = post_cortex_storage::SurrealDBStorage::new(
        &format!("ws://127.0.0.1:{port}"),
        Some("root"),
        Some("root"),
        Some("reset_test"),
        Some("main"),
    )
    .await
    .unwrap();
    let ask =
        || async { tokio::time::timeout(Duration::from_secs(10), storage.list_workspaces()).await };
    assert!(matches!(ask().await, Ok(Ok(_))), "before the reset");
    reset.notify_waiters();
    let mut answers = Vec::new();
    for _ in 0..4 {
        let answer = ask().await;
        let answered = matches!(answer, Ok(Ok(_)));
        answers.push(format!("{answer:?}"));
        if answered {
            return;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    panic!("no answer after the reset: {answers:#?}");
}

/// A namespace user's connection, signed in with a token of `token_secs`,
/// reset after `idle` (pcx-cloud's control plane was signed in 11 h before
/// the reset; a token lasts 1 h by default): what its next queries answer.
async fn namespace_user_after_reset(token_secs: u64, idle: Duration) -> Vec<String> {
    use surrealdb::opt::auth::{Namespace, Root};
    let upstream = std::env::var("PCX_SURREAL_RESET_URL").unwrap();
    let root = surrealdb::engine::any::connect(format!("ws://{upstream}"))
        .await
        .unwrap();
    root.signin(Root {
        username: "root".into(),
        password: "root".into(),
    })
    .await
    .unwrap();
    let ns = format!("reset_ns_{token_secs}");
    root.query(format!(
        "DEFINE NAMESPACE OVERWRITE {ns}; USE NS {ns}; \
         DEFINE USER OVERWRITE cp ON NAMESPACE PASSWORD 'pw' ROLES EDITOR \
         DURATION FOR TOKEN {token_secs}s, FOR SESSION 1d;"
    ))
    .await
    .unwrap()
    .check()
    .unwrap();
    let reset = Arc::new(Notify::new());
    let port = proxy(upstream, Arc::clone(&reset)).await;
    let db = surrealdb::engine::any::connect(format!("ws://127.0.0.1:{port}"))
        .await
        .unwrap();
    db.signin(Namespace {
        namespace: ns.clone(),
        username: "cp".into(),
        password: "pw".into(),
    })
    .await
    .unwrap();
    db.use_ns(&ns).use_db("main").await.unwrap();
    let ask = || async {
        match tokio::time::timeout(Duration::from_secs(10), db.query("RETURN 1;")).await {
            Ok(Ok(response)) => match response.check() {
                Ok(_) => "answered".to_string(),
                Err(e) => format!("error: {e}"),
            },
            Ok(Err(e)) => format!("error: {e}"),
            Err(_) => "hung 10 s".to_string(),
        }
    };
    let mut answers = vec![format!("before: {}", ask().await)];
    tokio::time::sleep(idle).await;
    reset.notify_waiters();
    for i in 0..3 {
        answers.push(format!("after {i}: {}", ask().await));
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    answers
}

#[tokio::test]
#[ignore = "needs a SurrealDB at PCX_SURREAL_RESET_URL"]
async fn a_namespace_user_answers_after_a_reset() {
    let fresh = namespace_user_after_reset(3600, Duration::from_secs(1)).await;
    let expired = namespace_user_after_reset(2, Duration::from_secs(4)).await;
    eprintln!("token alive: {fresh:#?}\ntoken expired: {expired:#?}");
    assert!(
        fresh.iter().skip(1).any(|a| a == "after 2: answered"),
        "{fresh:#?}"
    );
    assert!(
        expired.iter().skip(1).any(|a| a == "after 2: answered"),
        "{expired:#?}"
    );
}

/// What a namespace user's connection answers when it is reset while
/// `in_flight` queries (each a sleep of `query_ms`) are still unanswered,
/// as on the control plane, which every API call queries.
async fn reset_in_flight(in_flight: usize, query_ms: u64) -> Vec<String> {
    use surrealdb::opt::auth::{Namespace, Root};
    let upstream = std::env::var("PCX_SURREAL_RESET_URL").unwrap();
    let root = surrealdb::engine::any::connect(format!("ws://{upstream}"))
        .await
        .unwrap();
    root.signin(Root {
        username: "root".into(),
        password: "root".into(),
    })
    .await
    .unwrap();
    let ns = "reset_ns_flight";
    root.query(format!(
        "DEFINE NAMESPACE OVERWRITE {ns}; USE NS {ns}; \
         DEFINE USER OVERWRITE cp ON NAMESPACE PASSWORD 'pw' ROLES EDITOR;"
    ))
    .await
    .unwrap()
    .check()
    .unwrap();
    let reset = Arc::new(Notify::new());
    let port = proxy(upstream, Arc::clone(&reset)).await;
    let db = surrealdb::engine::any::connect(format!("ws://127.0.0.1:{port}"))
        .await
        .unwrap();
    db.signin(Namespace {
        namespace: ns.into(),
        username: "cp".into(),
        password: "pw".into(),
    })
    .await
    .unwrap();
    db.use_ns(ns).use_db("main").await.unwrap();
    let flying: Vec<_> = (0..in_flight)
        .map(|_| {
            let db = db.clone();
            tokio::spawn(async move {
                let asked = db.query(format!("SLEEP {query_ms}ms; RETURN 1;"));
                match tokio::time::timeout(Duration::from_secs(15), asked).await {
                    Ok(Ok(_)) => "answered".to_string(),
                    Ok(Err(e)) => format!("error: {e}"),
                    Err(_) => "hung 15 s".to_string(),
                }
            })
        })
        .collect();
    tokio::time::sleep(Duration::from_millis(200)).await;
    reset.notify_waiters();
    let mut answers = Vec::new();
    for f in flying {
        answers.push(format!("in flight: {}", f.await.unwrap()));
    }
    for i in 0..3 {
        let asked = db.query("RETURN 1;");
        let answer = match tokio::time::timeout(Duration::from_secs(10), asked).await {
            Ok(Ok(_)) => "answered".to_string(),
            Ok(Err(e)) => format!("error: {e}"),
            Err(_) => "hung 10 s".to_string(),
        };
        answers.push(format!("after {i}: {answer}"));
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    answers
}

#[tokio::test]
#[ignore = "needs a SurrealDB at PCX_SURREAL_RESET_URL"]
async fn a_connection_reset_with_queries_in_flight_answers_again() {
    let answers = reset_in_flight(8, 2000).await;
    eprintln!("reset in flight: {answers:#?}");
    assert!(!answers.iter().any(|a| a.contains("hung")), "{answers:#?}");
    assert!(
        answers.iter().any(|a| a == "after 2: answered"),
        "{answers:#?}"
    );
}

/// A TCP proxy to `upstream` that, once `drop` is notified, forwards nothing
/// more and answers nothing, keeping every connection open: packets lost
/// without a reset, as from a firewall that drops them.
async fn blackhole(upstream: String, drop: Arc<Notify>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (mut client, _) = listener.accept().await.unwrap();
            let mut server = TcpStream::connect(&upstream).await.unwrap();
            let drop = Arc::clone(&drop);
            tokio::spawn(async move {
                tokio::select! {
                    _ = tokio::io::copy_bidirectional(&mut client, &mut server) => {}
                    () = drop.notified() => {
                        // Held, unread and unanswered, until the test ends.
                        std::future::pending::<()>().await;
                    }
                }
            });
        }
    });
    port
}

/// The SDK alone hangs on such a connection forever (it pings but never
/// checks an answer): every query below would hang.
#[tokio::test]
#[ignore = "needs a SurrealDB at PCX_SURREAL_RESET_URL"]
async fn a_connection_whose_packets_are_lost_is_replaced() {
    let upstream = std::env::var("PCX_SURREAL_RESET_URL").unwrap();
    let drop = Arc::new(Notify::new());
    let port = blackhole(upstream, Arc::clone(&drop)).await;
    let storage = post_cortex_storage::SurrealDBStorage::new(
        &format!("ws://127.0.0.1:{port}"),
        Some("root"),
        Some("root"),
        Some("reset_lost"),
        Some("main"),
    )
    .await
    .unwrap();
    assert!(storage.list_workspaces().await.is_ok(), "before");
    drop.notify_waiters();
    let replaced_within = post_cortex_storage::surrealdb_storage::PROBE
        * (post_cortex_storage::surrealdb_storage::MISSES + 1)
        + post_cortex_storage::surrealdb_storage::ANSWER * 2;
    let started = std::time::Instant::now();
    let mut answers = Vec::new();
    while started.elapsed() < replaced_within + Duration::from_secs(10) {
        let answer = tokio::time::timeout(Duration::from_secs(5), storage.list_workspaces()).await;
        let answered = matches!(answer, Ok(Ok(_)));
        answers.push(format!(
            "{:?}: {}",
            started.elapsed(),
            if answered { "answered" } else { "no answer" }
        ));
        if answered {
            assert!(
                started.elapsed() <= replaced_within + Duration::from_secs(5),
                "{answers:#?}"
            );
            return;
        }
    }
    panic!("never answered again: {answers:#?}");
}

/// The SDK's ERROR events (target `surrealdb...`) so far.
static SDK_ERRORS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Counts the SDK's ERROR events into [`SDK_ERRORS`].
struct SdkErrors;

impl tracing::Subscriber for SdkErrors {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        *metadata.level() == tracing::Level::ERROR
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        if event.metadata().target().starts_with("surrealdb") {
            SDK_ERRORS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

/// Forwards `from` to `to`, holding what it reads while `held` is true and
/// sending it on once it is false again: packets lost for a while, then
/// retransmitted.
async fn pump(
    mut from: tokio::net::tcp::OwnedReadHalf,
    mut to: tokio::net::tcp::OwnedWriteHalf,
    mut held: tokio::sync::watch::Receiver<bool>,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buf = vec![0; 64 * 1024];
    loop {
        let n = match from.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        while *held.borrow_and_update() {
            if held.changed().await.is_err() {
                return;
            }
        }
        if to.write_all(&buf[..n]).await.is_err() {
            return;
        }
    }
}

/// The holds of a [`held_proxy`]'s connections, one per connection opened.
type Holds = Arc<std::sync::Mutex<Vec<tokio::sync::watch::Sender<bool>>>>;

/// A TCP proxy to `upstream` whose connections each hold their packets
/// while their hold is true; a connection opens unheld.
async fn held_proxy(upstream: String) -> (u16, Holds) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let holds = Holds::default();
    let opened = Arc::clone(&holds);
    tokio::spawn(async move {
        loop {
            let (client, _) = listener.accept().await.unwrap();
            let server = TcpStream::connect(&upstream).await.unwrap();
            let (hold, held) = tokio::sync::watch::channel(false);
            opened.lock().unwrap().push(hold);
            let (client_read, client_write) = client.into_split();
            let (server_read, server_write) = server.into_split();
            tokio::spawn(pump(client_read, server_write, held.clone()));
            tokio::spawn(pump(server_read, client_write, held));
        }
    });
    (port, holds)
}

/// When a replaced connection's packets come back, every answer finds its
/// asker (staging 2026-11-08: a call in flight kept the old connection
/// open, and each probe given up on logged the SDK's ERROR "Failed to send
/// query results to channel" when its answer came 105 s later).
#[test]
#[ignore = "needs a SurrealDB at PCX_SURREAL_RESET_URL"]
fn a_replaced_connections_late_answers_find_their_askers() {
    tracing::subscriber::set_global_default(SdkErrors).unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let upstream = std::env::var("PCX_SURREAL_RESET_URL").unwrap();
        let (port, holds) = held_proxy(upstream).await;
        let storage = Arc::new(
            post_cortex_storage::SurrealDBStorage::new(
                &format!("ws://127.0.0.1:{port}"),
                Some("root"),
                Some("root"),
                Some("reset_late"),
                Some("main"),
            )
            .await
            .unwrap(),
        );
        assert!(storage.list_workspaces().await.is_ok(), "before");
        let hold = |held: bool| {
            for connection in holds.lock().unwrap().iter() {
                connection.send_replace(held);
            }
        };
        hold(true);
        let in_flight = {
            let storage = Arc::clone(&storage);
            tokio::spawn(async move { storage.list_workspaces().await.is_ok() })
        };
        let replaced_within = post_cortex_storage::surrealdb_storage::PROBE
            * (post_cortex_storage::surrealdb_storage::MISSES + 1)
            + post_cortex_storage::surrealdb_storage::ANSWER * 2;
        tokio::time::sleep(replaced_within).await;
        let answer = tokio::time::timeout(Duration::from_secs(5), storage.list_workspaces()).await;
        assert!(matches!(answer, Ok(Ok(_))), "the new connection answers");
        hold(false);
        assert!(
            in_flight.await.unwrap(),
            "the call in flight answers once its packets come"
        );
        tokio::time::sleep(Duration::from_secs(2)).await;
    });
    let logged = SDK_ERRORS.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(logged, 0, "the SDK logged {logged} ERROR events");
}
