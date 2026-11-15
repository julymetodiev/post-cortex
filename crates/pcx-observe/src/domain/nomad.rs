// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! How Nomad's answers read as observe's messages (ARCHITECTURE section 8,
//! operations, a Nomad environment): jobs with their allocations and a
//! periodic job's launches, which allocation and task a log is asked of,
//! the end of a log, and a node with its memory and disks. Pure.

use chrono::DateTime;
use post_cortex_proto::observe::{
    AllocInfo, DiskInfo, JobInfo, Launch, NodeInfo, NomadAllocation, NomadDisk, NomadHostStats,
    NomadJob, NomadJobStub, NomadNode, NomadTaskState, TaskInfo,
};

use crate::domain::Refused;

/// Lines of a task's log when the request names none, and the most.
pub const ALLOC_LINES: (u32, u32) = (100, 500);

/// Most allocations a job answers.
const MAX_ALLOCATIONS: usize = 10;

/// Most launches a periodic job answers.
const MAX_LAUNCHES: usize = 5;

/// Most events a task answers.
const MAX_EVENTS: usize = 3;

/// The smallest filesystem that counts as a disk (devfs says 1024 bytes).
const SMALLEST_DISK: u64 = 1024 * 1024;

/// Nomad's nanoseconds since the epoch as RFC 3339; empty for none.
fn time(nanos: i64) -> String {
    if nanos == 0 {
        return String::new();
    }
    DateTime::from_timestamp_nanos(nanos)
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

/// The jobs among `stubs` that are no launch of another, by name.
#[must_use]
pub fn parents(stubs: &[NomadJobStub]) -> Vec<&NomadJobStub> {
    let mut parents: Vec<&NomadJobStub> = stubs.iter().filter(|s| s.parent_id.is_empty()).collect();
    parents.sort_by(|a, b| a.id.cmp(&b.id));
    parents
}

/// A task of an allocation, with its last events.
fn task(name: &str, state: &NomadTaskState) -> TaskInfo {
    let from = state.events.len().saturating_sub(MAX_EVENTS);
    TaskInfo {
        name: name.to_string(),
        state: state.state.clone(),
        failed: state.failed,
        events: state.events[from..]
            .iter()
            .map(|e| format!("{} {}: {}", time(e.time), e.r#type, e.display_message))
            .collect(),
    }
}

/// An allocation, its tasks by name.
fn allocation(allocation: &NomadAllocation) -> AllocInfo {
    let mut tasks: Vec<TaskInfo> = allocation
        .task_states
        .iter()
        .map(|(name, state)| task(name, state))
        .collect();
    tasks.sort_by(|a, b| a.name.cmp(&b.name));
    AllocInfo {
        id: allocation.id.clone(),
        node: allocation.node_name.clone(),
        state: allocation.client_status.clone(),
        version: allocation.job_version,
        restarts: allocation.task_states.values().map(|t| t.restarts).sum(),
        since: time(allocation.create_time),
        tasks,
    }
}

/// The allocations of `job` among `all`, the newest first, at most 10.
fn allocations_of(all: &[NomadAllocation], job: &str) -> Vec<AllocInfo> {
    let mut its: Vec<&NomadAllocation> = all.iter().filter(|a| a.job_id == job).collect();
    its.sort_by_key(|a| std::cmp::Reverse(a.create_time));
    its.into_iter()
        .take(MAX_ALLOCATIONS)
        .map(allocation)
        .collect()
}

/// Nomad's jobs as observe answers them: each job of `stubs` that is no
/// launch, with what `details` says of it (its version and schedule; a job
/// it lacks answers without them), its allocations among `allocations`,
/// and for a periodic job its launches among `stubs`, the newest first.
#[must_use]
pub fn jobs(
    stubs: &[NomadJobStub],
    details: &[NomadJob],
    allocations: &[NomadAllocation],
) -> Vec<JobInfo> {
    parents(stubs)
        .into_iter()
        .map(|stub| {
            let detail = details.iter().find(|d| d.id == stub.id);
            let periodic = detail
                .and_then(|d| d.periodic.as_ref())
                .filter(|p| p.enabled);
            let schedule = match periodic {
                Some(p) if p.specs.is_empty() && !p.spec.is_empty() => vec![p.spec.clone()],
                Some(p) => p.specs.clone(),
                None => Vec::new(),
            };
            let mut launched: Vec<&NomadJobStub> =
                stubs.iter().filter(|s| s.parent_id == stub.id).collect();
            launched.sort_by_key(|launch| std::cmp::Reverse(launch.submit_time));
            JobInfo {
                name: stub.id.clone(),
                r#type: stub.r#type.clone(),
                status: stub.status.clone(),
                version: detail.map_or(0, |d| d.version),
                submitted: time(stub.submit_time),
                stable: detail.is_some_and(|d| d.stable),
                stopped: detail.is_some_and(|d| d.stop),
                allocations: allocations_of(allocations, &stub.id),
                schedule,
                time_zone: periodic.map(|p| p.time_zone.clone()).unwrap_or_default(),
                launches: launched
                    .into_iter()
                    .take(MAX_LAUNCHES)
                    .map(|launch| Launch {
                        name: launch.id.clone(),
                        launched: time(launch.submit_time),
                        status: launch.status.clone(),
                        allocations: allocations_of(allocations, &launch.id),
                    })
                    .collect(),
            }
        })
        .collect()
}

/// The one allocation among `found` whose id starts with `asked`.
///
/// # Errors
/// None starts so, or several do.
pub fn allocation_of(found: Vec<NomadAllocation>, asked: &str) -> Result<NomadAllocation, Refused> {
    let mut its: Vec<NomadAllocation> = found
        .into_iter()
        .filter(|a| a.id.starts_with(asked))
        .collect();
    match (its.pop(), its.len()) {
        (None, _) => Err(Refused::NoAllocation(asked.to_string())),
        (Some(only), 0) => Ok(only),
        (Some(_), others) => Err(Refused::Allocations {
            start: asked.to_string(),
            count: others + 1,
        }),
    }
}

/// The task of `allocation` a log is asked of: the one `asked` names, or
/// its only one when `asked` is empty. An allocation that says no tasks
/// yet is asked of the task named.
///
/// # Errors
/// It names none and the allocation has several, or one it does not have.
pub fn task_of(allocation: &NomadAllocation, asked: &str) -> Result<String, Refused> {
    let mut tasks: Vec<String> = allocation.task_states.keys().cloned().collect();
    tasks.sort();
    match (asked, tasks.as_slice()) {
        ("", []) => Err(Refused::Empty("task")),
        ("", [only]) => Ok(only.clone()),
        (asked, tasks) if tasks.is_empty() || tasks.iter().any(|t| t == asked) => {
            Ok(asked.to_string())
        }
        _ => Err(Refused::Tasks(tasks)),
    }
}

/// The last `lines` lines of `text`, the end of a log; `cut` when `text`
/// starts inside a line, which is then left out.
#[must_use]
pub fn tail(text: &str, lines: u32, cut: bool) -> String {
    let mut all: Vec<&str> = text.lines().collect();
    if cut && all.len() > 1 {
        all.remove(0);
    }
    let from = all.len().saturating_sub(lines as usize);
    all[from..].join("\n")
}

/// A disk of `size` bytes with `used` of them used.
fn disk(name: &str, size: u64, used: u64) -> DiskInfo {
    let percent = used
        .saturating_mul(100)
        .checked_div(size)
        .map_or(0, |p| u32::try_from(p).unwrap_or(u32::MAX));
    DiskInfo {
        name: name.to_string(),
        size,
        used,
        used_percent: percent,
    }
}

/// A ZFS pool as its datasets say it.
struct Pool<'a> {
    name: &'a str,
    /// What its datasets use together.
    used: u64,
    /// What is available in it: the pool's own dataset says it, else the
    /// dataset with the least (one with a reservation has more).
    available: u64,
    /// Whether the pool's own dataset said `available`.
    own: bool,
}

/// A host's disks among its mounted filesystems: each device (`/dev/...`)
/// once, and each ZFS pool once, its datasets (`pool/a/b`) together: what
/// they use, of that and what is available in the pool. A directory
/// mounted again (nullfs, its device a path) and a filesystem smaller than
/// a mebibyte (devfs) are none.
fn disks(mounted: &[NomadDisk]) -> Vec<DiskInfo> {
    let mut devices: Vec<DiskInfo> = Vec::new();
    let mut pools: Vec<Pool<'_>> = Vec::new();
    for m in mounted.iter().filter(|m| m.size >= SMALLEST_DISK) {
        if m.device.starts_with("/dev/") {
            if devices.iter().all(|d| d.name != m.device) {
                devices.push(disk(&m.device, m.size, m.used));
            }
            continue;
        }
        if m.device.starts_with('/') {
            continue;
        }
        let name = m
            .device
            .split_once('/')
            .map_or(m.device.as_str(), |(pool, _)| pool);
        let own = m.device == name;
        match pools.iter_mut().find(|p| p.name == name) {
            Some(pool) => {
                pool.used = pool.used.saturating_add(m.used);
                if own {
                    pool.available = m.available;
                    pool.own = true;
                } else if !pool.own {
                    pool.available = pool.available.min(m.available);
                }
            }
            None => pools.push(Pool {
                name,
                used: m.used,
                available: m.available,
                own,
            }),
        }
    }
    devices.extend(
        pools
            .iter()
            .map(|p| disk(p.name, p.used.saturating_add(p.available), p.used)),
    );
    devices
}

/// A node, with its memory and disks from `stats`, or why they could not
/// be read.
#[must_use]
pub fn node(node: &NomadNode, stats: Result<NomadHostStats, String>) -> NodeInfo {
    let (stats, unread) = match stats {
        Ok(stats) => (stats, String::new()),
        Err(why) => (NomadHostStats::default(), why),
    };
    let memory = stats.memory.unwrap_or_default();
    NodeInfo {
        name: node.name.clone(),
        status: node.status.clone(),
        eligible: node.scheduling_eligibility == "eligible",
        draining: node.drain,
        version: node.version.clone(),
        memory: memory.total,
        memory_available: memory.available,
        disks: disks(&stats.disk_stats),
        unread,
    }
}

#[cfg(test)]
mod tests;
