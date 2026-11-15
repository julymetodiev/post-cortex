// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

#[test]
fn the_modules_cross_the_event() {
    let available = ModuleAvailable {
        modules: vec!["tools-git".into(), "tools-fs".into()],
    };
    assert_eq!(
        ModuleAvailable::from_event(&available.to_event()).unwrap(),
        Some(available)
    );
    assert_eq!(
        ModuleAvailable::from_event(&Event::default()).unwrap(),
        None
    );
}
