// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use post_cortex_proto::pb::Attachment;

fn file(path: &str, text: &str) -> File {
    File {
        path: path.into(),
        text: text.into(),
    }
}

fn module(sha: &str, used: &[&str]) -> DynamicModule {
    DynamicModule {
        id: "greet".into(),
        version: "1".into(),
        language: Language::Python.into(),
        files: Some(Attachment {
            sha256: sha.into(),
            ..Default::default()
        }),
        grants: Some(Grants {
            uses: used.iter().map(|u| (*u).to_string()).collect(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[test]
fn a_bundle_holds_only_what_it_may() {
    let good = [file("module.py", "print()"), file("proto/a.proto", "")];
    check_files(Language::Python, &good).unwrap();
    for (files, says) in [
        (vec![file("other.py", "")], "module.py"),
        (vec![file("module.py", ""), file("../x", "")], "beneath"),
        (vec![file("module.py", ""), file("/etc/x", "")], "beneath"),
        (vec![file("module.py", ""), file("module.py", "")], "twice"),
        (
            vec![file("module.py", ""), file("a_pb2.py", "")],
            "generated",
        ),
        (
            vec![file("module.py", "import pcx.module.v1")],
            "pcx.module.v1",
        ),
    ] {
        let refused = check_files(Language::Python, &files).unwrap_err();
        assert!(refused.contains(says), "{refused}");
    }
    assert!(check_files(Language::Unspecified, &good).is_err());
    assert!(check_id("my-module-2").is_ok());
    for bad in ["", "dynamic", "Up", "a/b"] {
        assert!(check_id(bad).is_err(), "{bad}");
    }
}

#[test]
fn the_same_files_give_the_same_hash_and_read_back() {
    let a = [file("b.py", "2"), file("a.py", "1")];
    let b = [file("a.py", "1"), file("b.py", "2")];
    let tar = tar_of(&a).unwrap();
    assert_eq!(sha256(&tar), sha256(&tar_of(&b).unwrap()));
    let read = files_of(&tar).unwrap();
    assert_eq!(read[0].path, "a.py");
    assert_eq!(read[1].text, "2");
}

#[test]
fn never_uses_every_module_or_itself() {
    for never in ["*", "dynamic"] {
        let grants = Grants {
            uses: vec![never.to_string()],
            ..Default::default()
        };
        assert!(check_grants(&grants).is_err());
    }
    let grants = Grants {
        uses: vec!["http".into(), "mail".into()],
        ..Default::default()
    };
    check_grants(&grants).unwrap();
    let reachable = BTreeSet::from(["http".to_string()]);
    assert_eq!(beyond_author(&grants, &reachable), vec!["mail".to_string()]);
}

#[test]
fn what_a_person_approved_is_the_modules_and_new_code_asks_again() {
    let mut said = module("a", &["http"]);
    said.approved = Some(Approved::default());
    let new = kept(said, None);
    assert!(new.approved.is_none(), "a request does not approve");
    assert!(asks(&new));
    assert!(question(&new).contains("greet 1"));

    let waiting = asked(new, "t", "d");
    assert!(!asks(&waiting), "asked once for these files");
    assert!(approve(waiting.clone(), "other", "ana").is_none());
    let ok = approve(waiting, "d", "ana").unwrap();
    assert!(approved(&ok));
    assert!(ok.asked.is_none());
    assert_eq!(ok.approved.as_ref().unwrap().approved_by, "ana");

    // New code, however small, and wider grants ask again.
    let changed = kept(module("b", &["http"]), Some(&ok));
    assert!(!approved(&changed) && asks(&changed));
    assert!(question(&changed).contains("change from"));
    let wider = kept(module("a", &["http", "mail"]), Some(&ok));
    assert!(!approved(&wider));
    // Narrower grants on the same code need nothing.
    let narrower = kept(module("a", &[]), Some(&ok));
    assert!(approved(&narrower));

    // A decision on files replaced meanwhile approves nothing.
    let first = asked(module("a", &[]), "t", "d");
    let replaced = kept(module("b", &[]), Some(&first));
    assert!(approve(replaced, "d", "ana").is_none());
}

#[test]
fn a_difference_names_what_changed() {
    let before = [file("a.py", "one\n")];
    let after = [file("a.py", "two\n"), file("b.py", "new\n")];
    let shown = difference(&before, &after);
    assert!(shown.contains("-one") && shown.contains("+two") && shown.contains("+new"));
    assert!(shown.starts_with("```diff\n"), "{shown}");
    assert_eq!(difference(&before, &before), NO_DIFFERENCE);
}

/// What the skill shows an agent installs: its example passes the rules.
#[test]
fn the_skills_example_is_a_bundle() {
    let skill = include_str!("../../../pcx-skills/library/module-builder.md");
    let (_, after) = skill.split_once("```json\n").unwrap();
    let (example, _) = after.split_once("\n```").unwrap();
    let request: serde_json::Value = serde_json::from_str(example).unwrap();
    let files: Vec<File> = request["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| file(f["path"].as_str().unwrap(), f["text"].as_str().unwrap()))
        .collect();
    check_files(Language::Python, &files).unwrap();
    check_id(request["id"].as_str().unwrap()).unwrap();
    tar_of(&files).unwrap();
}

#[test]
fn the_rule_is_the_scripts() {
    let script = include_str!("../../../../scripts/check-architecture.sh");
    for word in FOREIGN {
        assert!(
            script.contains(&word.replace('.', "\\.")),
            "{word} left foreign_modules"
        );
    }
}

#[test]
fn a_line_of_backticks_does_not_close_its_block() {
    let shown = fenced("md", "before\n```\ninside\n```\n");
    assert!(shown.starts_with("````md\n"), "{shown}");
    assert!(shown.ends_with("\n````"), "{shown}");
    assert_eq!(fenced("py", "print(1)"), "```py\nprint(1)\n```");
}

#[test]
fn a_changed_file_replaces_and_a_new_one_joins_in_order() {
    let file = |path: &str, text: &str| File {
        path: path.into(),
        text: text.into(),
    };
    let files = vec![file("module.py", "1"), file("tests/t.py", "2")];
    let changed = with_file(files.clone(), "module.py", "3");
    assert_eq!(changed[0].text, "3");
    assert_eq!(changed.len(), 2);
    let added = with_file(files, "lock.txt", "4");
    assert_eq!(added[0].path, "lock.txt");
    assert_eq!(added.len(), 3);
}

#[test]
fn a_bundle_leaves_out_what_is_hidden_and_a_package_cache() {
    assert!(bundled("module.ts"));
    assert!(bundled("src/parse.ts"));
    assert!(!bundled(".deno/npm/x.js"));
    assert!(!bundled("src/.cache"));
    assert!(!bundled("node_modules/a/index.js"));
}

#[test]
fn a_path_is_named_beneath_its_directory() {
    assert_eq!(beneath("/t/m", "/t/m/src/a.ts"), Some("src/a.ts"));
    assert_eq!(beneath("/t/m/", "/t/m/a.ts"), Some("a.ts"));
    assert_eq!(beneath("/t/m", "/t/mm/a.ts"), None);
}
