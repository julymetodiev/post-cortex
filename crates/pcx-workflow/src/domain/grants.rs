// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A workflow's grants and the ones a person approved
//! (docs/modules/workflow.md, triggers and approval): the modules it names,
//! the team members its `agent` blocks name and the tools of effect
//! `external` its `tool` blocks call. A run starts only within
//! what was approved, a write past it asks the board once, and what was
//! approved or asked is the module's, never the request's.

use std::collections::BTreeSet;

use pcx_module_api::Effect;
use post_cortex_proto::workflow::{Approved, Asked, Grants, Graph, Workflow, block::Kind};

use crate::domain::Found;

/// The members the `agent` blocks of `graph` name, its bodies and branches
/// included.
fn named(graph: &Graph, members: &mut BTreeSet<String>) {
    for block in &graph.blocks {
        match &block.kind {
            Some(Kind::Agent(agent)) if !agent.member.is_empty() => {
                members.insert(agent.member.clone());
            }
            Some(Kind::ForEach(each)) => {
                if let Some(body) = &each.body {
                    named(body, members);
                }
            }
            Some(Kind::Parallel(parallel)) => {
                for branch in &parallel.branches {
                    named(branch, members);
                }
            }
            _ => {}
        }
    }
}

/// Whether an `agent` block of `graph` asks the agent's own turn (it names
/// no member), its bodies and branches included.
fn asks_own(graph: &Graph) -> bool {
    graph.blocks.iter().any(|block| match &block.kind {
        Some(Kind::Agent(agent)) => agent.member.is_empty(),
        Some(Kind::ForEach(each)) => each.body.as_ref().is_some_and(asks_own),
        Some(Kind::Parallel(parallel)) => parallel.branches.iter().any(asks_own),
        _ => false,
    })
}

/// Whether `workflow` asks the agent's own turn: an `agent` block of it
/// names no member.
#[must_use]
pub fn own_turn(workflow: &Workflow) -> bool {
    workflow.graph.as_ref().is_some_and(asks_own)
}

/// What `workflow` needs a person's word for: the modules its grants name,
/// the members its graph names and the tools of effect `external` it was
/// kept calling ([`calling`]).
#[must_use]
pub fn wanted(workflow: &Workflow) -> Grants {
    let mut members = BTreeSet::new();
    if let Some(graph) = &workflow.graph {
        named(graph, &mut members);
    }
    let grants = workflow.grants.clone().unwrap_or_default();
    Grants {
        modules: grants.modules,
        members: members.into_iter().collect(),
        tools: grants.tools,
    }
}

/// `workflow`, checked as `found`, with the tools of effect `external` its
/// `tool` blocks call (its bodies' and branches' included) in its grants,
/// by name, once each.
#[must_use]
pub fn calling(mut workflow: Workflow, found: &Found) -> Workflow {
    let tools: BTreeSet<&str> = found
        .tools
        .values()
        .filter(|tool| tool.effect == Effect::External)
        .map(|tool| tool.name.as_str())
        .collect();
    workflow.grants.get_or_insert_default().tools = tools.into_iter().map(str::to_string).collect();
    workflow
}

fn set(names: &[String]) -> BTreeSet<&str> {
    names.iter().map(String::as_str).collect()
}

/// Whether `workflow`'s grants are within what a person approved; a new
/// workflow is approved by no one yet.
#[must_use]
pub fn approved(workflow: &Workflow) -> bool {
    let wanted = wanted(workflow);
    workflow
        .approved
        .as_ref()
        .and_then(|approved| approved.grants.as_ref())
        .is_some_and(|had| {
            set(&wanted.modules).is_subset(&set(&had.modules))
                && set(&wanted.members).is_subset(&set(&had.members))
                && set(&wanted.tools).is_subset(&set(&had.tools))
        })
}

/// Whether `workflow`, just written, needs a decision on its grants: not
/// approved, and not asked for these grants already, or asked and declined
/// (writing it again asks again).
#[must_use]
pub fn asks(workflow: &Workflow) -> bool {
    let wanted = wanted(workflow);
    let waits_for_these = workflow.asked.as_ref().is_some_and(|asked| {
        asked.declined_by.is_empty()
            && asked.grants.as_ref().is_some_and(|asked| {
                set(&asked.modules) == set(&wanted.modules)
                    && set(&asked.members) == set(&wanted.members)
                    && set(&asked.tools) == set(&wanted.tools)
            })
    });
    !approved(workflow) && !waits_for_these
}

/// `written` as the module keeps it: what was approved and asked is
/// `previous`'s (none for a new workflow), whatever the request said, and
/// its grants name no member and no tool (they are read from its graph).
#[must_use]
pub fn kept(mut written: Workflow, previous: Option<&Workflow>) -> Workflow {
    written.approved = previous.and_then(|p| p.approved.clone());
    written.asked = previous.and_then(|p| p.asked.clone());
    if let Some(grants) = &mut written.grants {
        grants.members.clear();
        grants.tools.clear();
    }
    written
}

