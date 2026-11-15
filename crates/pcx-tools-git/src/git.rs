// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The git service of `tools-git`: typed operations as the `git` program
//! run through the module's sandbox (so its hooks run there too), between
//! the tool handlers (JSON) and the `Sandbox` port. Only the configuration
//! names a commit's author: the user's `~/.gitconfig` is out of reach.

use std::time::Duration;

use pcx_module_api::sandbox::{ExecExit, ExecSpec, Sandbox, SandboxPath, process_env};
use pcx_module_api::text::{Excerpt, head};
use pcx_module_api::{AgentAuthor, ModuleError, ModuleErrorKind};
use tokio_util::sync::CancellationToken;

use crate::parse::{self, Branch, Change, Commit, Conventional, Unreadable};
use crate::push::Push;

/// The git program.
const GIT: &str = "git";

/// Longest a git command may run (hooks included).
const GIT_TIMEOUT: Duration = Duration::from_secs(120);

/// Bytes of git's output kept.
const OUTPUT_LIMIT: usize = 8 * 1024 * 1024;

/// Commits `log` lists when the call names no count.
const DEFAULT_LOG: usize = 20;

/// Commits `log` lists at most.
const MAX_LOG: usize = 500;

/// Lines of a patch answered at most.
const MAX_PATCH_LINES: usize = 2000;

/// Bytes of a patch answered at most.
const MAX_PATCH_BYTES: usize = 100 * 1024;

/// The author of the module's commits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Author {
    /// The name.
    pub name: String,
    /// The email.
    pub email: String,
}

/// A commit made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Committed {
    /// Its id.
    pub sha: String,
    /// Its message read as a Conventional Commit, when it is one.
    pub conventional: Option<Conventional>,
}

fn invalid(reason: String) -> ModuleError {
    ModuleError::new(ModuleErrorKind::InvalidArgument, reason)
}

fn unreadable(e: Unreadable) -> ModuleError {
    ModuleError::new(ModuleErrorKind::Internal, e.to_string())
}

/// A revision or branch the call names: not empty, not an option.
fn revision<'r>(what: &str, name: &'r str) -> Result<&'r str, ModuleError> {
    if name.is_empty() || name.starts_with('-') {
        return Err(invalid(format!(
            "{what} names a revision, not an option or nothing: {name:?}"
        )));
    }
    Ok(name)
}

/// The repositories of one module, through its sandbox.
pub struct Git<'a> {
    sandbox: &'a dyn Sandbox,
    /// `PATH` of its processes; none: the module API's default.
    path: Option<&'a str>,
    /// Its commits' author; none: it does not commit.
    author: Option<&'a Author>,
    /// Where it pushes; none: it does not push.
    push: Option<&'a Push>,
}

impl<'a> Git<'a> {
    /// The repositories `sandbox` lets the module reach.
    pub fn new(
        sandbox: &'a dyn Sandbox,
        path: Option<&'a str>,
        author: Option<&'a Author>,
        push: Option<&'a Push>,
    ) -> Self {
        Self {
            sandbox,
            path,
            author,
            push,
        }
    }

    /// The changes of the working tree and the index in `repo`.
    ///
    /// # Errors
    /// git failed, or printed what does not read.
    pub async fn status(&self, repo: Option<SandboxPath>) -> Result<Vec<Change>, ModuleError> {
        let out = self
            .git(
                &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
                repo,
            )
            .await?;
        parse::status(&out).map_err(unreadable)
    }

    /// The patch of `repo`'s changes: the staged ones, or (none: by
    /// default) those of the working tree against the index; only of
    /// `path` when it names one. Its first 2000 lines (100 KiB) when it is
    /// longer.
    ///
    /// # Errors
    /// git failed.
    pub async fn diff(
        &self,
        repo: Option<SandboxPath>,
        staged: Option<bool>,
        path: Option<&str>,
    ) -> Result<Excerpt, ModuleError> {
        let mut args = vec!["diff", "--no-color", "--no-ext-diff"];
        if staged.unwrap_or(false) {
            args.push("--cached");
        }
        args.push("--");
        if let Some(path) = path {
            args.push(path);
        }
        let patch = self.git(&args, repo).await?;
        Ok(head(&patch, MAX_PATCH_LINES, MAX_PATCH_BYTES))
    }

    /// The last `limit` commits (1 to 500; none: 20) of `rev` (none: HEAD).
    ///
    /// # Errors
    /// `InvalidArgument` for a count out of range or a revision that is an
    /// option; git failed, or printed what does not read.
    pub async fn log(
        &self,
        repo: Option<SandboxPath>,
        limit: Option<usize>,
        rev: Option<&str>,
    ) -> Result<Vec<Commit>, ModuleError> {
        let limit = limit.unwrap_or(DEFAULT_LOG);
        if limit == 0 || limit > MAX_LOG {
            return Err(invalid(format!("limit is 1 to {MAX_LOG}, not {limit}")));
        }
        let count = format!("-n{limit}");
        let mut args = vec!["log", &count, "--format=%H%x1f%an%x1f%aI%x1f%s%x1e"];
        if let Some(rev) = rev {
            args.push(revision("rev", rev)?);
        }
        let out = self.git(&args, repo).await?;
        parse::log(&out).map_err(unreadable)
    }

