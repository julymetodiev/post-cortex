// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

#[test]
fn a_task_works_in_a_worktree_of_its_own_on_a_branch_named_for_it() {
    let (path, branch) = place(
        "7d9F2a1b-0000-4000-8000-000000000001",
        "Fix: svc-music crash-loops!",
    );
    assert_eq!(path, ".worktrees/7d9f2a1b");
    assert_eq!(branch, "task/7d9f2a1b-fix-svc-music-crash-loops");
    let (_, bare) = place("t1", "!!!");
    assert_eq!(bare, "task/t1");
    let (_, long) = place("t1", &"word ".repeat(30));
    assert!(long.len() <= "task/t1-".len() + 40, "{long}");
    assert!(told(".wt/t1", "task/t1").contains("on its branch task/t1"));
}
