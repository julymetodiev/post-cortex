#!/usr/bin/env python3
"""Sync the catalog packages in integrations/ from their sources.

The `pcx` crate templates are the single source of truth (crates.io packages
cannot include_str! files outside the crate):

    crates/post-cortex-cli/templates/   hook scripts, settings.json,
                                                   skill, clients/AGENTS.md
    Cargo.toml [workspace.package] version          every package version

Two files outside the templates are sources too: the icon
(integrations/claude-code/.claude-plugin/icon.png) and the plugin launcher
(integrations/cursor/scripts/pcx.sh).

integrations/claude-code (Claude Code plugin)
  Copied:       hooks/pcx-*.py, hooks/pcx_resolve.py, skills/post-cortex-memory/**
  Generated:    hooks/hooks.json             (templates/settings.json, the hooks
                                              `pcx setup --project` writes, with
                                              ${CLAUDE_PLUGIN_ROOT} paths and no
                                              per-project ids or backend: the
                                              scripts resolve them)
                .claude-plugin/plugin.json    ("version" field only)
  Hand-written: .claude-plugin/plugin.json (all other fields), .mcp.json,
                README.md, /.claude-plugin/marketplace.json

integrations/cursor (Cursor Marketplace plugin)
  Copied:       skills/post-cortex-memory/**, assets/logo.png (the icon)
  Generated:    rules/post-cortex.mdc        (clients/AGENTS.md without ids, as
                                              `pcx setup` writes it for a global
                                              setup, as an always-applied rule)
                .cursor-plugin/plugin.json    ("version" field only)
  Hand-written: .cursor-plugin/plugin.json (all other fields), mcp.json,
                hooks/hooks.json, scripts/pcx.sh, README.md

integrations/gemini (Gemini CLI extension)
  Copied:       skills/post-cortex-memory/**, scripts/pcx.sh (from cursor/)
  Generated:    GEMINI.md                    (the same instructions as the
                                              Cursor rule, without frontmatter)
                gemini-extension.json         ("version" field only)
  Hand-written: gemini-extension.json (all other fields), hooks/hooks.json,
                README.md

integrations/js/deepseek-harness (dsh bundle)
  Copied:       skills/post-cortex-memory/**  (the plugin registers it
                                              through ctx.skills)
  Hand-written: everything else

integrations/mcp-registry (official MCP Registry entry)
  Generated:    server.json, server.self-hosted.json  ("version" and each
                                              package's "version" only)
  Hand-written: both files (all other fields), README.md

Usage:
    python3 integrations/sync.py           write the generated and copied files
    python3 integrations/sync.py --check   exit 1 and list what differs
"""

import json
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
TEMPLATES = REPO / "crates/post-cortex-cli/templates"
INTEGRATIONS = REPO / "integrations"
PLUGIN = INTEGRATIONS / "claude-code"
CURSOR = INTEGRATIONS / "cursor"
GEMINI = INTEGRATIONS / "gemini"
REGISTRY = INTEGRATIONS / "mcp-registry"
DSH = INTEGRATIONS / "js/deepseek-harness"
ICON = PLUGIN / ".claude-plugin/icon.png"
LAUNCHER = CURSOR / "scripts/pcx.sh"

HOOK_SCRIPTS = (
    "pcx-card.py",
    "pcx-file-context.py",
    "pcx-capture.py",
    "pcx-stop-check.py",
    "pcx-compact-reinject.py",
    "pcx-banner.py",
    "pcx_resolve.py",
)
SKILL = "skills/post-cortex-memory"
PLACEHOLDERS = (" <SESSION_ID>", " <WORKSPACE_ID>", " <BACKEND>")
PROJECT_HOOKS = '"$CLAUDE_PROJECT_DIR"/.claude/hooks/'
PLUGIN_HOOKS = '"${CLAUDE_PLUGIN_ROOT}"/hooks/'

