// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The work that goes on across a stop of the team (docs/connect.md section
//! 7): a turn's work is kept as going in `team/work` while it runs or waits
//! for its runner, and a team that starts goes on with what is still kept.

use std::time::{Duration, Instant};

use pcx_documents::{Documents, HostDocuments, Item, Typed};
use pcx_module_api::grpc::GrpcChannel;
use pcx_module_api::{ModuleError, Scope};
use post_cortex_proto::board::Stage;
use post_cortex_proto::team::GoingWork;
use tokio_util::sync::CancellationToken;

use crate::adapters::HostWorld;
use crate::domain::{Kind, MODULE_ID, Work};
use crate::ports::Labels;
use crate::wake::{Wake, Why};

/// Longest a look at what is going takes.
const DEADLINE: Duration = Duration::from_secs(30);

/// How long a team that starts waits between looks while memory is not up.
const AGAIN_AFTER: Duration = Duration::from_secs(5);

/// The looks a team that starts takes before it gives up.
const LOOKS: u32 = 60;

/// A call's scope as `author`, in `workspace`, within `within`.
pub fn scope(
    author: Option<String>,
    workspace: &str,
    within: Duration,
    stop: &CancellationToken,
) -> Scope {
    Scope {
        tenant: String::new(),
        profile: String::new(),
        session_id: None,
        workspace_id: Some(workspace.to_string()),
        author,
        caller_module: None,
        call_depth: 0,
        turn: None,
        reach: None,
        role: None,
        read_only: false,
        person: None,
        deadline: Instant::now() + within,
        cancel: stop.child_token(),
    }
}

/// `work`, kept as going for `wake`.
///
/// # Errors
/// Memory failed.
pub async fn mark(
    docs: &dyn Documents,
    work: Item<Work>,
    wake: &Wake,
) -> Result<Item<Work>, ModuleError> {
    let going = GoingWork {
        answers: wake.answers.clone().unwrap_or_default(),
        reached: wake.reach.is_some(),
        reach: wake.reach.clone().unwrap_or_default(),
    };
    let marked = Work {
        going: Some(going),
        ..work.data
    };
    Typed::new(docs, Kind::Work.document())
        .change(
            &work.document.id,
            work.document.version,
            &marked,
            &marked.agent,
        )
        .await
}

/// What goes on after a stop: `work`'s wake, when it is kept as going.
#[must_use]
pub fn wake_of(work: &Work) -> Option<Wake> {
    let going = work.going.as_ref()?;
    Some(Wake {
        member: work.agent.clone(),
        task: work.task.clone(),
        answers: Some(going.answers.clone()).filter(|a| !a.is_empty()),
        why: Why::Resumed,
        reach: going.reached.then(|| going.reach.clone()),
    })
}

/// The work of `docs` kept as going.
///
/// # Errors
/// Memory failed.
pub async fn kept(docs: &dyn Documents) -> Result<Vec<Item<Work>>, ModuleError> {
    let works: Vec<Item<Work>> = Typed::new(docs, Kind::Work.document()).all(None).await?;
    Ok(works
        .into_iter()
        .filter(|w| w.data.going.is_some())
        .collect())
}

/// `work`, no longer kept as going.
///
/// # Errors
/// Memory failed.
pub async fn forget(docs: &dyn Documents, work: Item<Work>) -> Result<(), ModuleError> {
    let ended = Work {
        going: None,
        ..work.data
    };
    Typed::new(docs, Kind::Work.document())
        .change(
            &work.document.id,
            work.document.version,
            &ended,
            &ended.agent,
        )
        .await?;
    Ok(())
}

/// `wake`'s work ended: it is no longer kept as going.
///
/// # Errors
/// Memory failed.
pub async fn clear(
    channel: &GrpcChannel,
    stop: &CancellationToken,
    workspace: &str,
    wake: &Wake,
) -> Result<(), ModuleError> {
    let author = Some(MODULE_ID.to_string());
    let scope = scope(author, workspace, DEADLINE, stop);
    let docs = HostDocuments::new(channel.clone(), &scope, workspace);
    let of = Work {
        agent: wake.member.clone(),
        task: wake.task.clone(),
        ..Work::default()
    };
    let found: Vec<Item<Work>> = Typed::new(&docs, Kind::Work.document())
        .all(Some((&of, &["agent", "task"])))
        .await?;
    for work in found.into_iter().filter(|w| w.data.going.is_some()) {
        forget(&docs, work).await?;
    }
    Ok(())
}

/// What a stop cut short, by workspace, as soon as memory answers; the work
/// on a task closed meanwhile is forgotten. A team that cannot read it says
/// so and starts without it: what it finds stays kept for its next start.
pub async fn cut_short(channel: &GrpcChannel, stop: &CancellationToken) -> Vec<(String, Wake)> {
    for look in 1..=LOOKS {
        match look_for_work(channel, stop).await {
            Ok(found) => return found,
            Err(error) if look == LOOKS => {
                tracing::error!(%error, "the team could not find the work a stop cut short");
            }
            Err(error) => {
                tracing::debug!(%error, "memory does not answer yet: the team looks again");
            }
        }
        tokio::select! {
            () = tokio::time::sleep(AGAIN_AFTER) => {}
            () = stop.cancelled() => break,
        }
    }
    Vec::new()
}

async fn look_for_work(
    channel: &GrpcChannel,
    stop: &CancellationToken,
) -> Result<Vec<(String, Wake)>, ModuleError> {
    let author = Some(MODULE_ID.to_string());
    let every = scope(author.clone(), "", DEADLINE, stop);
    let mut found = Vec::new();
    for workspace in pcx_documents::every_workspace(channel, &every).await? {
        let scope = scope(author.clone(), &workspace, DEADLINE, stop);
        let docs = HostDocuments::new(channel.clone(), &scope, &workspace);
        let world = HostWorld::new(channel.clone(), &scope, &workspace);
        for work in kept(&docs).await? {
            if world.closed(&work.data.task).await? {
                forget(&docs, work).await?;
            } else if let Some(wake) = wake_of(&work.data) {
                found.push((workspace.clone(), wake));
            }
        }
    }
    Ok(found)
}

/// Whether `column` of a board with `columns` and their `stages` closes its
/// tasks: a board without stages reads its last column as done, a column it
/// does not have as closed.
#[must_use]
pub fn column_closed(
    columns: &[String],
    mut stages: impl Iterator<Item = Stage>,
    column: &str,
) -> bool {
    let Some(at) = columns.iter().position(|c| c == column) else {
        return true;
    };
    match stages.nth(at) {
        Some(Stage::Open) => false,
        Some(Stage::Done | Stage::Canceled) => true,
        Some(Stage::Unspecified) | None => at + 1 == columns.len(),
    }
}

#[cfg(test)]
mod tests;
