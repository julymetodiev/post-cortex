// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! An `agent` block's turn and its answer read (docs/modules/workflow.md,
//! blocks): the agent ends its turn with one JSON object of the block's
//! output `Schema`. An answer that is not read is asked for once more in
//! the turn's session, told why; one not read again fails the block.

use std::collections::BTreeMap;

use pcx_expr::Value as Expr;
use pcx_expr::schema::Field;
use post_cortex_proto::workflow::Failure;

use crate::domain::{Act, failure, kinds};
use crate::ports::{Said, Whose};
use crate::service::engine::act::{Actor, Ended};

/// The characters of an agent's answer a failed step shows.
const SHOWN_CHARS: usize = 300;

impl Actor {
    /// A turn of `member` for `prompt`, its answer read by `output`.
    ///
    /// # Errors
    /// The turn failed, or took more than the block's timeout.
    pub async fn turn(
        &self,
        act: &Act,
        member: &str,
        prompt: &str,
        output: &[Field],
    ) -> Result<Ended, Failure> {
        let turns = &self.engine.ports.turns;
        let session = self
            .timed(
                act,
                turns.session(&format!(
                    "Run {} of {}, {}",
                    self.run, self.workflow.name, act.block
                )),
            )
            .await?;
        let schema = pcx_expr::schema::json_schema(output);
        let asked = format!(
            "{prompt}\n\nEnd with your answer as one JSON object, nothing after it, of this JSON Schema:\n{schema}"
        );
        // The agent's own turn has the tools of the workflow's grants, which
        // the check holds to at least one module for such a block.
        let whose = if member.is_empty() {
            Whose::Agent(
                self.workflow
                    .grants
                    .as_ref()
                    .map_or(&[][..], |g| &g.modules[..]),
            )
        } else {
            Whose::Member(member)
        };
        let said = self.say(act, &session, whose, &asked).await?;
        let why = match fields(output, &said.answer) {
            Ok(read) => return Ok(Ended::Succeeded(Expr::map(read), "next".into())),
            Err(why) => why,
        };
        // The session holds what the agent found and said: it is asked only
        // to write its answer again.
        let again = format!(
            "Your answer was not read: {why}.\n\nWrite it again as one JSON object and nothing else, of this JSON Schema (inside a JSON string a line break is written \\n):\n{schema}"
        );
        let said = self.say(act, &session, whose, &again).await?;
        Ok(match fields(output, &said.answer) {
            Ok(read) => Ended::Succeeded(Expr::map(read), "next".into()),
            Err(why) => {
                // What it said, for the step's failure: its first characters.
                let shown: String = said.answer.chars().take(SHOWN_CHARS).collect();
                Ended::Failed(failure(
                    kinds::ANSWER,
                    &format!("{why}, asked twice: \"{shown}\""),
                ))
            }
        })
    }

    /// One turn of `whose` in `session` for `prompt`, its tokens added to
    /// what the run spent.
    async fn say(
        &self,
        act: &Act,
        session: &str,
        whose: Whose<'_>,
        prompt: &str,
    ) -> Result<Said, Failure> {
        let turns = &self.engine.ports.turns;
        let said = self
            .timed(act, turns.turn(&self.scope, session, whose, prompt))
            .await?;
        self.spent(said.tokens)
            .await
            .map_err(|e| failure(e.kind.name(), &e.message))?;
        Ok(said)
    }
}

/// The fields of `output` in an agent's `answer`: its JSON object, from its
/// first `{` to its last `}`, read by the schema; else why it is not read.
fn fields(output: &[Field], answer: &str) -> Result<BTreeMap<String, Expr>, String> {
    let object = match (answer.find('{'), answer.rfind('}')) {
        (Some(start), Some(end)) if start < end => &answer[start..=end],
        _ => return Err("the agent's answer has no JSON object".into()),
    };
    let json = serde_json::from_str(object)
        .map_err(|e| format!("the agent's answer is not JSON ({e})"))?;
    pcx_expr::schema::read_answer(output, &json)
}
