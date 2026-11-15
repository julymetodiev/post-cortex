// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The chief's service (ARCHITECTURE section 8, the chief): a question
//! asked in a conversation, its turn run under the day's budget, each tool
//! call of the turn recorded from its events (and passed on to who watches
//! them), the turn's tokens added to the day; what its view reads. Between
//! the handlers and the ports.

use pcx_documents::{Documents, Item, Sessions, Typed};
use pcx_module_api::agent::{TurnEnd, TurnEvent, TurnRequest, Turns};
use pcx_module_api::{ModuleError, ModuleErrorKind, Scope};
use post_cortex_proto::chief::Rights;
use tokio::sync::mpsc;

use crate::domain::{self, Action, Called, Calls, Day, Kind, Refused, Talk, TalkRules};
use crate::ports::Clock;
use crate::settings::Settings;

/// Events a turn sends before it waits for the chief to record them.
const EVENTS: usize = 64;

/// Times a day's use is read again when another turn added to it first.
const ADD_TRIES: u32 = 5;

/// Who asks the chief.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Asker<'a> {
    /// Their author name.
    pub author: &'a str,
    /// The module they asked through; empty: a client of the host.
    pub channel: &'a str,
}

/// Which conversation a question is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conversation<'a> {
    /// A new one, in a new session.
    New,
    /// The one of this id.
    Talk(&'a str),
    /// The one of this session (a client's `RunTurn`): made on its first
    /// turn.
    Session(&'a str),
}

/// What a question got.
#[derive(Debug, Clone, PartialEq)]
pub struct Asked {
    /// The conversation's id, to go on with it.
    pub conversation: String,
    /// How its turn ended, with the chief's last text.
    pub end: TurnEnd,
}

fn refused(kind: ModuleErrorKind, why: &Refused) -> ModuleError {
    ModuleError::new(kind, why.to_string())
}

/// The chief over its ports.
pub struct Chief<'a> {
    /// Its conversations and actions, in the workspace it is asked from.
    pub docs: &'a dyn Documents,
    /// New sessions there.
    pub sessions: &'a dyn Sessions,
    /// Its days' use, in the workspace its settings name: one budget for
    /// the host, whichever workspace it is asked from.
    pub days: &'a dyn Documents,
    /// The agent's turns.
    pub turns: &'a dyn Turns,
    /// The day.
    pub clock: &'a dyn Clock,
    /// Its settings.
    pub settings: &'a Settings,
    /// Its scope: its name as the author, its workspace.
    pub scope: &'a Scope,
}

