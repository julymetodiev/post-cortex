"""The catalog packages (Cursor plugin, Gemini CLI extension, MCP Registry
entry): the pcx launcher the hooks and the Cursor MCP server run, and the
manifests' references to it.

    python3 -m unittest discover -s integrations/cursor/tests -v
"""

import json
import os
import re
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

INTEGRATIONS = Path(__file__).resolve().parent.parent.parent
CURSOR = INTEGRATIONS / "cursor"
GEMINI = INTEGRATIONS / "gemini"
REGISTRY = INTEGRATIONS / "mcp-registry"
LAUNCHER = CURSOR / "scripts/pcx.sh"
SH = shutil.which("sh")

# A pcx that records its arguments and stdin, and answers like a hook.
FAKE_PCX = """#!/bin/sh
printf '%s\\n' "$@" > "$PCX_LOG.args"
cat > "$PCX_LOG.stdin"
echo '{"additional_context": "from pcx"}'
"""

# Events each client documents (Cursor: cursor.com/docs/reference/plugins;
# Gemini CLI: geminicli.com/docs/hooks/reference) and the `pcx hook` events
# pcx implements for it (pcx/clients/{cursor,gemini}.rs).
CURSOR_EVENTS = {"sessionStart", "postToolUse", "beforeSubmitPrompt", "afterAgentResponse"}
GEMINI_EVENTS = {
    "SessionStart": "session-start",
    "AfterTool": "file-context",
    "BeforeAgent": "prompt",
    "AfterAgent": "capture",
}


