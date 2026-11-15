// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A bundle's lock file read by its rules (docs/modules/dynamic.md,
//! prepared): each line one version with its hashes, and a native package
//! only at the version every module shares. Pure.

/// A package of a lock file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    /// Its name, in the index's normal form.
    pub name: String,
    /// Its version.
    pub version: String,
    /// Its line, as `pip` reads it.
    pub line: String,
}

/// A name in the index's normal form: lowercase, runs of `-`, `_` and `.`
/// one `-`.
fn normal(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if matches!(c, '-' | '_' | '.') {
            if !out.ends_with('-') {
                out.push('-');
            }
        } else {
            out.push(c.to_ascii_lowercase());
        }
    }
    out
}

/// The name and version of `requirement`, `name[extras]==version ; marker`.
fn pinned(requirement: &str) -> Option<(String, String)> {
    let plain = requirement.split(';').next()?.trim();
    let (name, version) = plain.split_once("==")?;
    let name = name.split('[').next()?.trim();
    let version = version.trim();
    let exact = !name.is_empty()
        && !version.is_empty()
        && !version.contains([',', '*', ' ', '<', '>', '=', '!', '~']);
    exact.then(|| (normal(name), version.to_string()))
}

/// The pins of the list `native`, each `name==version`.
fn native_pins(native: &str) -> impl Iterator<Item = (String, String)> + '_ {
    native
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(pinned)
}

/// The version every module shares of the native package `name`.
#[must_use]
pub fn native_version(native: &str, name: &str) -> Option<String> {
    native_pins(native)
        .find(|(n, _)| n == name)
        .map(|(_, version)| version)
}

/// The pins of the lock file `lock`.
///
/// # Errors
/// What is wrong, naming the line: not one version, no hash, or a native
/// package at another version than `native` names.
pub fn pins(lock: &str, native: &str) -> Result<Vec<Pin>, String> {
    let joined = lock.replace("\\\n", " ");
    let mut found = Vec::new();
    for raw in joined.lines() {
        let line = raw
            .split_once(" #")
            .map_or(raw, |(before, _)| before)
            .trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let requirement = line
            .split_once(" --")
            .map_or(line, |(before, _)| before)
            .trim();
        let (name, version) = pinned(requirement)
            .ok_or_else(|| format!("{requirement}: a lock file pins one version (==)"))?;
        if !line.contains("--hash=sha256:") {
            return Err(format!(
                "{requirement}: a lock file names its hashes (--generate-hashes)"
            ));
        }
        if let Some(shared) = native_version(native, &name)
            && shared != version
        {
            return Err(format!(
                "{name} {version}: modules share {name} {shared}; lock that version"
            ));
        }
        found.push(Pin {
            name,
            version,
            line: line.to_string(),
        });
    }
    Ok(found)
}

/// A file that imports each package a `deno.lock` of `text` names.
pub fn locked_imports(text: &[u8]) -> Result<String, String> {
    let lock: serde_json::Value =
        serde_json::from_slice(text).map_err(|e| format!("deno.lock is not JSON: {e}"))?;
    let names = lock
        .get("specifiers")
        .and_then(serde_json::Value::as_object)
        .map(|specifiers| specifiers.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    Ok(names
        .iter()
        .map(|name| format!("import {name:?};\n"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NATIVE: &str = "# the list\nnumpy==2.5.4\npydantic-core==2.50.2\n";

    #[test]
    fn every_package_of_a_lock_is_imported() {
        let lock = br#"{"version":"5","specifiers":{"npm:marked@14.1.4":"14.1.4","jsr:@std/assert@*":"1.0.0"}}"#;
        let imports = locked_imports(lock).unwrap();
        assert!(imports.contains(r#"import "npm:marked@14.1.4";"#));
        assert!(imports.contains(r#"import "jsr:@std/assert@*";"#));
        assert_eq!(locked_imports(b"{}").unwrap(), "");
        assert!(locked_imports(b"no").is_err());
    }

    #[test]
    fn a_lock_pins_one_version_with_its_hashes() {
        let lock = "# a comment\nsix==1.17.0 \\\n    --hash=sha256:ab \\\n    --hash=sha256:cd\n    # via x\nPydantic_Core==2.50.2 --hash=sha256:ef\n";
        let found = pins(lock, NATIVE).unwrap();
        assert_eq!(found[0].name, "six");
        assert_eq!(found[0].version, "1.17.0");
        assert!(found[0].line.contains("--hash=sha256:cd"));
        assert_eq!(found[1].name, "pydantic-core");
    }

    #[test]
    fn what_a_lock_may_not_say_is_refused_by_its_name() {
        for (lock, says) in [
            ("six>=1 --hash=sha256:ab\n", "one version"),
            ("six\n", "one version"),
            ("six==1.*  --hash=sha256:ab\n", "one version"),
            ("six==1.17.0\n", "hashes"),
            ("numpy==2.0.0 --hash=sha256:ab\n", "numpy 2.5.4"),
        ] {
            let refused = pins(lock, NATIVE).unwrap_err();
            assert!(refused.contains(says), "{lock}: {refused}");
        }
    }

    #[test]
    fn the_shipped_list_names_one_version_of_each() {
        let list = include_str!("native-packages.txt");
        let mut names: Vec<String> = native_pins(list).map(|(name, _)| name).collect();
        let lines = list
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
            .count();
        assert_eq!(names.len(), lines);
        names.sort();
        names.dedup();
        assert_eq!(names.len(), lines);
    }
}
