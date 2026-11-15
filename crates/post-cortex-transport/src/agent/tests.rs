// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A client's turn (W25): its author and workspace are the headers', who
//! asks is its author, whatever the request says.

use std::collections::HashMap;

use super::*;

#[tokio::test]
async fn a_clients_turn_works_in_the_headers_workspace_as_the_headers_author() {
    let host = Arc::new(
        Host::start(Vec::new(), &HashMap::new(), &HashMap::new())
            .await
            .unwrap(),
    );
    let service = AgentService::new(host);
    let mut request = Request::new(wire::RunTurnRequest {
        session_id: "s1".into(),
        prompt: "Which tasks?".into(),
        asker: "someone else".into(),
        speaker: "chief".into(),
        ..wire::RunTurnRequest::default()
    });
    request
        .metadata_mut()
        .insert(AUTHOR_HEADER, "julius".parse().unwrap());
    request
        .metadata_mut()
        .insert(metadata::WORKSPACE, "w1".parse().unwrap());

    let scope = service.scope(&request, CancellationToken::new()).unwrap();
    assert_eq!(scope.author.as_deref(), Some("julius"));
    assert_eq!(scope.workspace_id.as_deref(), Some("w1"));
    assert_eq!(scope.session_id, None);
    let asked = service.asked(request.into_inner());
    assert_eq!(asked.asker, None, "who asks is the header's author");
    assert_eq!(asked.speaker.as_deref(), Some("chief"));

    let bare = Request::new(wire::RunTurnRequest::default());
    let scope = service.scope(&bare, CancellationToken::new()).unwrap();
    assert_eq!(scope.workspace_id, None, "asked from no workspace");
}
