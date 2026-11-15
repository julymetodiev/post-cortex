// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A bundle prepared before any of its code runs (docs/modules/dynamic.md,
//! prepared): unpacked read-only, its packages installed from its lock file
//! and nothing compiled. A native package comes from the wheelhouse of the
//! image when it holds it, anything else from the index, by the lock's
//! hashes. An environment is kept, shared by the bundles with its lock.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pcx_sandbox::Launcher;
use sha2::{Digest, Sha256};
use tokio::process::Command;
use tokio::sync::{Mutex, OnceCell};

use crate::bundle::unpack;
use crate::confined::{Confined, Confinement, Ran};
use crate::lock::{Pin, pins};
use crate::protoc;
use crate::watch::Watcher;

/// The native packages every module shares, one version of each.
const NATIVE: &str = include_str!("native-packages.txt");

/// How long installing may take.
const INSTALLING: Duration = Duration::from_mins(10);

/// The memory and open files of installing.
const WORK_MB: u32 = 2 * 1024;
const WORK_FILES: u64 = 4 * 1024;

/// What names this platform to an environment.
const PLATFORM: &str = "import sys, sysconfig; print(sys.implementation.cache_tag, sysconfig.get_platform(), sys.abiflags)";

/// Why a bundle could not be prepared.
#[derive(Debug, thiserror::Error)]
pub enum PrepareError {
    /// The bundle is not what its hash says.
    #[error("the bundle's hash is {found}, not {said}")]
    Hash {
        /// What it is.
        found: String,
        /// What was said.
        said: String,
    },
    /// The bundle breaks a rule; the message names the file.
    #[error("bundle: {0}")]
    Bundle(String),
    /// Its lock file cannot be met; the message names the package.
    #[error("dependencies: {0}")]
    Dependencies(String),
    /// A step could not run.
    #[error("{0}")]
    Step(String),
}

/// A prepared bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    /// Its files, read-only.
    pub bundle: PathBuf,
    /// Its packages (none without a lock file).
    pub environment: Option<PathBuf>,
}

/// What prepares bundles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepareConfig {
    /// Where bundles and environments are kept.
    pub store: PathBuf,
    /// The Python that installs (with `pip`).
    pub python: PathBuf,
    /// What every step reads: the runtimes.
    pub read: Vec<PathBuf>,
    /// The native packages built for this platform, when it has none on
    /// the index (FreeBSD: in the image).
    pub wheelhouse: Option<PathBuf>,
    /// The protobuf compiler, for a bundle with protos.
    pub protoc: Option<PathBuf>,
    /// Its plugin for TypeScript, for a TS bundle with protos.
    pub protoc_gen_es: Option<PathBuf>,
    /// Deno and the import map it runs a module with, for a TS bundle with a
    /// lock file.
    pub deno: Option<(PathBuf, PathBuf)>,
}

/// Prepares bundles.
pub struct Preparer {
    config: PrepareConfig,
    launcher: Launcher,
    watcher: Arc<Watcher>,
    bundles: Mutex<()>,
    environments: Mutex<()>,
    platform: OnceCell<String>,
}

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A step's failure, naming its path.
pub fn step(what: &str, path: &Path, e: &std::io::Error) -> PrepareError {
    PrepareError::Step(format!("{what} {}: {e}", path.display()))
}

/// The `.proto` files under `dir` (none when it is not there), as a compiler
/// names them from its include path.
fn protos_in(dir: &Path) -> Result<Vec<String>, PrepareError> {
    let mut found = Vec::new();
    let mut under = vec![dir.to_path_buf()];
    while let Some(next) = under.pop() {
        let entries = match std::fs::read_dir(&next) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(step("read", &next, &e)),
        };
        for entry in entries {
            let path = entry.map_err(|e| step("read", &next, &e))?.path();
            if path.is_dir() {
                under.push(path);
            } else if path.extension().is_some_and(|e| e == "proto")
                && let Ok(relative) = path.strip_prefix(dir)
            {
                found.push(relative.to_string_lossy().into_owned());
            }
        }
    }
    found.sort();
    Ok(found)
}

