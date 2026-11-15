// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A module's memory limit (docs/modules/dynamic.md, limits): `RLIMIT_AS`
//! stops V8 from starting and `rctl` is refused inside a pot, so the
//! launching side reads the processes' memory every second and kills a
//! module past its limit. One reading serves every module of a watcher.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use nix::sys::signal::{Signal, kill, killpg};
use nix::unistd::Pid;
use parking_lot::Mutex;
use tokio::process::Command;

/// How often memory is read.
const EVERY: Duration = Duration::from_secs(1);

/// Whether a helper waits for a module's process, to remove its jail, so it
/// is left alive: FreeBSD's (elsewhere the helper becomes the process).
pub const HELPER_WAITS: bool = cfg!(target_os = "freebsd");

/// `ps(1)`, the same on macOS, Linux and FreeBSD for these fields.
const PS: &str = "/bin/ps";

/// A process being watched.
#[derive(Debug)]
struct Watched {
    limit_kb: u64,
    over: Arc<AtomicBool>,
}

/// Kills what goes past its memory limit.
#[derive(Debug, Default)]
pub struct Watcher {
    watched: Mutex<HashMap<i32, Watched>>,
}

impl Watcher {
    /// A watcher, reading while it is held. Call within a runtime.
    #[must_use]
    pub fn start() -> Arc<Self> {
        let watcher = Arc::new(Self::default());
        let weak = Arc::downgrade(&watcher);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(EVERY).await;
                let Some(watcher) = weak.upgrade() else {
                    return;
                };
                watcher.read().await;
            }
        });
        watcher
    }

    /// Watch the process group `leader` leads, killed past `limit_mb`; the
    /// flag says it was.
    pub fn watch(&self, leader: i32, limit_mb: u32) -> Arc<AtomicBool> {
        let over = Arc::new(AtomicBool::new(false));
        self.watched.lock().insert(
            leader,
            Watched {
                limit_kb: u64::from(limit_mb) * 1024,
                over: Arc::clone(&over),
            },
        );
        over
    }

    /// Stop watching `leader`.
    pub fn forget(&self, leader: i32) {
        self.watched.lock().remove(&leader);
    }

    async fn read(&self) {
        if self.watched.lock().is_empty() {
            return;
        }
        let listed = Command::new(PS)
            .args(["-A", "-o", "pid=", "-o", "ppid=", "-o", "rss="])
            .output()
            .await;
        let table = match listed {
            Ok(listed) => parse(&String::from_utf8_lossy(&listed.stdout)),
            Err(e) => {
                tracing::warn!(error = %e, "the modules' memory could not be read");
                return;
            }
        };
        for (leader, watched) in self.watched.lock().iter() {
            let used: u64 = tree(&table, *leader).iter().map(|(_, kb)| kb).sum();
            if used > watched.limit_kb {
                watched.over.store(true, Ordering::SeqCst);
                let killed = if HELPER_WAITS {
                    tree(&table, *leader)
                        .into_iter()
                        .filter(|(pid, _)| pid != leader)
                        .try_for_each(|(pid, _)| kill(Pid::from_raw(pid), Signal::SIGKILL))
                } else {
                    killpg(Pid::from_raw(*leader), Signal::SIGKILL)
                };
                if let Err(e) = killed {
                    tracing::warn!(error = %e, leader, "kill a module past its memory limit");
                }
            }
        }
    }
}

/// The lines of `ps`: pid, parent and resident kilobytes. A line that is
/// not three numbers is not a process.
fn parse(listing: &str) -> Vec<(i32, i32, u64)> {
    listing
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((
                fields.next()?.parse().ok()?,
                fields.next()?.parse().ok()?,
                fields.next()?.parse().ok()?,
            ))
        })
        .collect()
}

/// `leader` and every process under it, each with its kilobytes.
fn tree(table: &[(i32, i32, u64)], leader: i32) -> Vec<(i32, u64)> {
    let mut children: HashMap<i32, Vec<(i32, u64)>> = HashMap::new();
    let mut found = Vec::new();
    for &(pid, parent, kb) in table {
        if pid == leader {
            found.push((pid, kb));
        }
        children.entry(parent).or_default().push((pid, kb));
    }
    let mut under = vec![leader];
    while let Some(pid) = under.pop() {
        for &(child, kb) in children.get(&pid).into_iter().flatten() {
            found.push((child, kb));
            under.push(child);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_module_counts_with_what_runs_under_it() {
        let listing = "  1 0 100\n 10 1 200\n 11 10 300\n 12 11 400\n 13 1 999\nnot a process\n";
        let table = parse(listing);
        assert_eq!(table.len(), 5);
        let kb = |leader| tree(&table, leader).iter().map(|(_, kb)| kb).sum::<u64>();
        assert_eq!(kb(10), 900);
        assert_eq!(kb(13), 999);
        assert_eq!(kb(77), 0);
    }
}
