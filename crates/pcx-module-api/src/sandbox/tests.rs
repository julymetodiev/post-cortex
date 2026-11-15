// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

#[tokio::test]
async fn a_run_collects_the_tail_of_each_stream_and_its_exit() {
    let (tx, rx) = mpsc::channel(8);
    for event in [
        ExecEvent::Stdout(b"hello ".to_vec()),
        ExecEvent::Stderr(b"warn".to_vec()),
        ExecEvent::Stdout(b"world".to_vec()),
        ExecEvent::Exit(ExecExit::Code(0)),
    ] {
        tx.send(event).await.unwrap();
    }
    let out = ExecRun::new(rx).collect(5).await.unwrap();
    assert_eq!(out.stdout, b"world");
    assert_eq!(out.stderr, b"warn");
    assert!(out.truncated);
    assert_eq!(out.exit, ExecExit::Code(0));
}

#[tokio::test]
async fn a_run_without_an_exit_is_unavailable() {
    let (tx, rx) = mpsc::channel(1);
    tx.send(ExecEvent::Stdout(b"x".to_vec())).await.unwrap();
    drop(tx);
    let error = ExecRun::new(rx).collect(10).await.unwrap_err();
    assert_eq!(error.kind, crate::ModuleErrorKind::Unavailable);
}