impl Chief<'_> {
    /// Ask `question` as `asker`, in `conversation`; the turn's end when
    /// it ends. Its events go to `watcher` too, when one watches (one that
    /// left does not stop the turn).
    ///
    /// # Errors
    /// `ResourceExhausted` when today's budget is spent; `NotFound` for no
    /// such conversation, `Denied` for one that is not `asker`'s from this
    /// channel; the agent failed; a tool call or the day's use could not be
    /// recorded (after the turn ran).
    pub async fn ask(
        &self,
        asker: Asker<'_>,
        question: &str,
        conversation: Conversation<'_>,
        watcher: Option<&mpsc::Sender<TurnEvent>>,
    ) -> Result<Asked, ModuleError> {
        let today = self.clock.today();
        let used = self.used(&today).await?;
        domain::within(used, self.settings.budget)
            .map_err(|why| refused(ModuleErrorKind::ResourceExhausted, &why))?;
        let talk = self.talk(asker, question, conversation).await?;
        let talk = self.said(talk, true, String::new()).await?;
        let request = TurnRequest {
            instructions: self.settings.instructions.clone(),
            model: self.settings.model.clone(),
            // Its conversations are the streams it speaks in, each question
            // written as who asked it (W25).
            speaker: Some(domain::MODULE_ID.to_string()),
            asker: Some(asker.author.to_string()),
            // Asked from no workspace, a call that names none (`bash`) runs in its own.
            workspace: Some(self.settings.workspace.clone()),
            ..TurnRequest::new(talk.data.session_id.clone(), question)
        };
        let (events, mut told) = mpsc::channel(EVENTS);
        let run = async move {
            let end = self.turns.run(self.scope, &request, &events).await;
            drop(events);
            end
        };
        let talk_id = talk.document.id.as_str();
        let record = async move {
            let mut calls = Calls::default();
            let mut failed = None;
            while let Some(event) = told.recv().await {
                if let Some(watcher) = watcher
                    && watcher.send(event.clone()).await.is_err()
                {
                    tracing::debug!("the chief's turn lost its watcher; the turn goes on");
                }
                if let Some(called) = calls.take(event)
                    && let Err(e) = self.record(talk_id, asker.author, called).await
                {
                    failed.get_or_insert(e);
                }
            }
            (calls, failed)
        };
        let (end, (calls, mut failed)) = tokio::join!(run, record);
        for called in calls.unanswered() {
            if let Err(e) = self.record(talk_id, asker.author, called).await {
                failed.get_or_insert(e);
            }
        }
        let said = match &end {
            Ok(end) => end.answer.clone(),
            Err(e) => format!("the turn failed: {e}"),
        };
        let talk = self.said(talk, false, said).await?;
        let end = end?;
        self.add(&today, domain::spent(&end.usage)).await?;
        if let Some(e) = failed {
            return Err(e);
        }
        Ok(Asked {
            conversation: talk.document.id,
            end,
        })
    }

    /// `talk` with whether a turn of it runs and what the last one ended with,
    /// which whoever lost the connection reads (`chief_talks`).
    async fn said(
        &self,
        talk: Item<Talk>,
        answering: bool,
        answer: String,
    ) -> Result<Item<Talk>, ModuleError> {
        let data = Talk {
            answering,
            answer,
            ..talk.data
        };
        self.talk_docs()
            .change(
                &talk.document.id,
                talk.document.version,
                &data,
                &talk.document.text,
            )
            .await
    }

    /// Its rights and today's use.
    ///
    /// # Errors
    /// Memory failed.
    pub async fn rights(&self) -> Result<Rights, ModuleError> {
        Ok(Rights {
            budget_tokens: self.settings.budget,
            used_today: self.used(&self.clock.today()).await?,
            person_tools: self.settings.grant.person_tools.clone(),
            withheld: self.settings.grant.withheld.clone(),
        })
    }

    /// Its conversations, the newest first, at most `limit` (0: 50).
    ///
    /// # Errors
    /// Memory failed.
    pub async fn talks(&self, limit: u32) -> Result<Vec<Item<Talk>>, ModuleError> {
        let (talks, _) = self
            .talk_docs()
            .after::<Talk>(None, None, listed(limit), |_| true)
            .await?;
        Ok(talks)
    }

    /// The actions of conversation `talk` (none: of every conversation, W25),
    /// the newest first, at most `limit` (0: 50).
    ///
    /// # Errors
    /// Memory failed.
    pub async fn actions(
        &self,
        talk: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Item<Action>>, ModuleError> {
        let of = Action {
            talk: talk.unwrap_or_default().to_string(),
            ..Action::default()
        };
        let matching = talk.map(|_| (&of, &["talk"][..]));
        let (actions, _) = Typed::new(self.docs, Kind::Action.document())
            .after(matching, None, listed(limit), |_| true)
            .await?;
        Ok(actions)
    }

    /// Action `id`.
    ///
    /// # Errors
    /// `NotFound` for no such action; memory failed.
    pub async fn action(&self, id: &str) -> Result<Item<Action>, ModuleError> {
        Typed::new(self.docs, Kind::Action.document()).get(id).await
    }

    /// The conversation `conversation` names, checked against `asker`, or
    /// a new one.
    async fn talk(
        &self,
        asker: Asker<'_>,
        question: &str,
        conversation: Conversation<'_>,
    ) -> Result<Item<Talk>, ModuleError> {
        let found: Option<Item<Talk>> = match conversation {
            Conversation::New => None,
            Conversation::Talk(id) => Some(self.talk_docs().get(id).await?),
            Conversation::Session(session) => {
                let of = Talk {
                    session_id: session.to_string(),
                    ..Talk::default()
                };
                self.talk_docs()
                    .all(Some((&of, &["session_id"])))
                    .await?
                    .into_iter()
                    .next()
            }
        };
        if let Some(talk) = found {
            talk.data
                .continued_by(asker.author, asker.channel)
                .map_err(|why| refused(ModuleErrorKind::Denied, &why))?;
            return Ok(talk);
        }
        let session_id = match conversation {
            Conversation::Session(session) => session.to_string(),
            Conversation::New | Conversation::Talk(_) => {
                self.sessions
                    .create(&format!("{} with {}", self.settings.name, asker.author))
                    .await?
            }
        };
        let talk = Talk {
            asked_by: asker.author.to_string(),
            channel: asker.channel.to_string(),
            session_id,
            ..Talk::default()
        };
        self.talk_docs().create(&talk, question).await
    }

    /// Record `called` in conversation `talk`.
    async fn record(&self, talk: &str, asked_by: &str, called: Called) -> Result<(), ModuleError> {
        let action = called.action(talk, asked_by).map_err(|e| {
            ModuleError::new(
                ModuleErrorKind::InvalidArgument,
                format!("a tool call's input is not an object: {e}"),
            )
        })?;
        let text = action.tool.clone();
        Typed::new(self.docs, Kind::Action.document())
            .create(&action, &text)
            .await
            .map(|_| ())
    }

    /// The tokens the turns of `date` used.
    async fn used(&self, date: &str) -> Result<u64, ModuleError> {
        let days = self.days(date).await?;
        Ok(days
            .iter()
            .fold(0, |sum, day| sum.saturating_add(day.data.tokens)))
    }

    /// Add `tokens` to `date`'s use; a day another turn changed first is
    /// read again.
    async fn add(&self, date: &str, tokens: u64) -> Result<(), ModuleError> {
        let kind = Typed::new(self.days, Kind::Day.document());
        let mut tries = 0;
        loop {
            tries += 1;
            let Some(day) = self.days(date).await?.into_iter().next() else {
                let day = Day {
                    date: date.to_string(),
                    tokens,
                };
                return kind.create(&day, "").await.map(|_| ());
            };
            let next = Day {
                tokens: day.data.tokens.saturating_add(tokens),
                ..day.data
            };
            match kind
                .change(&day.document.id, day.document.version, &next, "")
                .await
            {
                Ok(_) => return Ok(()),
                Err(e) if e.kind == ModuleErrorKind::Conflict && tries < ADD_TRIES => {}
                Err(e) => return Err(e),
            }
        }
    }

    /// The documents of `date`'s use (one, unless two turns made it at
    /// once).
    async fn days(&self, date: &str) -> Result<Vec<Item<Day>>, ModuleError> {
        let of = Day {
            date: date.to_string(),
            ..Day::default()
        };
        Typed::new(self.days, Kind::Day.document())
            .all(Some((&of, &["date"])))
            .await
    }

    fn talk_docs(&self) -> Typed<'_> {
        Typed::new(self.docs, Kind::Talk.document())
    }
}

/// Entries a list gives when it names no limit.
const LISTED: usize = 50;

/// Entries a list gives at most.
const MAX_LISTED: usize = 500;

/// The entries a list of `limit` gives.
fn listed(limit: u32) -> usize {
    match limit {
        0 => LISTED,
        n => usize::try_from(n).map_or(MAX_LISTED, |n| n.min(MAX_LISTED)),
    }
}

#[cfg(test)]
mod tests;
