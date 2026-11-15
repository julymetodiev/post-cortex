// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The rules of a dynamic module (docs/modules/dynamic.md), pure: what a
//! bundle may hold, what it may ask for, its tar, and what a person approved
//! and is asked: what was approved or asked is the module's, never a request's.

use std::collections::BTreeSet;
use std::fmt::Write;
use std::io::Read;
use std::path::{Component, Path};

use pcx_documents::Item;
use post_cortex_proto::board::Thread;
use post_cortex_proto::dynamic::{
    Approved, Asked, DynamicModule, DynamicRecord, File, Grants, Language, ModuleFile,
};
use sha2::{Digest, Sha256};
use similar::TextDiff;

/// The module's id.
pub const MODULE_ID: &str = "dynamic";
/// The kind of a module's document, by its name in the module.
pub const KIND_NAME: &str = "module";
/// The kind of a module's document.
pub const MODULE_KIND: &str = "dynamic/module";

/// A bundle's limits.
pub const MAX_FILES: usize = 1_000;
const MAX_TAR_BYTES: usize = 16 * 1024 * 1024;
const MAX_UNPACKED_BYTES: usize = 64 * 1024 * 1024;

/// A dynamic module's memory, by default and in MB.
pub const DEFAULT_MEMORY_MB: u32 = 256;

/// The words of the module protocol, which a module never speaks itself: the
/// rule of `foreign_modules` in `scripts/check-architecture.sh`.
pub const FOREIGN: [&str; 4] = [
    "pcx_module_pb",
    "pcx_module_grpc",
    "pcx.module.v1",
    "pcx/module/v1",
];

/// Message code is generated when a bundle is prepared, so what runs is what
/// its proto says.
const GENERATED: [&str; 3] = ["_pb2.py", "_pb2_grpc.py", "_pb.ts"];

/// The modules a dynamic one never uses.
const NEVER_USES: [&str; 2] = ["*", MODULE_ID];

/// Refused, the message naming what is at fault.
pub type Refused = String;

/// `id`: lowercase letters, digits and `-`, not this module's.
///
/// # Errors
/// It is none.
pub fn check_id(id: &str) -> Result<(), Refused> {
    let valid = !id.is_empty()
        && id != MODULE_ID
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if valid {
        Ok(())
    } else {
        Err(format!(
            "{id:?}: a module's id is lowercase letters, digits and -, and not {MODULE_ID}"
        ))
    }
}

/// The file a module of `language` starts from.
#[must_use]
pub fn entry(language: Language) -> Option<&'static str> {
    match language {
        Language::Python => Some("module.py"),
        Language::Ts => Some("module.ts"),
        Language::Unspecified => None,
    }
}

/// `files`, by what a bundle may hold.
///
/// # Errors
/// What is at fault, naming the file.
pub fn check_files(language: Language, files: &[File]) -> Result<(), Refused> {
    let start = entry(language).ok_or("a module's language is python or ts")?;
    if files.len() > MAX_FILES {
        return Err(format!("more than {MAX_FILES} files"));
    }
    if !files.iter().any(|f| f.path == start) {
        return Err(format!("a bundle starts from {start}, and it has none"));
    }
    let mut seen = BTreeSet::new();
    for file in files {
        let path = Path::new(&file.path);
        let beneath =
            !file.path.is_empty() && path.components().all(|c| matches!(c, Component::Normal(_)));
        if !beneath {
            return Err(format!("{}: a path stays beneath the bundle", file.path));
        }
        if !seen.insert(file.path.as_str()) {
            return Err(format!("{}: named twice", file.path));
        }
        if GENERATED.iter().any(|end| file.path.ends_with(end)) {
            return Err(format!(
                "{}: message code is generated from the proto, not kept",
                file.path
            ));
        }
        if let Some(word) = FOREIGN.iter().find(|w| file.text.contains(**w)) {
            return Err(format!(
                "{}: names {word}: a module speaks through the bindings only",
                file.path
            ));
        }
    }
    Ok(())
}

