// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The skills' view (ARCHITECTURE section 8): the library's skills, with a
//! button to add one; a skill's detail is its instructions and a button to
//! edit it (a dialog with what it has).

use pcx_module_api::view::build::{action, bar, dialog, group, lines, node, source, stack, text};
use pcx_module_api::view::{Body, Component, List, Markdown, View, component, markdown};
use serde_json::{Value, json as value};

use crate::specs::path;

/// The fields of a skill a person writes, as a `SkillRecord` nests them;
/// `named`: its name too.
fn skill_fields(named: bool) -> Value {
    let mut skill = value!({
        "description": text("What it is for, in a line"),
        "tools": lines("The tools it needs, e.g. bash")
    });
    let mut required = vec!["description"];
    if named {
        skill["name"] = text("Name (a-z, 0-9 and -; its prompt's name)");
        required.push("name");
    }
    value!({
        "skill": group("Skill", skill, &required),
        "instructions": { "type": "string", "format": "multiline", "title": "Instructions",
                  "description": "Markdown; a script goes in a code block, run with the tools it needs" }
    })
}

/// A skill's detail: `{{row.*}}` is its `SkillRecord`.
fn skill() -> Component {
    let shown = node(component::Kind::Markdown(Markdown {
        content: Some(markdown::Content::Source(source(
            &path("GetSkill"),
            value!({ "name": "{{row.skill.name}}" }),
            "/instructions",
        ))),
    }));
    let edit = dialog(
        "Edit",
        skill_fields(false),
        &["skill", "instructions"],
        Some(source(
            &path("GetSkill"),
            value!({ "name": "{{row.skill.name}}" }),
            "",
        )),
        action(
            "Save the skill",
            &path("UpdateSkill"),
            value!({ "name": "{{row.skill.name}}", "version": "{{row.version}}",
                     "description": "{{form.skill.description}}",
                     "instructions": "{{form.instructions}}",
                     "tools": { "names": "{{form.skill.tools}}" } }),
            Some("Change this skill? Every agent's prompt of it changes too."),
        ),
    );
    stack(vec![bar(vec![edit]), shown])
}

/// The skills' views.
pub fn views() -> Vec<View> {
    let skills = node(component::Kind::List(Box::new(List {
        source: Some(source(&path("ListSkills"), value!({}), "/skills")),
        title_field: "skill.name".to_string(),
        subtitle_field: "skill.description".to_string(),
        row_actions: Vec::new(),
        empty: "The library has no skills yet.".to_string(),
        detail: Some(Box::new(skill())),
        markdown: false,
    })));
    let add = dialog(
        "New skill",
        skill_fields(true),
        &["skill", "instructions"],
        None,
        action(
            "Add the skill",
            &path("AddSkill"),
            value!({ "name": "{{form.skill.name}}", "description": "{{form.skill.description}}",
                     "instructions": "{{form.instructions}}", "tools": "{{form.skill.tools}}" }),
            None,
        ),
    );
    vec![View {
        id: "skills".to_string(),
        title: "Skills".to_string(),
        description: "The library's skills: what each is for and its instructions.".to_string(),
        body: Some(Body::Root(Box::new(stack(vec![bar(vec![add]), skills])))),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    use pcx_module_api::Module;

    /// The view fits the module's RPCs, as the host checks it at load.
    #[test]
    fn the_view_fits_the_rpcs() {
        let module = crate::SkillsModule::new();
        pcx_module_api::view::check(module.manifest()).unwrap();
        assert_eq!(views().len(), 1);
    }
}