# The rewrites of `global_agents_md_block` in
# crates/post-cortex-daemon/src/bin/pcx/clients/common.rs: the instructions
# for a user-level file, whose project is named by the session-start hook.
GLOBAL_INTRO = (
    "This project keeps its long-term memory in post-cortex (MCP server `post-cortex`).",
    "Projects keep their long-term memory in post-cortex (MCP server `post-cortex`). "
    "The session and workspace of the current project are given at session start "
    "(`[PCX] Session: ... | Workspace: ...`).",
)
GLOBAL_SESSION = ("with `session_id` above", "with that session ID")
# What the catalog packages add: they may run without pcx (no hooks, so no
# session line), where the agent resolves the project itself.
NO_HOOKS_NOTE = (
    "\nNo `[PCX] Session:` line at session start (the hooks run only where `pcx` is "
    "installed)? Get the project's session and workspace with the `session` tool, "
    "action `resolve_project` (see the `post-cortex-memory` skill), before the first "
    "search or update.\n"
)


def workspace_version():
    text = (REPO / "Cargo.toml").read_text(encoding="utf-8")
    section = text.split("[workspace.package]", 1)[1].split("\n[", 1)[0]
    return re.search(r'^version\s*=\s*"([^"]+)"', section, re.M).group(1)


def text_of(path):
    return path.read_text(encoding="utf-8")


def dump(value):
    return json.dumps(value, indent=2, ensure_ascii=False) + "\n"


def with_version(path, update=None):
    """A hand-written JSON manifest with the workspace version set in it."""
    manifest = json.loads(text_of(path))
    manifest["version"] = workspace_version()
    if update:
        update(manifest)
    return dump(manifest)


def global_instructions():
    """templates/clients/AGENTS.md for a user-level setup, as
    `global_agents_md_block` (pcx) builds it, plus the no-hooks note."""
    lines = [
        line
        for line in text_of(TEMPLATES / "clients/AGENTS.md").splitlines()
        if not ("<SESSION_ID>" in line or "<WORKSPACE_ID>" in line or line == "|---|---|")
    ]
    text = "\n".join(lines).replace(*GLOBAL_INTRO).replace(*GLOBAL_SESSION)
    text = text.replace("\n\n\n", "\n\n") + "\n"
    if "<" in text and "_ID>" in text:
        raise SystemExit("templates/clients/AGENTS.md: unhandled placeholder")
    return text.rstrip() + "\n" + NO_HOOKS_NOTE


def skill_files():
    files = {}
    for src in sorted((TEMPLATES / SKILL).rglob("*")):
        if src.is_file() and "__pycache__" not in src.parts:
            files[src.relative_to(TEMPLATES).as_posix()] = (text_of(src), False)
    return files


# ----------------------------------------------------------- claude-code


def plugin_command(command):
    """A project hook command as the plugin runs it: the script from the
    plugin root, no id or backend arguments (the scripts resolve them). The
    project's `echo` banner becomes pcx-banner.py, which resolves the ids
    and steps aside when the project prints its own banner."""
    if command.startswith("echo '[PCX]"):
        return "python3 -I " + PLUGIN_HOOKS + "pcx-banner.py"
    command = command.replace(PROJECT_HOOKS, PLUGIN_HOOKS)
    for placeholder in PLACEHOLDERS:
        command = command.replace(placeholder, "")
    if "<" in command and ">" in command.split("<", 1)[1]:
        raise SystemExit("templates/settings.json: unhandled placeholder in " + command)
    return command


def hooks_json():
    settings = json.loads(text_of(TEMPLATES / "settings.json"))
    hooks = settings["hooks"]
    for groups in hooks.values():
        for group in groups:
            for hook in group["hooks"]:
                hook["command"] = plugin_command(hook["command"])
    body = {
        "description": "post-cortex memory hooks: rules on every prompt, cards on "
        "session start, context re-injected after compaction, file memory before "
        "edits, a Stop check for unlogged changes, and conversation capture when "
        "it is on in ~/.post-cortex/config.json",
        "hooks": hooks,
    }
    return dump(body)