/// `workflow` with what it needs asked for in `decision` on `task`.
#[must_use]
pub fn asked(mut workflow: Workflow, task: &str, decision: &str) -> Workflow {
    workflow.asked = Some(Asked {
        grants: Some(wanted(&workflow)),
        task: task.to_string(),
        decision: decision.to_string(),
        declined_by: String::new(),
    });
    workflow
}

/// `workflow` with the decision `decision` on its grants declined by
/// `declined_by`: it stays as it was approved (a new one: not approved) and
/// asks no one until it is written again; none when the decision is not the
/// one it waits for.
#[must_use]
pub fn decline(mut workflow: Workflow, decision: &str, declined_by: &str) -> Option<Workflow> {
    let asked = workflow.asked.as_mut()?;
    if asked.decision != decision || !asked.declined_by.is_empty() {
        return None;
    }
    asked.declined_by = declined_by.to_string();
    Some(workflow)
}

/// `workflow` with what decision `decision` asked approved by
/// `approved_by`; none when the decision is not the one it waits for.
#[must_use]
pub fn approve(mut workflow: Workflow, decision: &str, approved_by: &str) -> Option<Workflow> {
    let asked = workflow.asked.take()?;
    if asked.decision != decision {
        return None;
    }
    workflow.approved = Some(Approved {
        grants: asked.grants,
        decision: decision.to_string(),
        approved_by: approved_by.to_string(),
    });
    Some(workflow)
}

fn list(names: &BTreeSet<&str>) -> String {
    names.iter().copied().collect::<Vec<_>>().join(", ")
}

/// The names a workflow is let reach, of each kind.
type Reach<'a> = (BTreeSet<&'a str>, BTreeSet<&'a str>, BTreeSet<&'a str>);

/// What `reach` (modules, members, tools) lets a workflow do, in the words
/// `call`, `run` and `act`, joined by `and`; with none of them: calling no
/// module.
fn does(reach: &Reach<'_>, (call, run, act): (&str, &str, &str), and: &str) -> String {
    let (modules, members, tools) = reach;
    let mut parts = Vec::new();
    if !modules.is_empty() || (members.is_empty() && tools.is_empty()) {
        let named = if modules.is_empty() {
            "no module".to_string()
        } else {
            list(modules)
        };
        parts.push(format!("{call} {named}"));
    }
    if !members.is_empty() {
        parts.push(format!("{run} turns of {}", list(members)));
    }
    if !tools.is_empty() {
        parts.push(format!("{act} with {}", list(tools)));
    }
    parts.join(and)
}

