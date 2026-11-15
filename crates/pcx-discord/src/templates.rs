// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The workflows the module ships as its manifest declares them
//! (docs/modules/discord.md, the workflows; docs/modules/workflow.md,
//! templates): each the JSON of a workflow in `templates/`, with the
//! values an organisation chooses.

use post_cortex_proto::module::{TemplateParameter, WorkflowTemplate};

/// A name of the settings: a channel's, a member's.
const NAME: &str = "[a-z0-9-]{1,64}";

/// A name as Discord shows it: a role's, a forum tag's.
const SHOWN: &str = "[A-Za-z0-9 _.-]{1,100}";

/// A whole number of at most `digits` digits.
fn digits(digits: u8) -> String {
    format!("[0-9]{{1,{digits}}}")
}

fn parameter(name: &str, description: &str, default: &str, pattern: &str) -> TemplateParameter {
    TemplateParameter {
        name: name.to_string(),
        description: description.to_string(),
        default: default.to_string(),
        pattern: pattern.to_string(),
    }
}

fn template(
    name: &str,
    description: &str,
    workflow: &str,
    parameters: Vec<TemplateParameter>,
) -> WorkflowTemplate {
    WorkflowTemplate {
        name: name.to_string(),
        description: description.to_string(),
        workflow: workflow.to_string(),
        parameters,
    }
}

/// The module's templates.
pub fn templates() -> Vec<WorkflowTemplate> {
    vec![
        template(
            "discord-help",
            "Answers the help forum's new posts from the workspace's documents, hands the rest to the team, and moderates abuse: a timeout and a person's decision on a ban, a ban at once when it comes again.",
            include_str!("../templates/discord-help.json"),
            vec![
                parameter(
                    "help_channel",
                    "The help forum, by its name in the settings",
                    "help",
                    NAME,
                ),
                parameter(
                    "team_channel",
                    "Where the team is told, by its name in the settings",
                    "team",
                    NAME,
                ),
                parameter(
                    "member",
                    "The team member who answers: reading tools only, reaching only the public documents",
                    "community",
                    NAME,
                ),
                parameter(
                    "answered_tag",
                    "The forum tag of an answered post",
                    "answered",
                    SHOWN,
                ),
                parameter(
                    "needs_team_tag",
                    "The forum tag of a post a person must answer",
                    "needs-team",
                    SHOWN,
                ),
                parameter(
                    "timeout_minutes",
                    "How long a first abuse keeps its author from writing",
                    "1440",
                    &digits(5),
                ),
                parameter(
                    "repeat_hours",
                    "Within how many hours a second abuse is a ban at once",
                    "720",
                    &digits(5),
                ),
                parameter(
                    "delete_hours",
                    "The hours of a banned member's messages deleted (at most 168)",
                    "24",
                    &digits(3),
                ),
            ],
        ),
        template(
            "discord-welcome",
            "Gives each member who joins a role.",
            include_str!("../templates/discord-welcome.json"),
            vec![parameter(
                "role",
                "The role, by its name in the server",
                "Member",
                SHOWN,
            )],
        ),
        template(
            "discord-changelog",
            "Posts a release's notes to a channel, when it is run with them.",
            include_str!("../templates/discord-changelog.json"),
            vec![parameter(
                "channel",
                "The channel, by its name in the settings",
                "changelog",
                NAME,
            )],
        ),
        template(
            "discord-announce",
            "Announces a release as one card: its title, what it brings, a link and up to four pictures.",
            include_str!("../templates/discord-announce.json"),
            vec![
                parameter(
                    "channel",
                    "The channel, by its name in the settings",
                    "announcements",
                    NAME,
                ),
                parameter(
                    "color",
                    "The card's colour, 0xRRGGBB as a whole number (3907071 is #3B9DFF)",
                    "3907071",
                    &digits(8),
                ),
            ],
        ),
        template(
            "discord-feedback",
            "Thanks the author of each new post of the feedback forum and tells the team its title and author.",
            include_str!("../templates/discord-feedback.json"),
            vec![
                parameter(
                    "feedback_channel",
                    "The feedback forum, by its name in the settings",
                    "feedback",
                    NAME,
                ),
                parameter(
                    "team_channel",
                    "Where the team is told, by its name in the settings",
                    "team",
                    NAME,
                ),
            ],
        ),
        template(
            "discord-automod",
            "Tells the team each time AutoMod acts: on whom, by which rule and what it did.",
            include_str!("../templates/discord-automod.json"),
            vec![parameter(
                "team_channel",
                "Where the team is told, by its name in the settings",
                "team",
                NAME,
            )],
        ),
    ]
}