def claude_code_files():
    files = {}
    for name in HOOK_SCRIPTS:
        files["hooks/" + name] = (text_of(TEMPLATES / name), name.startswith("pcx-"))
    files["hooks/hooks.json"] = (hooks_json(), False)
    files.update(skill_files())
    files[".claude-plugin/plugin.json"] = (
        with_version(PLUGIN / ".claude-plugin/plugin.json"),
        False,
    )
    return files


# ---------------------------------------------------------------- cursor


def cursor_rule():
    """The always-applied rule, in the frontmatter `pcx setup --project`
    gives `.cursor/rules/post-cortex.mdc`."""
    return (
        "---\ndescription: post-cortex memory rules\nalwaysApply: true\n---\n\n"
        + global_instructions()
    )


def cursor_files():
    files = skill_files()
    files["rules/post-cortex.mdc"] = (cursor_rule(), False)
    files["assets/logo.png"] = (ICON.read_bytes(), False)
    files[".cursor-plugin/plugin.json"] = (
        with_version(CURSOR / ".cursor-plugin/plugin.json"),
        False,
    )
    return files


# ---------------------------------------------------------------- gemini


def gemini_files():
    files = skill_files()
    files["GEMINI.md"] = (global_instructions(), False)
    files["scripts/pcx.sh"] = (text_of(LAUNCHER), True)
    files["gemini-extension.json"] = (with_version(GEMINI / "gemini-extension.json"), False)
    return files


# ---------------------------------------------------------- mcp-registry


def registry_files():
    def packages(server):
        for package in server.get("packages", []):
            package["version"] = workspace_version()
        if len(server.get("description", "")) > 100:
            raise SystemExit("mcp-registry/server.json: description over 100 characters")

    return {
        name: (with_version(REGISTRY / name, packages), False)
        for name in ("server.json", "server.self-hosted.json")
    }


# --------------------------------------------------------------- driver

# (package directory, files it owns, directories whose other files are stale)
PACKAGES = (
    (PLUGIN, claude_code_files, ("hooks", SKILL)),
    (CURSOR, cursor_files, (SKILL, "rules", "assets")),
    (GEMINI, gemini_files, (SKILL, "scripts")),
    (DSH, skill_files, (SKILL,)),
    (REGISTRY, registry_files, ()),
)


def stale_files(root, files, owned_dirs):
    """Files in a package's generated directories that the sources no
    longer have (a renamed hook, a removed skill file)."""
    present = set()
    for rel in owned_dirs:
        top = root / rel
        if top.exists():
            present |= {
                p.relative_to(root).as_posix()
                for p in top.rglob("*")
                if p.is_file() and "__pycache__" not in p.parts
            }
    return sorted(present - set(files))


def sync_package(root, files, owned_dirs, check):
    """Write (or, with check, only compare) one package; returns what differs."""
    differ = []
    for rel, (content, executable) in files.items():
        path = root / rel
        binary = isinstance(content, bytes)
        if path.exists():
            current = path.read_bytes() if binary else path.read_text(encoding="utf-8")
        else:
            current = None
        mode_ok = not executable or (path.exists() and path.stat().st_mode & 0o111)
        if current == content and mode_ok:
            continue
        differ.append(rel)
        if not check:
            path.parent.mkdir(parents=True, exist_ok=True)
            if binary:
                path.write_bytes(content)
            else:
                path.write_text(content, encoding="utf-8")
            if executable:
                path.chmod(0o755)
    for rel in stale_files(root, files, owned_dirs):
        differ.append(rel + " (not in the source)")
        if not check:
            (root / rel).unlink()
    return differ


def main():
    check = "--check" in sys.argv[1:]
    failed = False
    for root, files, owned_dirs in PACKAGES:
        differ = sync_package(root, files(), owned_dirs, check)
        name = root.relative_to(REPO).as_posix()
        if check and differ:
            failed = True
            print(name + " is out of date with its sources:")
            for rel in differ:
                print("  " + rel)
        elif not check:
            for rel in differ:
                print("synced " + name + "/" + rel)
    if failed:
        print("run: python3 integrations/sync.py")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
