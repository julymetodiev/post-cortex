// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use std::time::Duration;

/// A dropped call sets the flag its running job checks, and the permit
/// comes back once the job returns.
#[tokio::test]
async fn a_dropped_call_abandons_its_job() {
    let slots = Arc::new(Semaphore::new(1));
    let steps = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = Arc::clone(&steps);
    let call = run(&slots, move |abandoned| {
        for _ in 0..100 {
            abandoned.check()?;
            counted.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), call)
            .await
            .is_err()
    );
    let permit = tokio::time::timeout(Duration::from_secs(1), slots.acquire())
        .await
        .unwrap()
        .unwrap();
    drop(permit);
    assert!(steps.load(Ordering::SeqCst) < 20);
}

#[tokio::test]
async fn a_closed_semaphore_is_inactive() {
    let slots = Arc::new(Semaphore::new(1));
    slots.close();
    let err = run(&slots, |_| Ok(())).await.unwrap_err();
    assert!(matches!(err, Error::Inactive), "{err}");
}