/// `grants`, by what a dynamic module may ask for.
///
/// # Errors
/// What is at fault.
pub fn check_grants(grants: &Grants) -> Result<(), Refused> {
    if let Some(never) = grants
        .uses
        .iter()
        .find(|used| NEVER_USES.contains(&used.as_str()))
    {
        return Err(format!("a dynamic module never uses {never}"));
    }
    Ok(())
}

/// The modules of `grants` that `reachable` lacks: a module's grants are
/// within its author's.
#[must_use]
pub fn beyond_author(grants: &Grants, reachable: &BTreeSet<String>) -> Vec<String> {
    grants
        .uses
        .iter()
        .filter(|used| !reachable.contains(*used))
        .cloned()
        .collect()
}

/// The tar of `files`, the same for the same files: sorted, without times or
/// owners.
///
/// # Errors
/// Over a bundle's limits, or a name a tar cannot hold.
pub fn tar_of(files: &[File]) -> Result<Vec<u8>, Refused> {
    let mut sorted: Vec<&File> = files.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    let unpacked: usize = sorted.iter().map(|f| f.text.len()).sum();
    if unpacked > MAX_UNPACKED_BYTES {
        return Err(format!("more than {MAX_UNPACKED_BYTES} bytes unpacked"));
    }
    let mut builder = tar::Builder::new(Vec::new());
    for file in sorted {
        let mut header = tar::Header::new_gnu();
        header.set_size(file.text.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        builder
            .append_data(&mut header, &file.path, file.text.as_bytes())
            .map_err(|e| format!("{}: {e}", file.path))?;
    }
    let bytes = builder
        .into_inner()
        .map_err(|e| format!("the tar was not made: {e}"))?;
    if bytes.len() > MAX_TAR_BYTES {
        return Err(format!("more than {MAX_TAR_BYTES} bytes packed"));
    }
    Ok(bytes)
}

/// What `item` answers.
#[must_use]
pub fn record(item: Item<DynamicModule>) -> DynamicRecord {
    DynamicRecord {
        document: item.document.id,
        version: item.document.version,
        module: Some(item.data),
        ..Default::default()
    }
}

/// Whether the file at `path` beneath a directory goes in its bundle: not a
/// hidden file or a package cache the author keeps beside it.
#[must_use]
pub fn bundled(path: &str) -> bool {
    !path
        .split('/')
        .any(|part| part.starts_with('.') || part == "node_modules")
}

/// `path` as a path beneath directory `dir`, when it is one.
#[must_use]
pub fn beneath<'a>(dir: &str, path: &'a str) -> Option<&'a str> {
    path.strip_prefix(dir.trim_end_matches('/'))?
        .strip_prefix('/')
}

/// The SHA-256 of `bytes`, in lowercase hex.
#[must_use]
pub fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut out, b| {
            // Writing to a String does not fail.
            if write!(out, "{b:02x}").is_err() {
                tracing::error!("a hash could not be written");
            }
            out
        })
}

/// What `difference` says when the files are the approved ones.
pub const NO_DIFFERENCE: &str = "Nothing changed from the approved version.";

/// The hash of `module`'s files.
fn files_hash(module: &DynamicModule) -> &str {
    module.files.as_ref().map_or("", |f| f.sha256.as_str())
}

fn uses(grants: Option<&Grants>) -> BTreeSet<&str> {
    grants
        .map(|g| g.uses.iter().map(String::as_str).collect())
        .unwrap_or_default()
}

/// Whether `wanted` asks for no more than `had`.
fn within(wanted: Option<&Grants>, had: Option<&Grants>) -> bool {
    let (w, h) = (
        wanted.cloned().unwrap_or_default(),
        had.cloned().unwrap_or_default(),
    );
    uses(wanted).is_subset(&uses(had))
        && (!w.write_memory || h.write_memory)
        && (!w.read_conversation || h.read_conversation)
        && (!w.models || h.models)
        && w.memory_mb <= h.memory_mb
}