    /// The commits on `branch` that `base` (none: HEAD) does not have, newest
    /// first: the work of a task branch.
    ///
    /// # Errors
    /// As [`Git::log`].
    pub async fn branch_commits(
        &self,
        repo: Option<SandboxPath>,
        branch: &str,
        base: Option<&str>,
    ) -> Result<Vec<Commit>, ModuleError> {
        let range = format!(
            "{}..{}",
            revision("base", base.unwrap_or("HEAD"))?,
            revision("branch", branch)?
        );
        let out = self
            .git(
                &["log", "--format=%H%x1f%an%x1f%aI%x1f%s%x1e", &range],
                repo,
            )
            .await?;
        parse::log(&out).map_err(unreadable)
    }

    /// The local branches.
    ///
    /// # Errors
    /// git failed, or printed what does not read.
    pub async fn branches(&self, repo: Option<SandboxPath>) -> Result<Vec<Branch>, ModuleError> {
        let out = self
            .git(
                &[
                    "branch",
                    "--format=%(refname:short)%1f%(HEAD)%1f%(upstream:short)%1e",
                ],
                repo,
            )
            .await?;
        parse::branches(&out).map_err(unreadable)
    }

    /// A new branch `name` from `from` (none: HEAD), checked out unless
    /// `switch` is false; whether it was checked out.
    ///
    /// # Errors
    /// `InvalidArgument` for a name that is an option; git failed.
    pub async fn create_branch(
        &self,
        repo: Option<SandboxPath>,
        name: &str,
        from: Option<&str>,
        switch: Option<bool>,
    ) -> Result<bool, ModuleError> {
        let switch = switch.unwrap_or(true);
        let name = revision("name", name)?;
        let from = revision("from", from.unwrap_or("HEAD"))?;
        let args = if switch {
            ["switch", "-c", name, from]
        } else {
            ["branch", "--", name, from]
        };
        self.git(&args, repo).await?;
        Ok(switch)
    }

    /// Check out branch `name`.
    ///
    /// # Errors
    /// `InvalidArgument` for a name that is an option; git failed.
    pub async fn switch(&self, repo: Option<SandboxPath>, name: &str) -> Result<(), ModuleError> {
        self.git(&["switch", revision("name", name)?], repo).await?;
        Ok(())
    }