/// What the decision on `workflow`'s grants asks a person.
#[must_use]
pub fn question(workflow: &Workflow) -> String {
    let wanted = wanted(workflow);
    let wants: Reach<'_> = (
        set(&wanted.modules),
        set(&wanted.members),
        set(&wanted.tools),
    );
    let Some(had) = workflow.approved.as_ref() else {
        return format!(
            "Let workflow {} run, {}?",
            workflow.name,
            does(&wants, ("calling", "with", "acting"), ", "),
        );
    };
    let had = had.grants.clone().unwrap_or_default();
    let had: Reach<'_> = (set(&had.modules), set(&had.members), set(&had.tools));
    let more: Reach<'_> = (
        wants.0.difference(&had.0).copied().collect(),
        wants.1.difference(&had.1).copied().collect(),
        wants.2.difference(&had.2).copied().collect(),
    );
    format!(
        "Let workflow {} also {} (it {} now)?",
        workflow.name,
        does(&more, ("call", "run", "act"), " and "),
        does(&had, ("calls", "runs", "acts"), " and "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grants(modules: &[&str]) -> Option<Grants> {
        Some(Grants {
            modules: modules.iter().map(|m| (*m).to_string()).collect(),
            ..Grants::default()
        })
    }

    #[test]
    fn a_workflow_runs_within_what_was_approved_and_asks_once_past_it() {
        let new = Workflow {
            name: "report".into(),
            grants: grants(&["board"]),
            approved: Some(Approved::default()),
            ..Default::default()
        };
        let new = kept(new, None);
        assert!(new.approved.is_none(), "the request does not approve");
        assert!(!approved(&new));
        assert!(asks(&new));
        assert_eq!(question(&new), "Let workflow report run, calling board?");

        let waiting = asked(new, "t-1", "d-1");
        assert!(!asks(&waiting), "asked once for these grants");
        assert!(decline(waiting.clone(), "d-2", "ana").is_none());
        let declined = decline(waiting.clone(), "d-1", "ana").unwrap();
        assert!(!approved(&declined));
        assert!(asks(&declined), "written again, it asks again");
        assert!(decline(declined, "d-1", "ana").is_none(), "declined once");
        assert!(approve(waiting.clone(), "d-2", "ana").is_none());
        let ok = approve(waiting, "d-1", "ana").unwrap();
        assert!(approved(&ok));
        assert!(ok.asked.is_none());

        let wider = Workflow {
            grants: grants(&["board", "mail"]),
            ..ok.clone()
        };
        let wider = kept(wider, Some(&ok));
        assert!(!approved(&wider));
        assert!(asks(&wider));
        assert_eq!(
            question(&wider),
            "Let workflow report also call mail (it calls board now)?"
        );
        let narrower = kept(
            Workflow {
                grants: grants(&[]),
                ..ok.clone()
            },
            Some(&ok),
        );
        assert!(approved(&narrower), "fewer grants need no decision");
    }

    /// A workflow of `members`' turns: one in its graph, one in a body.
    fn of_members(top: &str, inner: &str) -> Workflow {
        use post_cortex_proto::workflow::{AgentBlock, Block, ForEach};
        let turn = |id: &str, member: &str| Block {
            id: id.into(),
            kind: Some(Kind::Agent(AgentBlock {
                member: member.into(),
                ..Default::default()
            })),
            ..Default::default()
        };
        Workflow {
            name: "daily".into(),
            grants: Some(Grants {
                modules: vec!["telegram".into()],
                members: vec!["someone-the-request-named".into()],
                ..Grants::default()
            }),
            graph: Some(Graph {
                blocks: vec![
                    turn("look", top),
                    turn("own", ""),
                    Block {
                        id: "each".into(),
                        kind: Some(Kind::ForEach(ForEach {
                            body: Some(Graph {
                                blocks: vec![turn("inner", inner)],
                                ..Default::default()
                            }),
                            ..Default::default()
                        })),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn the_members_a_graph_names_are_asked_for_and_approved_with_its_modules() {
        let new = kept(of_members("eq-sentinel", "eq-sentinel"), None);
        assert_eq!(
            wanted(&new).members,
            ["eq-sentinel"],
            "from the graph, not the request; the agent's own turn names none"
        );
        assert_eq!(
            question(&new),
            "Let workflow daily run, calling telegram, with turns of eq-sentinel?"
        );
        let ok = approve(asked(new, "t-1", "d-1"), "d-1", "ana").unwrap();
        assert!(approved(&ok));

        // Approved before members were asked for (modules only): not within.
        let before = Workflow {
            approved: Some(Approved {
                grants: grants(&["telegram"]),
                ..Default::default()
            }),
            ..ok.clone()
        };
        assert!(!approved(&before));
        assert!(asks(&before));
        assert_eq!(
            question(&before),
            "Let workflow daily also run turns of eq-sentinel (it calls telegram now)?"
        );

        // Another member in a body: asked again, for that one.
        let wider = kept(of_members("eq-sentinel", "eq-medic"), Some(&ok));
        assert!(!approved(&wider));
        assert!(asks(&wider));
        assert_eq!(
            question(&wider),
            "Let workflow daily also run turns of eq-medic (it calls telegram and runs turns of eq-sentinel now)?"
        );
    }

    /// What a check finds of `blocks`, each a block id and the board's
    /// tool it calls, of effect `external` when named in `external`.
    fn found(blocks: &[(&str, &str)], external: &[&str]) -> Found {
        let tools = crate::test_support::board_tools();
        let mut found = Found::default();
        for (block, name) in blocks {
            let (_, mut tool) = tools
                .iter()
                .find(|(_, t)| t.name == *name)
                .cloned()
                .unwrap();
            tool.effect = if external.contains(name) {
                Effect::External
            } else {
                Effect::Read
            };
            found.tools.insert((*block).to_string(), tool);
        }
        found
    }

    #[test]
    fn each_external_tool_is_asked_for_and_a_new_one_asks_again() {
        let mut request = Workflow {
            name: "report".into(),
            grants: grants(&["board"]),
            ..Default::default()
        };
        request.grants.as_mut().unwrap().tools = vec!["what-the-request-says".into()];
        let new = kept(request, None);
        assert!(wanted(&new).tools.is_empty(), "the request's are not read");
        let new = calling(
            new,
            &found(
                &[
                    ("list", "listtasks"),
                    ("add", "addtask"),
                    ("again", "addtask"),
                ],
                &["addtask"],
            ),
        );
        assert_eq!(wanted(&new).tools, ["addtask"], "external only, once");
        assert_eq!(
            question(&new),
            "Let workflow report run, calling board, acting with addtask?"
        );
        let ok = approve(asked(new, "t-1", "d-1"), "d-1", "ana").unwrap();
        assert!(approved(&ok));

        let read_more = calling(
            kept(ok.clone(), Some(&ok)),
            &found(&[("add", "addtask"), ("get", "gettask")], &["addtask"]),
        );
        assert!(
            approved(&read_more),
            "a tool that only reads is the module's"
        );

        let more = calling(
            kept(ok.clone(), Some(&ok)),
            &found(
                &[("add", "addtask"), ("decide", "decide")],
                &["addtask", "decide"],
            ),
        );
        assert!(!approved(&more), "the same modules, another action");
        assert!(asks(&more));
        assert_eq!(
            question(&more),
            "Let workflow report also act with decide (it calls board and acts with addtask now)?"
        );
    }
}