/// Whether `module`'s files and grants are what a person approved.
#[must_use]
pub fn approved(module: &DynamicModule) -> bool {
    module.approved.as_ref().is_some_and(|a| {
        a.sha256 == files_hash(module) && within(module.grants.as_ref(), a.grants.as_ref())
    })
}

/// Whether `module`, just written, needs a decision: not approved, and these
/// files were not asked about already, or were and a person declined them
/// (writing them again asks again).
#[must_use]
pub fn asks(module: &DynamicModule) -> bool {
    !approved(module)
        && module
            .asked
            .as_ref()
            .is_none_or(|asked| asked.sha256 != files_hash(module) || !asked.declined_by.is_empty())
}

/// `written` as the module keeps it: what was approved and asked is
/// `previous`'s, whatever the request said; nothing is asked of files
/// approved as they are.
#[must_use]
pub fn kept(mut written: DynamicModule, previous: Option<&DynamicModule>) -> DynamicModule {
    written.approved = previous.and_then(|p| p.approved.clone());
    let asked = previous.and_then(|p| p.asked.clone());
    written.asked = if approved(&written) { None } else { asked };
    written
}

/// The name module `id` asks in: a decision's author.
#[must_use]
pub fn asker(id: &str) -> String {
    format!("dynamic:{id}")
}

/// The decision `waited` that `now` waits on no more: its files changed and
/// are asked about again, or are approved as they are.
#[must_use]
pub fn superseded<'a>(waited: Option<&'a str>, now: &DynamicModule) -> Option<&'a str> {
    waited.filter(|w| now.asked.as_ref().is_none_or(|a| a.decision != *w))
}

/// The decisions of `thread` that module `id` asked and that still wait,
/// besides `current`: asked of files since changed.
#[must_use]
pub fn stale_in(thread: &Thread, id: &str, current: &str) -> Vec<String> {
    let asker = asker(id);
    thread
        .decisions
        .iter()
        .filter(|d| d.pending && d.id != current)
        .filter(|d| d.decision.as_ref().is_some_and(|d| d.decided_by == asker))
        .map(|d| d.id.clone())
        .collect()
}

/// `module` asked about, in `decision` on `task`.
#[must_use]
pub fn asked(mut module: DynamicModule, task: &str, decision: &str) -> DynamicModule {
    module.asked = Some(Asked {
        sha256: files_hash(&module).to_string(),
        task: task.to_string(),
        decision: decision.to_string(),
        declined_by: String::new(),
    });
    module
}

/// `module` with the decision `decision` on its files declined by
/// `declined_by`: what was approved before goes on, and no one is asked
/// until the files are written again; none when it is not the decision it
/// waits on.
#[must_use]
pub fn decline(
    mut module: DynamicModule,
    decision: &str,
    declined_by: &str,
) -> Option<DynamicModule> {
    let waiting = module.asked.as_mut()?;
    if waiting.decision != decision || !waiting.declined_by.is_empty() {
        return None;
    }
    waiting.declined_by = declined_by.to_string();
    Some(module)
}

/// `module` with what `decision` asked approved by `approved_by`; none when it
/// is not the decision it waits on for these files.
#[must_use]
pub fn approve(
    mut module: DynamicModule,
    decision: &str,
    approved_by: &str,
) -> Option<DynamicModule> {
    let waiting = module.asked.take()?;
    if waiting.decision != decision || waiting.sha256 != files_hash(&module) {
        return None;
    }
    module.approved = Some(Approved {
        sha256: waiting.sha256,
        version: module.version.clone(),
        grants: module.grants.clone(),
        decision: decision.to_string(),
        approved_by: approved_by.to_string(),
        language: module.language,
        files: module.files.clone(),
    });
    Some(module)
}