    /// Commit `paths` (none: every change) with `message`: the configured
    /// author commits it, and names `by` its author when an agent acts
    /// (docs/host.md, a commit's author).
    ///
    /// # Errors
    /// `FailedPrecondition` without a configured author; `InvalidArgument`
    /// for an empty message; git failed (nothing to commit, a hook refused).
    pub async fn commit(
        &self,
        repo: Option<SandboxPath>,
        message: &str,
        paths: &[String],
        by: Option<&AgentAuthor>,
    ) -> Result<Committed, ModuleError> {
        let author = self.author.ok_or_else(|| {
            ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                "tools-git commits only as the author its configuration names (author_name, author_email)",
            )
        })?;
        if message.trim().is_empty() {
            return Err(invalid("a commit needs a message".to_string()));
        }
        let mut add = vec!["add", "-A", "--"];
        add.extend(paths.iter().map(String::as_str));
        self.git(&add, repo.clone()).await?;
        let name = format!("user.name={}", author.name);
        let email = format!("user.email={}", author.email);
        let mut commit = vec!["-c", &name, "-c", &email, "commit", "--quiet"];
        let agent = by.map(|a| format!("--author={} <{}>", a.name, a.email));
        commit.extend(agent.as_deref());
        commit.extend(["-m", message]);
        self.git(&commit, repo.clone()).await?;
        let sha = self.git(&["rev-parse", "HEAD"], repo).await?;
        Ok(Committed {
            sha: sha.trim().to_string(),
            conventional: parse::conventional(message),
        })
    }

    /// A worktree of `repo` at `path` (relative to it, inside it) on
    /// `branch`: made, or the one there; its absolute directory and whether
    /// it was there. A branch that does not exist starts from `from`; none
    /// names it: with a remote (`push`), from the remote's copy of the
    /// branch the repository has checked out, fetched first, so a task never
    /// starts on a clone gone stale; without one, from HEAD.
    ///
    /// # Errors
    /// `InvalidArgument` for a path that leaves the repository or is an
    /// option, or a branch that is one; `FailedPrecondition` for a remote
    /// the module cannot reach or a checkout on no branch; git failed (the
    /// fetch too).
    pub async fn add_worktree(
        &self,
        repo: Option<SandboxPath>,
        path: &str,
        branch: &str,
        from: Option<&str>,
    ) -> Result<(String, bool), ModuleError> {
        let inside = !path.is_empty()
            && !path.starts_with(['-', '/'])
            && path.split('/').all(|part| !part.is_empty() && part != "..");
        if !inside {
            return Err(invalid(format!(
                "a worktree's path is a directory inside the repository: {path:?}"
            )));
        }
        let branch = revision("branch", branch)?;
        let root = self
            .git(&["rev-parse", "--show-toplevel"], repo.clone())
            .await?;
        let dir = format!("{}/{path}", root.trim());
        let listed = self
            .git(&["worktree", "list", "--porcelain"], repo.clone())
            .await?;
        if listed.lines().any(|l| l == format!("worktree {dir}")) {
            return Ok((dir, true));
        }
        let known = format!("refs/heads/{branch}");
        let exists = self
            .git(&["show-ref", "--verify", "--quiet", &known], repo.clone())
            .await
            .is_ok();
        if exists {
            self.git(&["worktree", "add", path, branch], repo).await?;
        } else {
            let from = match (from, self.push) {
                (Some(from), _) => revision("from", from)?.to_string(),
                (None, Some(push)) => self.fetch_checked_out(repo.clone(), push).await?,
                (None, None) => "HEAD".to_string(),
            };
            self.git(&["worktree", "add", "-b", branch, path, &from], repo)
                .await?;
        }
        Ok((dir, false))
    }

    /// Fetch, from `push`'s remote, its copy of the branch `repo` has
    /// checked out into `refs/pcx/remote/<branch>`, which it returns (a ref
    /// of its own, not `FETCH_HEAD`, which every fetch in the repository
    /// rewrites; the remote may be a URL or a path, so no name of it is in
    /// the ref).
    async fn fetch_checked_out(
        &self,
        repo: Option<SandboxPath>,
        push: &Push,
    ) -> Result<String, ModuleError> {
        push.reachable(self.sandbox.policy())?;
        let checked_out = self
            .git(&["symbolic-ref", "--quiet", "--short", "HEAD"], repo.clone())
            .await
            .map_err(|e| {
                ModuleError::new(
                    ModuleErrorKind::FailedPrecondition,
                    format!(
                        "a new task's branch starts from the remote's copy of the checked-out branch, and the repository has none checked out ({})",
                        e.message
                    ),
                )
            })?;
        let checked_out = revision("checked-out branch", checked_out.trim())?;
        let tracking = format!("refs/pcx/remote/{checked_out}");
        let refspec = format!("+refs/heads/{checked_out}:{tracking}");
        self.git_with(
            &["fetch", "--quiet", &push.remote, &refspec],
            repo,
            push.env(),
        )
        .await?;
        Ok(tracking)
    }

    /// Push the local `branch` of `repo` to the configured remote's branch
    /// of the same name, as it is (never forced); the remote and the commit
    /// it is at now.
    ///
    /// # Errors
    /// `FailedPrecondition` without `push`, without the network grant, or
    /// with the agent's socket under no root; `InvalidArgument` for a branch
    /// the configuration does not allow or that is an option; git failed (the
    /// remote refused it, a hook did).
    pub async fn push(
        &self,
        repo: Option<SandboxPath>,
        branch: &str,
    ) -> Result<(String, String), ModuleError> {
        let push = self.push.ok_or_else(|| {
            ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                "tools-git pushes only when its configuration names push",
            )
        })?;
        let branch = push.allowed(revision("branch", branch)?)?;
        push.reachable(self.sandbox.policy())?;
        let local = format!("refs/heads/{branch}");
        let sha = self
            .git(&["rev-parse", "--verify", &local], repo.clone())
            .await?;
        let refspec = format!("{local}:{local}");
        self.git_with(
            &["push", "--porcelain", &push.remote, &refspec],
            repo,
            push.env(),
        )
        .await?;
        Ok((push.remote.clone(), sha.trim().to_string()))
    }

    /// git's stdout for `args` in `repo` (none: the first root); its stderr
    /// in the error when it fails.
    async fn git(&self, args: &[&str], repo: Option<SandboxPath>) -> Result<String, ModuleError> {
        self.git_with(args, repo, Vec::new()).await
    }

    /// [`Self::git`] with `extra` in its environment.
    async fn git_with(
        &self,
        args: &[&str],
        repo: Option<SandboxPath>,
        extra: Vec<(String, String)>,
    ) -> Result<String, ModuleError> {
        let mut env = process_env(self.path);
        env.extend(extra);
        env.push(("GIT_CONFIG_NOSYSTEM".to_string(), "1".to_string()));
        env.push(("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()));
        let spec = ExecSpec {
            program: GIT.to_string(),
            args: args.iter().map(|a| (*a).to_string()).collect(),
            cwd: repo,
            env,
            stdin: None,
            timeout: GIT_TIMEOUT,
            output_limit: OUTPUT_LIMIT,
        };
        let out = self
            .sandbox
            .exec(spec, CancellationToken::new())
            .await?
            .collect(OUTPUT_LIMIT)
            .await?;
        match out.exit {
            ExecExit::Code(0) => Ok(String::from_utf8_lossy(&out.stdout).into_owned()),
            exit => Err(ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                format!(
                    "git {} ended with {exit}: {}",
                    args.join(" "),
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            )),
        }
    }
}