@unittest.skipIf(SH is None, "no POSIX sh")
class Launcher(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)
        self.home = self.tmp / "home"
        self.home.mkdir()
        self.bin = self.tmp / "bin"
        self.bin.mkdir()
        self.log = self.tmp / "pcx"

    def install_pcx(self):
        pcx = self.bin / "pcx"
        pcx.write_text(FAKE_PCX)
        pcx.chmod(0o755)
        return pcx

    def run_launcher(self, *args, stdin='{"cwd": "/p"}', path=None):
        env = {
            "HOME": str(self.home),
            "PATH": f"{self.bin}:/usr/bin:/bin" if path is None else path,
            "PCX_LOG": str(self.log),
            "PCX_FALLBACKS": "",
        }
        return subprocess.run(
            [SH, str(LAUNCHER), *args],
            input=stdin,
            capture_output=True,
            text=True,
            env=env,
            timeout=10,
        )

    def pcx_args(self):
        path = Path(str(self.log) + ".args")
        return path.read_text().split("\n")[:-1] if path.exists() else None

    def write(self, rel, text):
        path = self.home / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def test_hook_runs_pcx_with_the_payload(self):
        self.install_pcx()
        out = self.run_launcher("hook", "cursor", "sessionStart")
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertEqual(self.pcx_args(), ["hook", "cursor", "sessionStart"])
        self.assertEqual(Path(str(self.log) + ".stdin").read_text(), '{"cwd": "/p"}')
        self.assertIn("from pcx", out.stdout)

    def test_a_fallback_location_is_found_without_path(self):
        pcx = self.install_pcx()
        env_path = "/usr/bin:/bin"
        out = subprocess.run(
            [SH, str(LAUNCHER), "hook", "gemini", "session-start"],
            input="{}",
            capture_output=True,
            text=True,
            env={"HOME": str(self.home), "PATH": env_path, "PCX_LOG": str(self.log),
                 "PCX_FALLBACKS": f"/nonexistent/pcx {pcx}"},
            timeout=10,
        )
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertEqual(self.pcx_args(), ["hook", "gemini", "session-start"])

    def test_hook_is_silent_without_pcx(self):
        out = self.run_launcher("hook", "cursor", "sessionStart", path="/usr/bin:/bin")
        self.assertEqual((out.returncode, out.stdout, out.stderr), (0, "", ""))
        out = self.run_launcher("hook", "gemini", "file-context", path="/usr/bin:/bin")
        self.assertEqual((out.returncode, out.stdout, out.stderr), (0, "", ""))

    def test_a_skipped_prompt_hook_lets_the_prompt_go_on(self):
        out = self.run_launcher("hook", "cursor", "beforeSubmitPrompt", path="/usr/bin:/bin")
        self.assertEqual(out.returncode, 0)
        self.assertEqual(json.loads(out.stdout), {"continue": True})

    def test_hook_steps_aside_for_pcx_setup_hooks(self):
        self.install_pcx()
        # What `pcx setup` writes into ~/.cursor/hooks.json, plain and with
        # a quoted pcx path.
        for command in (
            "/opt/homebrew/bin/pcx hook cursor sessionStart --profile cloud",
            '"/Users/a b/bin/pcx" hook cursor sessionStart --profile cloud',
        ):
            self.write(".cursor/hooks.json", json.dumps(
                {"hooks": {"sessionStart": [{"command": command}]}}))
            out = self.run_launcher("hook", "cursor", "sessionStart")
            self.assertEqual((out.returncode, out.stdout), (0, ""), command)
            self.assertIsNone(self.pcx_args(), command)
        # Gemini's own hooks do not matter to Cursor.
        (self.home / ".cursor/hooks.json").unlink()
        self.write(".gemini/settings.json", json.dumps({"hooks": {"SessionStart": [{"hooks": [
            {"type": "command", "command": "/usr/local/bin/pcx hook gemini session-start"}]}]}}))
        self.assertEqual(self.run_launcher("hook", "cursor", "sessionStart").returncode, 0)
        self.assertEqual(self.pcx_args(), ["hook", "cursor", "sessionStart"])
        Path(str(self.log) + ".args").unlink()
        self.run_launcher("hook", "gemini", "session-start")
        self.assertIsNone(self.pcx_args())

    def test_capture_hooks_follow_the_host_setting(self):
        self.install_pcx()
        for client, event in (("cursor", "afterAgentResponse"), ("gemini", "capture")):
            out = self.run_launcher("hook", client, event)
            self.assertEqual(out.returncode, 0)
            self.assertIsNone(self.pcx_args(), "capture is off by default")
        self.write(".post-cortex/config.json", json.dumps({
            "active": "cloud",
            "hosts": {"cursor": {"capture": True}, "gemini-cli": {"enabled": True, "capture": False}},
        }, indent=2))
        self.run_launcher("hook", "cursor", "afterAgentResponse")
        self.assertEqual(self.pcx_args(), ["hook", "cursor", "afterAgentResponse"])
        Path(str(self.log) + ".args").unlink()
        self.run_launcher("hook", "gemini", "capture")
        self.assertIsNone(self.pcx_args())
        # Non-capture events never look at the setting.
        self.run_launcher("hook", "gemini", "prompt")
        self.assertEqual(self.pcx_args(), ["hook", "gemini", "prompt"])

    def test_mcp_runs_pcx_mcp(self):
        self.install_pcx()
        out = self.run_launcher("mcp", "--author", "cursor", stdin="")
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertEqual(self.pcx_args(), ["mcp", "--author", "cursor"])

    def test_mcp_without_pcx_says_how_to_install_it(self):
        out = self.run_launcher("mcp", "--author", "cursor", stdin="", path="/usr/bin:/bin")
        self.assertEqual(out.returncode, 127)
        self.assertIn("pcx is not installed", out.stderr)


def load(path):
    return json.loads(path.read_text(encoding="utf-8"))


def placeholders(value):
    return set(re.findall(r"\$\{([^}:]+)", json.dumps(value)))