/// `pin`'s wheel in `house`, if it holds one (`name-version-*.whl`, the name
/// with `_`).
fn wheel_of(house: &Path, pin: &Pin) -> Result<Option<PathBuf>, PrepareError> {
    let prefix = format!("{}-{}-", pin.name.replace('-', "_"), pin.version);
    let entries = std::fs::read_dir(house).map_err(|e| step("read", house, &e))?;
    for entry in entries {
        let path = entry.map_err(|e| step("read", house, &e))?.path();
        let held = path
            .file_name()
            .map(|name| name.to_string_lossy())
            .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".whl"));
        if held {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

impl Preparer {
    /// A preparer of `config`, confining by `launcher`. Call within a
    /// runtime.
    #[must_use]
    pub fn new(config: PrepareConfig, launcher: Launcher) -> Self {
        Self {
            config,
            launcher,
            watcher: Watcher::start(),
            bundles: Mutex::new(()),
            environments: Mutex::new(()),
            platform: OnceCell::new(),
        }
    }

    /// The bundle `tar_bytes` of hash `sha256`, prepared, or as it was kept.
    ///
    /// # Errors
    /// See [`PrepareError`].
    pub async fn prepare(&self, tar_bytes: &[u8], sha256: &str) -> Result<Prepared, PrepareError> {
        let found = hex(tar_bytes);
        if !found.eq_ignore_ascii_case(sha256) {
            return Err(PrepareError::Hash {
                found,
                said: sha256.to_string(),
            });
        }
        let bundle = self.unpacked(tar_bytes, &found).await?;
        let lock = bundle.join("requirements.lock");
        let deno_lock = bundle.join("deno.lock");
        let environment = if lock.exists() {
            Some(self.environment(&lock).await?)
        } else if deno_lock.exists() {
            Some(self.deno_cache(&bundle, &deno_lock).await?)
        } else {
            None
        };
        Ok(Prepared {
            bundle,
            environment,
        })
    }

    async fn unpacked(&self, tar_bytes: &[u8], sha256: &str) -> Result<PathBuf, PrepareError> {
        let parent = self.config.store.join("bundles");
        let bundle = parent.join(sha256);
        // One at a time: two tenants may install the same files.
        let _one = self.bundles.lock().await;
        if bundle.exists() {
            return Ok(bundle);
        }
        let partial = parent.join(format!(".{sha256}.{}", std::process::id()));
        std::fs::create_dir_all(&partial).map_err(|e| step("make", &partial, &e))?;
        let made = match unpack(tar_bytes, &partial) {
            Ok(()) => self.generated(&partial).await,
            Err(refused) => Err(refused),
        };
        let kept = made.and_then(|()| {
            std::fs::rename(&partial, &bundle).map_err(|e| step("keep", &bundle, &e))
        });
        if let Err(refused) = kept {
            if let Err(e) = std::fs::remove_dir_all(&partial) {
                tracing::warn!(error = %e, "remove a refused bundle");
            }
            return Err(refused);
        }
        Ok(bundle)
    }

    /// The message code of the protos under `bundle/proto`, in `bundle/gen`,
    /// by the language `bundle` starts from: what runs is what its proto says.
    async fn generated(&self, bundle: &Path) -> Result<(), PrepareError> {
        let protos = protos_in(&bundle.join("proto"))?;
        if protos.is_empty() {
            return Ok(());
        }
        let protoc = self
            .config
            .protoc
            .as_ref()
            .ok_or_else(|| PrepareError::Step("this host has no protoc".to_string()))?;
        let proto = bundle.join("proto");
        let generated = bundle.join("gen");
        std::fs::create_dir_all(&generated).map_err(|e| step("make", &generated, &e))?;
        // The compiler runs as the jail's user, and writes here.
        self.launcher
            .give(&generated)
            .map_err(|e| step("give", &generated, &e))?;
        let plugin =
            if bundle.join("module.ts").exists() {
                Some(self.config.protoc_gen_es.as_deref().ok_or_else(|| {
                    PrepareError::Step("this host has no protoc-gen-es".to_string())
                })?)
            } else {
                None
            };
        let args = protoc::args(&proto, &generated, plugin, protos);
        let ran = self.run(protoc, bundle, false, &args).await?;
        if ran.success {
            Ok(())
        } else {
            Err(PrepareError::Bundle(format!(
                "its protos do not compile: {}",
                ran.tail()
            )))
        }
    }

    /// This platform: its Python, system and architecture.
    async fn platform(&self) -> Result<&String, PrepareError> {
        self.platform
            .get_or_try_init(|| async {
                let python = &self.config.python;
                let found = Command::new(python)
                    .args(["-I", "-c", PLATFORM])
                    .output()
                    .await
                    .map_err(|e| step("run", python, &e))?;
                if !found.status.success() {
                    return Err(PrepareError::Step(format!(
                        "{}: no platform: {}",
                        python.display(),
                        String::from_utf8_lossy(&found.stderr).trim()
                    )));
                }
                Ok(String::from_utf8_lossy(&found.stdout).trim().to_string())
            })
            .await
    }

    /// The environment of `lock`, kept by the lock, the platform and the
    /// list of native packages.
    async fn environment(&self, lock: &Path) -> Result<PathBuf, PrepareError> {
        let text = std::fs::read_to_string(lock).map_err(|e| step("read", lock, &e))?;
        let pins = pins(&text, NATIVE).map_err(PrepareError::Dependencies)?;
        let id = hex(format!("{text}\n{}\n{NATIVE}", self.platform().await?).as_bytes());
        let kept = self.config.store.join("environments").join(&id);
        // One at a time: two bundles with one lock share its environment.
        let _one = self.environments.lock().await;
        if kept.exists() {
            return Ok(kept);
        }
        let work = self.config.store.join("work").join(&id);
        let scratch = work.with_extension("scratch");
        for left in [&work, &scratch] {
            match std::fs::remove_dir_all(left) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(step("empty", left, &e)),
            }
        }
        std::fs::create_dir_all(&work).map_err(|e| step("make", &work, &e))?;
        let done = self.install(&pins, &work, &kept).await;
        for left in [&work, &scratch] {
            match std::fs::remove_dir_all(left) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => tracing::warn!(error = %e, "remove a preparation's work"),
            }
        }
        done.map(|()| kept)
    }

    /// The npm packages of `lock` in a Deno cache, kept by the lock, shared by
    /// the bundles with it; no package's script runs (`deno cache` runs none).
    async fn deno_cache(&self, bundle: &Path, lock: &Path) -> Result<PathBuf, PrepareError> {
        let (deno, imports) = self.config.deno.as_ref().ok_or_else(|| {
            PrepareError::Step("this host has no Deno to prepare with".to_string())
        })?;
        let text = std::fs::read(lock).map_err(|e| step("read", lock, &e))?;
        let id = hex(&text);
        let kept = self.config.store.join("environments").join(&id);
        let _one = self.environments.lock().await;
        if kept.exists() {
            return Ok(kept);
        }
        let work = self.config.store.join("work").join(&id);
        let scratch = work.with_extension("scratch");
        for left in [&work, &scratch] {
            match std::fs::remove_dir_all(left) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(step("empty", left, &e)),
            }
        }
        std::fs::create_dir_all(&work).map_err(|e| step("make", &work, &e))?;
        let cache = work.join("environment");
        // Every package of the lock: the environment is the lock's, tests' included.
        let listed = work.join("locked.ts");
        let locked = crate::lock::locked_imports(&text).map_err(PrepareError::Dependencies)?;
        std::fs::write(&listed, locked).map_err(|e| step("write", &listed, &e))?;
        let args = [
            "cache".to_string(),
            "--frozen".to_string(),
            format!("--lock={}", lock.display()),
            format!("--import-map={}", imports.display()),
            bundle.join("module.ts").display().to_string(),
            listed.display().to_string(),
        ];
        let env = [("DENO_DIR".to_string(), cache.display().to_string())];
        let done = match self
            .confined(deno, &work, true, &[bundle], &env, &args)
            .await
        {
            Ok(ran) if ran.success => {
                let parent = self.config.store.join("environments");
                std::fs::create_dir_all(&parent)
                    .and_then(|()| std::fs::rename(&cache, &kept))
                    .map_err(|e| step("keep", &kept, &e))
            }
            Ok(ran) => Err(PrepareError::Dependencies(ran.tail())),
            Err(failed) => Err(failed),
        };
        for left in [&work, &scratch] {
            if let Err(e) = std::fs::remove_dir_all(left)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(error = %e, "remove a preparation's work");
            }
        }
        done.map(|()| kept)
    }

    async fn install(&self, pins: &[Pin], work: &Path, kept: &Path) -> Result<(), PrepareError> {
        let mut built = Vec::new();
        let mut from_index = String::new();
        for pin in pins {
            let held = match &self.config.wheelhouse {
                Some(house) => wheel_of(house, pin)?,
                None => None,
            };
            match held {
                Some(wheel) => built.push(wheel.to_string_lossy().into_owned()),
                None => {
                    from_index.push_str(&pin.line);
                    from_index.push('\n');
                }
            }
        }
        let target = work.join("environment");
        let pip = |extra: &[&str]| -> Vec<String> {
            [
                "-I",
                "-m",
                "pip",
                "install",
                "--quiet",
                "--no-deps",
                "--no-compile",
                "--no-cache-dir",
                "--disable-pip-version-check",
                "--target",
            ]
            .iter()
            .map(ToString::to_string)
            .chain([target.to_string_lossy().into_owned()])
            .chain(extra.iter().map(ToString::to_string))
            .collect()
        };
        if !from_index.is_empty() {
            let lock = work.join("requirements.lock");
            std::fs::write(&lock, &from_index).map_err(|e| step("write", &lock, &e))?;
            let lock = lock.to_string_lossy().into_owned();
            let args = pip(&["--require-hashes", "--only-binary", ":all:", "-r", &lock]);
            let ran = self.run(&self.config.python, work, true, &args).await?;
            if !ran.success {
                return Err(PrepareError::Dependencies(ran.tail()));
            }
        }
        if !built.is_empty() {
            let mut extra: Vec<&str> = vec!["--no-index"];
            extra.extend(built.iter().map(String::as_str));
            let ran = self
                .run(&self.config.python, work, false, &pip(&extra))
                .await?;
            if !ran.success {
                return Err(PrepareError::Dependencies(ran.tail()));
            }
        }
        let parent = kept
            .parent()
            .ok_or_else(|| PrepareError::Step(format!("{}: no parent", kept.display())))?;
        std::fs::create_dir_all(parent).map_err(|e| step("make", parent, &e))?;
        if !target.exists() {
            std::fs::create_dir_all(&target).map_err(|e| step("make", &target, &e))?;
        }
        std::fs::rename(&target, kept).map_err(|e| step("keep", kept, &e))
    }

    /// `program` confined: writing only `work` and a scratch directory beside
    /// it, the network only when it fetches.
    async fn run(
        &self,
        program: &Path,
        work: &Path,
        network: bool,
        args: &[String],
    ) -> Result<Ran, PrepareError> {
        self.confined(program, work, network, &[], &[], args).await
    }

    /// [`Preparer::run`], reading `read` too, with `env`.
    async fn confined(
        &self,
        program: &Path,
        work: &Path,
        network: bool,
        read: &[&Path],
        env: &[(String, String)],
        args: &[String],
    ) -> Result<Ran, PrepareError> {
        self.launcher
            .give(work)
            .map_err(|e| step("give", work, &e))?;
        let confinement = Confinement {
            read: self
                .config
                .read
                .iter()
                .chain(&self.config.wheelhouse)
                .cloned()
                .chain(read.iter().map(|path| path.to_path_buf()))
                .collect(),
            written: vec![work.to_path_buf()],
            // Beside the work, not in it: nullfs mounts no root over another.
            scratch: work.with_extension("scratch"),
            cwd: work.to_path_buf(),
            env: env.to_vec(),
            network,
            spawn: true,
            memory_mb: WORK_MB,
            open_files: WORK_FILES,
            // A preparation serves no socket.
            sockets_group: None,
        };
        Confined::new(
            self.launcher.clone(),
            Arc::clone(&self.watcher),
            confinement,
        )
        .run(program, args, INSTALLING)
        .await
        .map_err(PrepareError::Step)
    }
}
