// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

fn backend_down() -> Result<()> {
    Err(AdapterError::Io(std::io::Error::other(
        "connection refused",
    )))
}

#[tokio::test]
async fn backend_failures_in_a_row_open_the_breaker() {
    let breaker = Breaker::default();
    for _ in 0..FAILURES {
        assert!(breaker.guard(async { backend_down() }).await.is_err());
    }
    let mut reached = false;
    let refused = breaker
        .guard(async {
            reached = true;
            Ok(())
        })
        .await;
    assert!(matches!(refused, Err(PortError::Unavailable(_))));
    assert!(!reached, "an open breaker does not reach the backend");
}

#[tokio::test]
async fn typed_answers_and_successes_do_not_open_it() {
    let breaker = Breaker::default();
    for _ in 0..FAILURES * 2 {
        let missing = breaker
            .guard(async {
                Err::<(), _>(AdapterError::Port(post_cortex_ports::error::not_found(
                    "row", "x",
                )))
            })
            .await;
        assert!(missing.is_err_and(|e| e.is_not_found()));
    }
    for _ in 0..FAILURES - 1 {
        assert!(breaker.guard(async { backend_down() }).await.is_err());
    }
    assert!(breaker.guard(async { Ok(()) }).await.is_ok());
    for _ in 0..FAILURES - 1 {
        assert!(breaker.guard(async { backend_down() }).await.is_err());
    }
    assert!(
        breaker.guard(async { Ok(()) }).await.is_ok(),
        "a success resets the count"
    );
}
