#!/usr/bin/env python3
"""post-cortex UserPromptSubmit hook of the Claude Code plugin: the memory
rules, with the project's session and workspace, before every prompt.

Usage (hooks/hooks.json of the plugin):
    python3 -I "${CLAUDE_PLUGIN_ROOT}"/hooks/pcx-banner.py
Project setups (`pcx setup --project`) print the same line with an `echo`
in .claude/settings.json; when the project has such a banner this hook
prints nothing, so the rules appear once. Fails open: prints the rules
without ids when the project cannot be resolved.
"""

import json
import os
import sys

sys.dont_write_bytecode = True  # no __pycache__ next to the hooks
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
try:
    import pcx_resolve
except ImportError:  # an older copy of the hooks without the resolver
    pcx_resolve = None

RULES = (
    "Rules: 1) Search PCX before answering code/arch questions "
    "2) Log discoveries to PCX after "
    "3) PCX replaces Claude Code's auto memory: \"remember\" means "
    "update_conversation_context, never a memory/*.md file"
)


def project_has_banner():
    """Whether the project's own settings already print a [PCX] banner on
    UserPromptSubmit (a `pcx setup --project` install)."""
    root = os.environ.get("CLAUDE_PROJECT_DIR", "").strip()
    if not root:
        return False
    for name in ("settings.json", "settings.local.json"):
        try:
            with open(os.path.join(root, ".claude", name), encoding="utf-8") as f:
                settings = json.load(f)
        except (OSError, ValueError):
            continue
        groups = (settings.get("hooks") or {}).get("UserPromptSubmit") or []
        for group in groups:
            for hook in group.get("hooks") or []:
                if "[PCX]" in str(hook.get("command", "")):
                    return True
    return False


def main():
    payload = pcx_resolve.read_payload(sys.stdin) if pcx_resolve else {}
    if project_has_banner():
        return
    if pcx_resolve is not None:
        if pcx_resolve.shadowed_by_project(__file__):
            return
        if not pcx_resolve.host_settings()["enabled"]:
            return
        url, token, _ = pcx_resolve.resolve_server()
        ids = pcx_resolve.project_ids(payload, url, token) if url else None
        if ids:
            print("%s | %s" % (pcx_resolve.ids_line(ids), RULES))
            return
    print("[PCX] " + RULES)


if __name__ == "__main__":
    try:
        main()
    except Exception:  # never block the prompt
        print("[PCX] " + RULES)
