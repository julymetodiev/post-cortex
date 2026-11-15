// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

use pcx_host::RootGrant;

fn grants(roots: &[(&str, bool)], network: bool, spawn: bool) -> ModuleGrants {
    ModuleGrants {
        roots: roots
            .iter()
            .map(|(path, write)| RootGrant {
                path: (*path).into(),
                write: *write,
            })
            .collect(),
        network,
        spawn,
    }
}

#[test]
fn an_entrys_grants_change_and_the_rest_of_the_file_stays() {
    let text = r#"# the daemon
port = 3737

# git, for the board
[[builtins]]
id = "tools-git"
grants = ["write_memory", "filesystem"]
filesystem = [{ path = "/old", write = true }]
config = { author = "pcx" }

[[builtins]]
id = "board"
"#;
    let changed = with_grants(
        text,
        "tools-git",
        &grants(&[("/repo", true), ("/docs", false)], false, true),
        true,
    )
    .unwrap();
    assert_eq!(
        changed,
        r#"# the daemon
port = 3737

# git, for the board
[[builtins]]
id = "tools-git"
grants = ["write_memory", "filesystem", "spawn"]
filesystem = [{ path = "/repo", write = true }, { path = "/docs", write = false }]
config = { author = "pcx" }

[[builtins]]
id = "board"
"#
    );
    let none = with_grants(&changed, "tools-git", &grants(&[], false, false), true).unwrap();
    assert!(
        none.contains("grants = [\"write_memory\"]\nconfig"),
        "{none}"
    );
}

#[test]
fn a_builtin_the_file_does_not_name_is_added_and_an_unknown_module_refused() {
    let added = with_grants(
        "port = 3737\n",
        "tools-shell",
        &grants(&[("/repo", true)], true, true),
        true,
    )
    .unwrap();
    assert_eq!(
        added,
        "port = 3737\n\n[[builtins]]\nid = \"tools-shell\"\ngrants = [\"filesystem\", \"spawn\", \"network\"]\nfilesystem = [{ path = \"/repo\", write = true }]\n"
    );
    let inline = with_grants(
        "builtins = [{ id = \"board\" }]\n",
        "tools-fs",
        &grants(&[], false, false),
        true,
    )
    .unwrap();
    assert!(
        inline.contains("{ id = \"tools-fs\", grants = [] }"),
        "{inline}"
    );
    assert!(matches!(
        with_grants("", "greet", &grants(&[], false, false), false),
        Err(SettingsError::Unknown { .. })
    ));
}

#[test]
fn a_process_modules_entry_is_granted_too() {
    let text = "[[modules]]\nid = \"greet\"\ncommand = \"/bin/greet\"\nsha256 = \"ab\"\n";
    let changed = with_grants(text, "greet", &grants(&[], true, false), false).unwrap();
    assert!(
        changed.ends_with("sha256 = \"ab\"\ngrants = [\"network\"]\n"),
        "{changed}"
    );
}