/// What the decision on `module` asks a person.
#[must_use]
pub fn question(module: &DynamicModule) -> String {
    let wanted = uses(module.grants.as_ref());
    let list = if wanted.is_empty() {
        "no module".to_string()
    } else {
        wanted.into_iter().collect::<Vec<_>>().join(", ")
    };
    match &module.approved {
        None => format!(
            "Let dynamic module {} {} run, calling {list}?",
            module.id, module.version
        ),
        Some(had) => format!(
            "Let dynamic module {} change from {} to {}, calling {list}?",
            module.id, had.version, module.version
        ),
    }
}

/// The files of the tar `bytes`: their paths and text.
///
/// # Errors
/// It does not read.
pub fn files_of(bytes: &[u8]) -> Result<Vec<File>, Refused> {
    let mut archive = tar::Archive::new(bytes);
    let entries = archive
        .entries()
        .map_err(|e| format!("the bundle does not read: {e}"))?;
    let mut files = Vec::new();
    for entry in entries {
        let mut entry = entry.map_err(|e| format!("the bundle does not read: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| format!("the bundle does not read: {e}"))?
            .display()
            .to_string();
        let mut text = String::new();
        entry
            .read_to_string(&mut text)
            .map_err(|e| format!("{path}: {e}"))?;
        files.push(File { path, text });
    }
    Ok(files)
}

/// `files` with `text` at `path`: that file changed, or added, in path order.
#[must_use]
pub fn with_file(mut files: Vec<File>, path: &str, text: &str) -> Vec<File> {
    match files.iter_mut().find(|f| f.path == path) {
        Some(file) => file.text = text.to_string(),
        None => files.push(File {
            path: path.to_string(),
            text: text.to_string(),
        }),
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

/// `text` as a Markdown code block in `language`: its fence is longer than any
/// run of backticks that opens a line of it, so no line of it closes the block.
#[must_use]
pub fn fenced(language: &str, text: &str) -> String {
    let longest = text
        .lines()
        .map(|line| line.trim_start().chars().take_while(|c| *c == '`').count())
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    let end = if text.ends_with('\n') { "" } else { "\n" };
    format!("{fence}{language}\n{text}{end}{fence}")
}

/// A file as a person reads it: its path, then its text.
#[must_use]
pub fn shown_file(file: &File) -> String {
    let language = Path::new(&file.path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    format!("`{}`\n\n{}", file.path, fenced(language, &file.text))
}

/// `files` of `module`, read at document version `revision`, as its view's rows.
#[must_use]
pub fn rows(module: &DynamicModule, revision: u64, files: Vec<File>) -> Vec<ModuleFile> {
    files
        .into_iter()
        .map(|file| ModuleFile {
            id: module.id.clone(),
            revision,
            version: module.version.clone(),
            shown: shown_file(&file),
            path: file.path,
            text: file.text,
        })
        .collect()
}

/// What changes from the files `before` to the files `after`, as a person
/// reads it, in Markdown.
#[must_use]
pub fn difference(before: &[File], after: &[File]) -> String {
    let text_of = |files: &[File], path: &str| {
        files
            .iter()
            .find(|f| f.path == path)
            .map(|f| f.text.clone())
            .unwrap_or_default()
    };
    let paths: BTreeSet<&str> = before
        .iter()
        .chain(after)
        .map(|f| f.path.as_str())
        .collect();
    let mut out = String::new();
    for path in paths {
        let (old, new) = (text_of(before, path), text_of(after, path));
        if old != new {
            out.push_str(
                &TextDiff::from_lines(&old, &new)
                    .unified_diff()
                    .header(path, path)
                    .to_string(),
            );
        }
    }
    if out.is_empty() {
        return NO_DIFFERENCE.to_string();
    }
    fenced("diff", &out)
}

#[cfg(test)]
mod tests;
