// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! How a prepared bundle runs (docs/modules/dynamic.md): the host's Python or
//! Deno, from the bundle's directory, with its message code and packages on
//! the path. The module's check and the host's load run the same line.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use pcx_module_api::{DESCRIPTORS_ENV, DESCRIPTORS_FILE};
use post_cortex_proto::dynamic::Language;

/// The runtimes of a host, as the launcher reads them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runtimes {
    /// Free-threaded CPython.
    pub python: PathBuf,
    /// Deno.
    pub deno: PathBuf,
    /// The import map that gives a TS module the host's `@post-cortex/module` and
    /// `@bufbuild/protobuf`: the binding always matches the host, and the
    /// module's messages and the binding share one protobuf.
    pub imports: PathBuf,
}

/// A process to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    /// The program.
    pub program: PathBuf,
    /// Its arguments.
    pub args: Vec<String>,
    /// Its environment.
    pub env: HashMap<String, String>,
    /// What it reads.
    pub read: Vec<PathBuf>,
    /// Its working directory.
    pub cwd: PathBuf,
}

impl Runtimes {
    fn base(
        &self,
        language: Language,
        bundle: &Path,
        environment: Option<&Path>,
    ) -> Result<Run, String> {
        let mut read = vec![bundle.to_path_buf()];
        read.extend(environment.map(Path::to_path_buf));
        let mut env = HashMap::new();
        // Named whether or not it can be seen from here: in the cloud the bundle
        // is in the launcher's pot, and only the module's process reads it.
        env.insert(
            DESCRIPTORS_ENV.to_string(),
            bundle
                .join("gen")
                .join(DESCRIPTORS_FILE)
                .display()
                .to_string(),
        );
        let program = match language {
            Language::Python => {
                let mut path = vec![bundle.join("gen").display().to_string()];
                path.extend(environment.map(|e| e.display().to_string()));
                env.insert("PYTHONPATH".to_string(), path.join(":"));
                env.insert("PYTHONDONTWRITEBYTECODE".to_string(), "1".to_string());
                self.python.clone()
            }
            Language::Ts => {
                env.insert("DENO_NO_UPDATE_CHECK".to_string(), "1".to_string());
                if let Some(cache) = environment {
                    env.insert("DENO_DIR".to_string(), cache.display().to_string());
                }
                self.deno.clone()
            }
            Language::Unspecified => return Err("no language".to_string()),
        };
        Ok(Run {
            program,
            args: Vec::new(),
            env,
            read,
            cwd: bundle.to_path_buf(),
        })
    }

    /// What Deno is told for `command`: a bundle's packages only as they were
    /// prepared.
    fn deno_args(&self, command: &str, environment: Option<&Path>) -> Vec<String> {
        let mut args = vec![
            command.to_string(),
            "--no-prompt".to_string(),
            "--allow-all".to_string(),
            format!("--import-map={}", self.imports.display()),
        ];
        if environment.is_some() {
            // Run from the bundle: its lock file alone says the versions.
            args.push("--cached-only".to_string());
            args.push("--frozen".to_string());
            args.push("--lock=deno.lock".to_string());
        }
        args
    }

    /// The module of `bundle` running.
    ///
    /// # Errors
    /// No language.
    pub fn module(
        &self,
        language: Language,
        bundle: &Path,
        environment: Option<&Path>,
        memory_mb: u32,
    ) -> Result<Run, String> {
        let mut run = self.base(language, bundle, environment)?;
        run.args = match language {
            Language::Ts => {
                let mut args = self.deno_args("run", environment);
                args.push(format!("--v8-flags=--max-old-space-size={memory_mb}"));
                args.push("module.ts".to_string());
                args
            }
            _ => vec!["module.py".to_string()],
        };
        Ok(run)
    }

    /// The tests under `bundle/tests`.
    ///
    /// # Errors
    /// No language.
    pub fn tests(
        &self,
        language: Language,
        bundle: &Path,
        environment: Option<&Path>,
    ) -> Result<Run, String> {
        let mut run = self.base(language, bundle, environment)?;
        run.args = match language {
            Language::Ts => {
                let mut args = self.deno_args("test", environment);
                // Types are checked apart, by [`Self::types`].
                args.push("--no-check".to_string());
                args.push("tests/".to_string());
                args
            }
            _ => ["-m", "unittest", "discover", "-s", "tests"]
                .map(String::from)
                .to_vec(),
        };
        Ok(run)
    }

    /// The run that checks the types of `bundle`'s tests and what they import,
    /// for a language that has them: the compiler takes far more memory than a
    /// module's run, so it has its own limit. It reads the bundle and runs none
    /// of its code.
    ///
    /// # Errors
    /// As [`Self::tests`].
    pub fn types(
        &self,
        language: Language,
        bundle: &Path,
        environment: Option<&Path>,
    ) -> Result<Option<Run>, String> {
        if language != Language::Ts {
            return Ok(None);
        }
        let mut run = self.base(language, bundle, environment)?;
        run.args = vec![
            "check".to_string(),
            format!("--import-map={}", self.imports.display()),
        ];
        if environment.is_some() {
            run.args
                .extend(["--cached-only", "--frozen", "--lock=deno.lock"].map(String::from));
        }
        run.args.push("tests/".to_string());
        Ok(Some(run))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typescript_has_its_types_checked_apart_and_python_none() {
        let runtimes = Runtimes {
            python: "py".into(),
            deno: "deno".into(),
            imports: "/i/imports.json".into(),
        };
        let bundle = Path::new("/b");
        let env = Path::new("/e");
        assert!(
            runtimes
                .types(Language::Python, bundle, Some(env))
                .unwrap()
                .is_none()
        );
        let types = runtimes
            .types(Language::Ts, bundle, Some(env))
            .unwrap()
            .unwrap();
        assert_eq!(types.args[0], "check");
        assert!(types.args.iter().any(|a| a == "--cached-only"));
        let tests = runtimes.tests(Language::Ts, bundle, Some(env)).unwrap();
        assert!(tests.args.iter().any(|a| a == "--no-check"));
    }

    #[test]
    fn a_module_runs_from_its_bundle_with_its_packages_on_the_path() {
        let runtimes = Runtimes {
            python: "/py".into(),
            deno: "/deno".into(),
            imports: "/imports.json".into(),
        };
        let run = runtimes
            .module(
                Language::Python,
                Path::new("/b"),
                Some(Path::new("/e")),
                256,
            )
            .unwrap();
        assert_eq!(run.program, PathBuf::from("/py"));
        assert_eq!(run.args, ["module.py"]);
        assert_eq!(run.env["PYTHONPATH"], "/b/gen:/e");
        // Named though `/b` is not on this machine: the launcher's may be elsewhere.
        assert_eq!(
            run.env[DESCRIPTORS_ENV],
            format!("/b/gen/{DESCRIPTORS_FILE}")
        );
        assert_eq!(run.read, [PathBuf::from("/b"), PathBuf::from("/e")]);
        let run = runtimes
            .module(Language::Ts, Path::new("/b"), None, 512)
            .unwrap();
        assert!(
            run.args
                .contains(&"--v8-flags=--max-old-space-size=512".to_string())
        );
        assert!(
            runtimes
                .module(Language::Unspecified, Path::new("/b"), None, 1)
                .is_err()
        );
    }
}
