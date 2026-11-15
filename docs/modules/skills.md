# skills

Part of the specification (`docs/ARCHITECTURE.md`, LAW rule 2): section 8.

- **`skills`** (W17): a library of skills attached to agents.
  - Documents: `skills/skill` (name, description, instructions in Markdown, the tools it
    needs); a change is its next version.
  - Tools: `skill_add`, `skill_update`, `skill_list`, `skill_get`.
  - The host's MCP server lists each skill as an MCP prompt (`Module.prompts`, added in W17),
    so other agents use them too. Scripts in a skill run through `tools-shell` (W15).
  - *Where it runs* (owner 2026-10-25): one crate, `pcx-skills`, run like the board: in
    process in the daemon as a built-in (`[[builtins]] id = "skills"`), its `pcx-skills` binary
    as a module process for the cloud catalog. It uses `post-cortex.memory`.
  - *One library*: the skills of a host (a tenant in the cloud) live in one workspace, the
    module's required setting `workspace_id` (its `config_type`); the tools and the prompts
    work there, so `prompts/list`, which names no workspace, lists them.
  - *A skill*: its name (`[a-z0-9-]`, at most 64 characters, unique in the library:
    `skill_add` refuses a name that is taken), its description, its instructions, the tools it
    needs (named, not checked: a skill may name a module not running yet). `skill_update`
    writes the next version from `expected_version` (a conflict answers the latest, as the
    board does). No delete.
  - *Built-in skills* (owner 2026-10-31): the crate ships skills as Markdown files
    (`crates/pcx-skills/library/<name>.md`: a front matter of `name`, `description` and
    `tools`, then the instructions). When the module starts it adds each one whose name the
    library has not (memory callable: it tries again until it is). One the library has is
    renewed to the text shipped when the module itself wrote its last version (the document's
    author is `skills`): nobody changed it, so a new release's text reaches it (owner
    2026-11-01). One whose last version someone else wrote (a person, an agent) is left as it is,
    so their change stays. The first are `workflow-builder` and `module-builder`
    (docs/modules/workflow.md, the builder).
  - *The view*: the library's skills, with a button to add one; a skill's detail is its
    instructions and a button to edit it (a dialog with what it has).
  - *Its prompt*: the skill's name and description, no arguments; `get` answers one user
    message: the instructions, then the tools it needs. A script is written in the
    instructions; the agent runs it through `tools-shell`, which the skill lists in its tools;
    the module runs nothing.
  - *Documents of a module, shared*: the port over a module's documents (get, put from an
    expected version, list by data fields) and its adapter over memory's `manage_document`
    through the host handle are one crate, `pcx-documents`, used by `board` and `skills`; so
    is the port of a module's new sessions in its workspace (`Sessions`, over memory's
    `session` and `manage_workspace`), used by `team` and `chief`.
  - *Layers*: the skill's rules (its name, its prompt) are pure domain code; the skills service
    reaches its documents through `pcx-documents`; the tool handlers only map JSON.
