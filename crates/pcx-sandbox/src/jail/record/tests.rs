// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

/// The jail every case records.
const JAIL: &str = "pcx-test";

#[test]
fn undo_takes_the_steps_back_in_reverse() {
    let base = tempfile::tempdir().unwrap();
    let mut record = Record::fresh(base.path(), JAIL).unwrap();
    let dir = base.path().join("root");
    let file = dir.join("hints");
    record.note(Step::Dir(dir.clone())).unwrap();
    std::fs::create_dir(&dir).unwrap();
    record.note(Step::File(file.clone())).unwrap();
    std::fs::write(&file, "").unwrap();
    // Never mounted: undoing it is no error.
    record.note(Step::Mount(file.clone())).unwrap();
    drop(record);
    undone(base.path(), JAIL).unwrap();
    assert!(!dir.exists());
    assert_eq!(std::fs::read_dir(base.path()).unwrap().count(), 0);
    // Nothing recorded: nothing to undo.
    undone(base.path(), JAIL).unwrap();
}

#[test]
fn a_directory_not_empty_is_never_removed() {
    let base = tempfile::tempdir().unwrap();
    let mut record = Record::fresh(base.path(), JAIL).unwrap();
    let dir = base.path().join("root");
    record.note(Step::Dir(dir.clone())).unwrap();
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("kept"), "a person's file").unwrap();
    assert!(undone(base.path(), JAIL).is_err());
    assert!(dir.join("kept").exists());
    // The record stays for the jail's next helper.
    assert!(base.path().join(format!("{JAIL}.{EXTENSION}")).exists());
}

#[test]
fn a_fresh_record_undoes_what_a_killed_helper_left() {
    let base = tempfile::tempdir().unwrap();
    let dir = base.path().join("left");
    std::fs::create_dir(&dir).unwrap();
    let line = serde_json::to_string(&Step::Dir(dir.clone())).unwrap();
    // The helper was killed while writing its next step.
    let earlier = base.path().join(format!("{JAIL}.{EXTENSION}"));
    std::fs::write(&earlier, format!("{line}\n{{\"Mou")).unwrap();
    let record = Record::fresh(base.path(), JAIL).unwrap();
    assert!(!dir.exists());
    assert_eq!(std::fs::read_to_string(&earlier).unwrap(), "", "{record:?}");
}

#[test]
fn a_broken_step_before_the_last_is_an_error() {
    let base = tempfile::tempdir().unwrap();
    let earlier = base.path().join(format!("{JAIL}.{EXTENSION}"));
    std::fs::write(&earlier, "not a step\n{\"Dir\":\"/nowhere\"}\n").unwrap();
    assert!(Record::fresh(base.path(), JAIL).is_err());
    assert!(earlier.exists());
}

#[test]
fn a_mount_point_is_found_as_the_host_or_the_jail_names_it() {
    assert_eq!(unescaped(r"/a\040b/c"), "/a b/c");
    assert_eq!(unescaped(r"/plain\x"), r"/plain\x");
    let mounted = vec![
        PathBuf::from("/opt/pot/jails/p/m/var/run/pcx-sandbox/7/usr"),
        PathBuf::from("/var/run/pcx-sandbox/7/var/run/ld-elf.so.hints"),
    ];
    assert!(is_mounted(
        &mounted,
        Path::new("/var/run/pcx-sandbox/7/usr")
    ));
    assert!(is_mounted(
        &mounted,
        Path::new("/var/run/pcx-sandbox/7/var/run/ld-elf.so.hints")
    ));
    assert!(!is_mounted(
        &mounted,
        Path::new("/var/run/pcx-sandbox/7/bin")
    ));
    assert!(!is_mounted(
        &mounted,
        Path::new("/var/run/pcx-sandbox/17/usr")
    ));
}