class CursorPlugin(unittest.TestCase):
    def test_manifest_paths_are_relative_and_exist(self):
        manifest = load(CURSOR / ".cursor-plugin/plugin.json")
        self.assertRegex(manifest["name"], r"^[a-z0-9]([a-z0-9.-]*[a-z0-9])?$")
        for key in ("logo", "mcpServers", "hooks", "skills", "rules"):
            rel = manifest[key]
            self.assertFalse(rel.startswith("/") or ".." in rel, key)
            self.assertTrue((CURSOR / rel).exists(), key)

    def test_mcp_server_runs_the_launcher(self):
        server = load(CURSOR / "mcp.json")["mcpServers"]["post-cortex"]
        self.assertEqual(server["command"], "sh")
        self.assertEqual(server["args"][0], "${CURSOR_PLUGIN_ROOT}/scripts/pcx.sh")
        self.assertEqual(server["args"][1:], ["mcp", "--author", "cursor"])
        # Only Cursor's own variable: a ${VAR} would need a `variables` schema.
        self.assertEqual(placeholders(server), {"CURSOR_PLUGIN_ROOT"})

    def test_hooks_call_pcx_hook_cursor(self):
        hooks = load(CURSOR / "hooks/hooks.json")["hooks"]
        self.assertEqual(set(hooks), CURSOR_EVENTS)
        for event, entries in hooks.items():
            (entry,) = entries
            self.assertEqual(
                entry["command"],
                f'sh "${{CURSOR_PLUGIN_ROOT}}/scripts/pcx.sh" hook cursor {event}',
            )
        self.assertEqual(hooks["postToolUse"][0]["matcher"], "Read")

    def test_rule_is_always_applied(self):
        rule = (CURSOR / "rules/post-cortex.mdc").read_text(encoding="utf-8")
        self.assertTrue(rule.startswith("---\ndescription: "))
        self.assertIn("\nalwaysApply: true\n---\n", rule)
        self.assertNotIn("<SESSION_ID>", rule)


class GeminiExtension(unittest.TestCase):
    def test_manifest(self):
        manifest = load(GEMINI / "gemini-extension.json")
        self.assertEqual(manifest["name"], "post-cortex")
        self.assertTrue((GEMINI / manifest["contextFileName"]).is_file())
        server = manifest["mcpServers"]["post-cortex"]
        self.assertEqual(server["httpUrl"], "${PC_URL:-https://api.pcx.is}/mcp")
        # Every variable has a default or is a declared setting.
        settings = {s["envVar"] for s in manifest["settings"]}
        self.assertEqual(settings, {"PC_AUTH_TOKEN"})
        self.assertTrue(all(s["sensitive"] for s in manifest["settings"]))
        self.assertIn("${PC_AUTH_TOKEN:-}", server["headers"]["Authorization"])

    def test_hooks_call_pcx_hook_gemini(self):
        hooks = load(GEMINI / "hooks/hooks.json")["hooks"]
        self.assertEqual(set(hooks), set(GEMINI_EVENTS))
        for event, groups in hooks.items():
            (group,) = groups
            (hook,) = group["hooks"]
            self.assertEqual(hook["type"], "command")
            self.assertEqual(
                hook["command"],
                f'sh "${{extensionPath}}/scripts/pcx.sh" hook gemini {GEMINI_EVENTS[event]}',
            )
        self.assertEqual(hooks["AfterTool"][0]["matcher"], "read_file")
        self.assertTrue(os.access(GEMINI / "scripts/pcx.sh", os.X_OK))


class RegistryEntry(unittest.TestCase):
    def test_server_json(self):
        for name in ("server.json", "server.self-hosted.json"):
            server = load(REGISTRY / name)
            self.assertEqual(server["name"], "io.github.julymetodiev/post-cortex")
            self.assertLessEqual(len(server["description"]), 100)
            (remote,) = server["remotes"]
            self.assertEqual(remote["type"], "streamable-http")
            self.assertEqual(remote["url"], "https://api.pcx.is/mcp")
            (header,) = remote["headers"]
            self.assertTrue(header["isRequired"] and header["isSecret"])
        self.assertNotIn("packages", load(REGISTRY / "server.json"))
        (package,) = load(REGISTRY / "server.self-hosted.json")["packages"]
        self.assertEqual((package["registryType"], package["identifier"]),
                         ("cargo", "post-cortex-daemon"))
        self.assertEqual(package["version"], load(REGISTRY / "server.json")["version"])


if __name__ == "__main__":
    unittest.main()
