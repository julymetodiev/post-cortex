// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The scheduler's domain (ARCHITECTURE section 8, operations, W28): what
//! a job may be, when it is due (the latest of its times since its anchor,
//! once, however many passed), when it is due next, the task a run adds
//! and how a script's end reads. Pure: no host, no clock (time is a
//! parameter).

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Duration, TimeZone, Utc};
use chrono_tz::Tz;
use croner::Cron as Pattern;
use post_cortex_proto::board::AddTaskRequest;
use post_cortex_proto::scheduler::job::{Does, When};
use post_cortex_proto::scheduler::{Cron, Job, Outcome, TaskTemplate};

/// The module's id.
pub const MODULE_ID: &str = "scheduler";

/// The fewest seconds between two runs of an interval job.
pub const MIN_EVERY: u32 = 60;

/// A script's timeout when its job names none, in seconds.
pub const DEFAULT_TIMEOUT: u32 = 300;

/// The longest a script may run, in seconds.
const MAX_TIMEOUT: u32 = 3_600;

/// Bytes of a script's output a run keeps.
pub const OUTPUT_TAIL: usize = 4_000;

/// The kinds of documents the scheduler keeps, `scheduler/<name>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A job.
    Job,
    /// A job's run.
    Run,
}

impl Kind {
    /// Its name in the module.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Job => "job",
            Self::Run => "run",
        }
    }

    /// Its full name, `scheduler/<name>`, as memory keeps it.
    #[must_use]
    pub fn document(self) -> String {
        format!("{MODULE_ID}/{}", self.name())
    }

    /// Every kind.
    pub const ALL: [Self; 2] = [Self::Job, Self::Run];
}

/// A rule of the scheduler a job breaks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Broken {
    /// Its name is not 1 to 64 of a-z, 0-9, '-'.
    Name,
    /// Another job has its name.
    Taken(String),
    /// It says not when it runs.
    NoWhen,
    /// Its cron expression is not five fields a cron reads.
    Cron(String),
    /// Its time zone is not an IANA one.
    TimeZone(String),
    /// It runs more often than every 60 seconds.
    TooOften(u32),
    /// It says not what it does.
    NoDoes,
    /// Its script names no program.
    NoProgram,
    /// Its script's timeout is past an hour.
    LongTimeout(u32),
    /// Its task lacks a field it needs.
    Task(&'static str),
    /// Its workflow's name is not one.
    Workflow,
}

impl fmt::Display for Broken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Name => write!(
                f,
                "a job's name is 1 to {} of a-z, 0-9 and '-'",
                pcx_module_api::name::NAME_CHARS
            ),
            Self::Taken(name) => write!(f, "a job is named {name} already"),
            Self::NoWhen => f.write_str("a job says when it runs: a cron or every_seconds"),
            Self::Cron(why) => write!(f, "the cron expression is not one of five fields: {why}"),
            Self::TimeZone(zone) => write!(f, "{zone} is not an IANA time zone"),
            Self::TooOften(every) => write!(
                f,
                "a job runs at most every {MIN_EVERY} seconds, not every {every}"
            ),
            Self::NoDoes => f.write_str("a job says what it does: a script, a task or a workflow"),
            Self::NoProgram => f.write_str("a script names its program"),
            Self::LongTimeout(seconds) => write!(
                f,
                "a script runs at most {MAX_TIMEOUT} seconds, not {seconds}"
            ),
            Self::Task(field) => write!(f, "a job's task needs its {field}"),
            Self::Workflow => write!(
                f,
                "a job's workflow is named by 1 to {} of a-z, 0-9 and '-'",
                pcx_module_api::name::NAME_CHARS
            ),
        }
    }
}

impl std::error::Error for Broken {}

/// `cron`'s pattern and its time zone.
fn pattern(cron: &Cron) -> Result<(Pattern, Tz), Broken> {
    let fields = cron.expression.split_whitespace().count();
    if fields != 5 {
        return Err(Broken::Cron(format!("it has {fields} fields")));
    }
    let pattern = Pattern::from_str(&cron.expression).map_err(|e| Broken::Cron(e.to_string()))?;
    let zone = match cron.time_zone.trim() {
        "" => Tz::UTC,
        zone => Tz::from_str(zone).map_err(|_| Broken::TimeZone(zone.to_string()))?,
    };
    Ok((pattern, zone))
}

