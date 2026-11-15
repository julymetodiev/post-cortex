// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Calls refused before anything is sent, and how the log keeps them.

use super::*;

#[tokio::test]
async fn a_private_address_is_refused_before_anything_is_sent() {
    let world = World::new(Scripted::default());
    let request = HttpRequest {
        method: "GET".into(),
        url: "https://internal.example.com/metrics".into(),
        ..HttpRequest::default()
    };
    let refused = world
        .service()
        .call(request, true, None, None)
        .await
        .unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::Denied);
    assert!(world.client.sent.lock().is_empty());
    assert_eq!(world.log().await[0].outcome(), Outcome::Refused);
}

#[tokio::test]
async fn a_request_refused_before_its_address_is_known_is_logged_by_what_it_named() {
    // Staging 2026-11-08: an http_get without a method was refused, and its
    // entry, titled by the empty method, was not logged.
    let world = World::new(Scripted::default());
    let refused = |request: HttpRequest| async {
        world
            .service()
            .call(request, true, None, None)
            .await
            .unwrap_err()
            .message
    };
    let no_method = refused(HttpRequest {
        url: "https://example.com/a?token=secret".into(),
        ..HttpRequest::default()
    })
    .await;
    assert!(no_method.contains("log entry"), "{no_method}");
    let nothing = refused(HttpRequest::default()).await;
    assert!(nothing.contains("log entry"), "{nothing}");
    let titles: Vec<_> = world.log().await.into_iter().map(|c| c.title).collect();
    assert_eq!(titles.len(), 2);
    assert!(
        titles.contains(&"(no method) example.com".to_string()),
        "{titles:?}"
    );
    assert!(
        titles.contains(&"(no method) (no address)".to_string()),
        "{titles:?}"
    );
    assert!(!format!("{titles:?}").contains("secret"));
}