/// Whether `job` may be kept: its name, when it runs and what it does.
///
/// # Errors
/// The first rule it breaks.
pub fn check(job: &Job) -> Result<(), Broken> {
    if !pcx_module_api::name::is_slug(&job.name) {
        return Err(Broken::Name);
    }
    match &job.when {
        None => return Err(Broken::NoWhen),
        Some(When::Cron(cron)) => {
            pattern(cron)?;
        }
        Some(When::EverySeconds(seconds)) => {
            every(*seconds)?;
        }
    }
    match &job.does {
        None => Err(Broken::NoDoes),
        Some(Does::Script(script)) if script.program.trim().is_empty() => Err(Broken::NoProgram),
        Some(Does::Script(script)) if script.timeout_seconds > MAX_TIMEOUT => {
            Err(Broken::LongTimeout(script.timeout_seconds))
        }
        Some(Does::Script(_)) => Ok(()),
        Some(Does::Task(task)) => check_task(task),
        Some(Does::Workflow(workflow)) if !pcx_module_api::name::is_slug(&workflow.name) => {
            Err(Broken::Workflow)
        }
        Some(Does::Workflow(_)) => Ok(()),
    }
}

fn check_task(task: &TaskTemplate) -> Result<(), Broken> {
    if task.workspace_id.trim().is_empty() {
        return Err(Broken::Task("workspace_id"));
    }
    if task.board.trim().is_empty() {
        return Err(Broken::Task("board"));
    }
    if task.title.trim().is_empty() {
        return Err(Broken::Task("title"));
    }
    Ok(())
}

/// Whether `name` is free among `jobs` (their ids and names), but for the
/// job `id` itself.
///
/// # Errors
/// Another job has it.
pub fn free<'a>(
    name: &str,
    id: Option<&str>,
    mut jobs: impl Iterator<Item = (&'a str, &'a str)>,
) -> Result<(), Broken> {
    if jobs.any(|(other, taken)| taken == name && Some(other) != id) {
        Err(Broken::Taken(name.to_string()))
    } else {
        Ok(())
    }
}

/// How often an interval job runs, in seconds.
fn every(seconds: u32) -> Result<i64, Broken> {
    if seconds < MIN_EVERY {
        Err(Broken::TooOften(seconds))
    } else {
        Ok(i64::from(seconds))
    }
}

/// The latest of `job`'s times after `anchor` and at most `now`: the one
/// run it is due, however many of its times passed; none when it is
/// paused, not due, or its cron finds no such time.
///
/// # Errors
/// A job the rules refuse (one written before a rule).
pub fn due(
    job: &Job,
    anchor: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, Broken> {
    if job.paused || now <= anchor {
        return Ok(None);
    }
    match job.when.as_ref().ok_or(Broken::NoWhen)? {
        When::EverySeconds(seconds) => {
            let every = every(*seconds)?;
            let passed = (now - anchor).num_seconds() / every;
            Ok((passed > 0).then(|| anchor + Duration::seconds(passed * every)))
        }
        When::Cron(cron) => {
            let (pattern, zone) = pattern(cron)?;
            // A cron that matches no time before now (a 30 February) is
            // never due.
            match pattern.find_previous_occurrence(&now.with_timezone(&zone), true) {
                Ok(latest) => {
                    let latest = latest.with_timezone(&Utc);
                    Ok((latest > anchor).then_some(latest))
                }
                Err(_) => Ok(None),
            }
        }
    }
}

/// When `job` is due next after `now`; none when it is paused or its cron
/// finds no such time.
///
/// # Errors
/// A job the rules refuse.
pub fn next(
    job: &Job,
    anchor: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, Broken> {
    if job.paused {
        return Ok(None);
    }
    match job.when.as_ref().ok_or(Broken::NoWhen)? {
        When::EverySeconds(seconds) => {
            let every = every(*seconds)?;
            let passed = (now - anchor).num_seconds().max(0) / every;
            Ok(Some(anchor + Duration::seconds((passed + 1) * every)))
        }
        When::Cron(cron) => {
            let (pattern, zone) = pattern(cron)?;
            match pattern.find_next_occurrence(&now.with_timezone(&zone), false) {
                Ok(next) => Ok(Some(next.with_timezone(&Utc))),
                // A cron that matches no later time is never due again.
                Err(_) => Ok(None),
            }
        }
    }
}

/// Seconds after a time of its schedule within which a job's run is on
/// time for who watches it from outside.
const LATE_AFTER: i64 = 900;

/// How a job stands for who watches it from outside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    /// It runs on time.
    Ok,
    /// It is paused.
    Paused,
    /// No run of it succeeded or still runs since this time of its
    /// schedule, more than 15 minutes ago.
    Late(DateTime<Utc>),
}

impl fmt::Display for Health {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ok => f.write_str("it runs on time"),
            Self::Paused => f.write_str("it is paused"),
            Self::Late(since) => write!(
                f,
                "no run of it succeeded since it was due at {}",
                since.format("%Y-%m-%dT%H:%M:%SZ")
            ),
        }
    }
}

/// The time of its schedule a run of `job`, last changed at `changed`, is
/// owed for at `now`: the latest that passed more than 15 minutes ago; none
/// when no time of it is that old.
///
/// # Errors
/// A job the rules refuse.
pub fn owed(
    job: &Job,
    changed: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, Broken> {
    due(job, changed, now - Duration::seconds(LATE_AFTER))
}

/// How `job`, last changed at `changed`, stands at `now`, from its `runs`
/// (when each was due, or started when run by hand, and how it ended):
/// paused; late when a time of its schedule passed more than 15 minutes ago
/// and no run since that time succeeded or still runs (a run that failed
/// or was skipped is no run of that time); else on time.
///
/// # Errors
/// A job the rules refuse.
pub fn health(
    job: &Job,
    changed: DateTime<Utc>,
    runs: impl IntoIterator<Item = (DateTime<Utc>, Outcome)>,
    now: DateTime<Utc>,
) -> Result<Health, Broken> {
    if job.paused {
        return Ok(Health::Paused);
    }
    let Some(owed) = owed(job, changed, now)? else {
        return Ok(Health::Ok);
    };
    let ran = runs
        .into_iter()
        .any(|(at, outcome)| at >= owed && matches!(outcome, Outcome::Ok | Outcome::Running));
    Ok(if ran { Health::Ok } else { Health::Late(owed) })
}

/// The task a run of job `name` due at `due` (none: run by hand at `now`)
/// adds: its key names the job and the time, so the run adds it once.
#[must_use]
pub fn task(template: &TaskTemplate, name: &str, at: DateTime<Utc>) -> AddTaskRequest {
    AddTaskRequest {
        workspace_id: template.workspace_id.clone(),
        board: template.board.clone(),
        title: template.title.clone(),
        text: template.text.clone(),
        assignee: Some(template.assignee.clone()).filter(|a| !a.trim().is_empty()),
        labels: template.labels.clone(),
        key: Some(format!("{MODULE_ID}:{name}:{}", at.timestamp())),
        ..AddTaskRequest::default()
    }
}

/// How a script ended, as a run reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// It exited with this code.
    Code(i32),
    /// It ran past its timeout.
    TimedOut,
    /// A signal or a cancel ended it.
    Killed,
}

/// The outcome, the exit code and the result of a script that `ended`
/// with `output` (its stdout then its stderr).
#[must_use]
pub fn script_end(ended: Ended, output: &str) -> (Outcome, Option<i32>, String) {
    let tail = tail(output);
    match ended {
        Ended::Code(0) => (Outcome::Ok, Some(0), tail),
        Ended::Code(code) => (Outcome::Failed, Some(code), tail),
        Ended::TimedOut => (Outcome::Failed, None, format!("timed out\n{tail}")),
        Ended::Killed => (Outcome::Failed, None, format!("killed\n{tail}")),
    }
}

/// The last [`OUTPUT_TAIL`] bytes of `output`, cut at a character.
fn tail(output: &str) -> String {
    let mut from = output.len().saturating_sub(OUTPUT_TAIL);
    while !output.is_char_boundary(from) {
        from += 1;
    }
    output[from..].trim().to_string()
}

/// `seconds` and `nanos` since the epoch as a time; none for a time out
/// of range or negative nanoseconds (no time memory writes).
#[must_use]
pub fn at(seconds: i64, nanos: i32) -> Option<DateTime<Utc>> {
    match u32::try_from(nanos) {
        Ok(nanos) => Utc.timestamp_opt(seconds, nanos).single(),
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests;
