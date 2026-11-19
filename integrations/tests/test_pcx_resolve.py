"""Tests of templates/pcx_resolve.py and the Claude Code hooks that use it.

Run from the repository root:
    python3 -m unittest discover -s integrations/tests -v
"""

import contextlib
import importlib.util
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from unittest import mock

sys.dont_write_bytecode = True  # no __pycache__ in the templates directory
REPO = Path(__file__).resolve().parents[2]
TEMPLATES = REPO / "crates/post-cortex-cli/templates"
VECTORS = REPO / "integrations/testdata/project-keys.json"

spec = importlib.util.spec_from_file_location("pcx_resolve", TEMPLATES / "pcx_resolve.py")
pcx_resolve = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pcx_resolve)

S = "11111111-2222-3333-4444-555555555555"
W = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
ENV_KEYS = (
    "PC_CONFIG_DIR",
    "PC_URL",
    "PC_PROFILE",
    "PC_AUTH_TOKEN",
    "CLAUDE_PLUGIN_OPTION_URL",
    "CLAUDE_PLUGIN_OPTION_API_KEY",
    "CLAUDE_PROJECT_DIR",
)


def git(*args, cwd):
    subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True)


class FakeDaemon:
    """POST /api/projects/resolve and POST /api/events on 127.0.0.1 (a
    SessionStart answered as the memory module answers it), recording every
    request."""

    def __init__(self):
        self.requests = []
        self.refuse = None  # (status, text) to answer POSTs with
        self.refuse_workspace = None  # (workspace_id, status, text) for events naming it
        self.event_answer = None  # body -> answer of /api/events, in place of the cards
        daemon = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def reply(self, body):
                data = json.dumps(body).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def do_POST(self):
                length = int(self.headers.get("Content-Length") or 0)
                body = json.loads(self.rfile.read(length) or b"{}")
                daemon.requests.append(("POST", self.path, body, dict(self.headers)))
                refused = daemon.refuse
                named = daemon.refuse_workspace
                if named and (body.get("scope") or {}).get("workspace_id") == named[0]:
                    refused = named[1:]
                if refused:
                    status, text = refused
                    data = text.encode()
                    self.send_response(status)
                    self.send_header("Content-Type", "text/plain")
                    self.send_header("Content-Length", str(len(data)))
                    self.end_headers()
                    self.wfile.write(data)
                    return
                if self.path == "/api/events" and daemon.event_answer:
                    self.reply(daemon.event_answer(body))
                    return
                if self.path == "/api/events":
                    workspace = body["scope"]["workspace_id"]
                    self.reply(
                        {
                            "hookSpecificOutput": {
                                "hookEventName": body["event"]["hook_event_name"],
                                "additionalContext": "## post-cortex cards\n\n"
                                "### User card (v2)\nPrefers short answers.\n\n"
                                "No project card yet: workspace_id %s" % workspace,
                            }
                        }
                    )
                    return
                self.reply({"workspace_id": W, "session_id": S, "created": True})

        self.server = HTTPServer(("127.0.0.1", 0), Handler)
        self.url = "http://127.0.0.1:%d" % self.server.server_address[1]
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()


class Isolated(unittest.TestCase):
    """A temp PC_CONFIG_DIR and none of the variables the resolver reads."""

    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.config = self.tmp / "config"
        env = {k: v for k, v in os.environ.items() if k not in ENV_KEYS}
        env["PC_CONFIG_DIR"] = str(self.config)
        patcher = mock.patch.dict(os.environ, env, clear=True)
        patcher.start()
        self.addCleanup(patcher.stop)
        self.addCleanup(shutil.rmtree, self.tmp, True)

    def write(self, rel, value):
        path = self.config / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(value))


class Vectors(unittest.TestCase):
    def test_remote_keys(self):
        vectors = json.loads(VECTORS.read_text())
        for case in vectors["remotes"]:
            key = pcx_resolve.normalize_remote(case["url"])
            self.assertEqual(key, case["key"], case["url"])
            if key:
                self.assertEqual(key.rsplit("/", 1)[-1], case["name"], case["url"])

    def test_local_keys(self):
        for case in json.loads(VECTORS.read_text())["local"]:
            self.assertEqual(pcx_resolve.local_key(case["root"]), case["key"])

    def test_identities(self):
        for case in json.loads(VECTORS.read_text())["identities"]:
            self.assertEqual(pcx_resolve.identity(case["token"]), case["identity"], case["token"])


class CacheVectors(unittest.TestCase):
    def test_cache_entries_count_for_their_backend_and_identity(self):
        cases = json.loads(VECTORS.read_text())["cache"]
        self.assertGreater(len(cases), 5)
        for case in cases:
            with tempfile.TemporaryDirectory() as tmp, mock.patch.dict(
                os.environ, {"PC_CONFIG_DIR": tmp}
            ):
                Path(tmp, "projects.json").write_text(json.dumps({"k": case["entry"]}))
                ids = pcx_resolve.cached_ids("k", case["url"], case["token"])
                expected = {"session_id": S, "workspace_id": W} if case["hit"] else None
                self.assertEqual(ids, expected, case["why"])


@unittest.skipIf(shutil.which("git") is None, "git is not installed")
class Projects(Isolated):
    def test_remote_local_and_plain_directory(self):
        repo = self.tmp / "repo"
        (repo / "src").mkdir(parents=True)
        git("init", "-q", cwd=repo)
        root = os.path.realpath(repo)
        key, name, found = pcx_resolve.project(str(repo / "src"))
        self.assertEqual((key, name, found), (pcx_resolve.local_key(root), "repo", root))

        git("remote", "add", "origin", "git@github.com:a/b.git", cwd=repo)
        self.assertEqual(
            pcx_resolve.project(str(repo / "src")), ("github.com/a/b", "b", root)
        )

        plain = self.tmp / "plain"
        plain.mkdir()
        real = os.path.realpath(plain)
        self.assertEqual(
            pcx_resolve.project(str(plain)), (pcx_resolve.local_key(real), "plain", real)
        )

    def test_pinned_ids_win_in_order(self):
        root = self.tmp
        self.assertIsNone(pcx_resolve.pinned_ids(str(root)))
        (root / "CLAUDE.md").write_text(
            "# CLAUDE.md\n\n**Session ID**  `%s`\n**Workspace ID** | `%s`\n" % (S, W)
        )
        self.assertEqual(
            pcx_resolve.pinned_ids(str(root)), {"session_id": S, "workspace_id": W}
        )
        (root / "CLAUDE.md").write_text(
            "| Session ID | `<SESSION_ID>` |\n|---|---|\n| **Workspace ID** | `<WORKSPACE_ID>` |\n"
        )
        self.assertIsNone(pcx_resolve.pinned_ids(str(root)))
        (root / ".post-cortex.json").write_text(json.dumps({"session_id": "s", "workspace_id": "w"}))
        self.assertEqual(
            pcx_resolve.pinned_ids(str(root)), {"session_id": "s", "workspace_id": "w"}
        )

    def test_resolve_then_cache_per_url(self):
        daemon = FakeDaemon()
        self.addCleanup(daemon.close)
        repo = self.tmp / "proj"
        repo.mkdir()
        git("init", "-q", cwd=repo)
        git("remote", "add", "origin", "https://github.com/a/proj.git", cwd=repo)
        self.write("config.json", {"hosts": {"claude-code": {"workspace": "Team"}}})
        payload = {"cwd": str(repo)}

        ids = pcx_resolve.project_ids(payload, daemon.url, "tok")
        self.assertEqual(ids, {"session_id": S, "workspace_id": W})
        method, path, body, headers = daemon.requests[0]
        self.assertEqual((method, path), ("POST", "/api/projects/resolve"))
        self.assertEqual(
            body,
            {"key": "github.com/a/proj", "name": "proj", "client": "claude-code", "workspace": "Team"},
        )
        self.assertEqual(headers.get("Authorization"), "Bearer tok")
        cache = json.loads((self.config / "projects.json").read_text())
        self.assertEqual(
            cache["github.com/a/proj"],
            {
                "workspace_id": W,
                "session_id": S,
                "url": daemon.url,
                "identity": pcx_resolve.identity("tok"),
            },
        )
        self.assertNotIn("tok", json.dumps(cache).replace(pcx_resolve.identity("tok"), ""))

        # Cached: no second request.
        self.assertEqual(pcx_resolve.project_ids(payload, daemon.url, "tok"), ids)
        self.assertEqual(len(daemon.requests), 1)
        # Another backend: the cache entry does not count.
        self.assertIsNone(pcx_resolve.project_ids(payload, "http://127.0.0.1:9", None))

    def test_another_key_on_the_same_backend_resolves_again(self):
        """Another organisation's key on the same url (found 2026-11-16: a
        new organisation's session took the ids cached for the person's own
        one) is resolved anew; an entry from before identities too."""
        daemon = FakeDaemon()
        self.addCleanup(daemon.close)
        plain = self.tmp / "veil"
        plain.mkdir()
        key = pcx_resolve.project(str(plain))[0]
        mine = {"session_id": "s-mine", "workspace_id": "w-mine"}
        pcx_resolve.cache_ids(key, daemon.url, "key-of-my-org", mine)
        payload = {"cwd": str(plain)}
        self.assertEqual(pcx_resolve.project_ids(payload, daemon.url, "key-of-my-org"), mine)
        self.assertEqual(daemon.requests, [])

        theirs = pcx_resolve.project_ids(payload, daemon.url, "key-of-the-new-org")
        self.assertEqual(theirs, {"session_id": S, "workspace_id": W})
        self.assertEqual(len(daemon.requests), 1)
        entry = json.loads((self.config / "projects.json").read_text())[key]
        self.assertEqual(entry["identity"], pcx_resolve.identity("key-of-the-new-org"))

        # An entry written before identities: a miss with a key.
        self.write("projects.json", {key: dict(mine, url=daemon.url)})
        self.assertEqual(pcx_resolve.project_ids(payload, daemon.url, "tok"), theirs)
        self.assertEqual(len(daemon.requests), 2)

    def test_refusal_fails_open_with_the_server_message(self):
        daemon = FakeDaemon()
        self.addCleanup(daemon.close)
        why = "this key is limited to workspace(s) acme; creating a workspace for a new project needs an organisation-wide key"
        daemon.refuse = (403, why + "\n")
        plain = self.tmp / "y"
        plain.mkdir()
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            self.assertIsNone(pcx_resolve.project_ids({"cwd": str(plain)}, daemon.url, "tok"))
        self.assertEqual(
            err.getvalue(), "post-cortex: /api/projects/resolve: HTTP 403: %s\n" % why
        )
        self.assertFalse((self.config / "projects.json").exists())

    def test_unreachable_backend_fails_open(self):
        plain = self.tmp / "x"
        plain.mkdir()
        self.assertIsNone(pcx_resolve.project_ids({"cwd": str(plain)}, "http://127.0.0.1:9", None))


class Backend(Isolated):
    def test_defaults_without_config(self):
        self.assertEqual(
            pcx_resolve.resolve_server(), (pcx_resolve.CLOUD_URL, None, "cloud")
        )
        self.assertEqual(
            pcx_resolve.host_settings(), {"enabled": True, "capture": False, "workspace": None}
        )

    def test_precedence(self):
        self.write(
            "config.json",
            {
                "active": "work",
                "profiles": {"work": {"url": "https://work.example/"}},
                "hosts": {"claude-code": {"enabled": False, "capture": True}},
            },
        )
        self.write("credentials.json", {"work": {"token": "w"}, "cloud": {"token": "c"}})
        self.assertEqual(pcx_resolve.resolve_server(), ("https://work.example", "w", "work"))
        self.assertEqual(
            pcx_resolve.host_settings(), {"enabled": False, "capture": True, "workspace": None}
        )
        # The plugin's url option: the key of the profile pointing there.
        os.environ["CLAUDE_PLUGIN_OPTION_URL"] = pcx_resolve.CLOUD_URL
        self.assertEqual(pcx_resolve.resolve_server()[:2], (pcx_resolve.CLOUD_URL, "c"))
        os.environ["CLAUDE_PLUGIN_OPTION_API_KEY"] = "plugin-key"
        self.assertEqual(pcx_resolve.resolve_server()[1], "plugin-key")
        # PC_URL and PC_AUTH_TOKEN win.
        os.environ["PC_URL"] = "http://h:1/"
        os.environ["PC_AUTH_TOKEN"] = "env"
        self.assertEqual(pcx_resolve.resolve_server(), ("http://h:1", "env", "work"))
        # A project hook's arguments win over everything.
        del os.environ["PC_AUTH_TOKEN"], os.environ["CLAUDE_PLUGIN_OPTION_API_KEY"]
        self.assertEqual(
            pcx_resolve.resolve_server("https://x.example", "cloud"),
            ("https://x.example", "c", "cloud"),
        )
        # A url alone is a self-hosted daemon.
        self.assertEqual(pcx_resolve.resolve_server("http://d:3737")[2], "self")


class Shadowing(Isolated):
    def test_plugin_steps_aside_for_project_hooks(self):
        project = self.tmp / "p"
        hooks = project / ".claude/hooks"
        hooks.mkdir(parents=True)
        plugin = self.tmp / "plugin/hooks/pcx-card.py"
        plugin.parent.mkdir(parents=True)
        plugin.write_text("")
        os.environ["CLAUDE_PROJECT_DIR"] = str(project)
        self.assertFalse(pcx_resolve.shadowed_by_project(str(plugin)))
        (hooks / "pcx-card.py").write_text("")
        self.assertFalse(pcx_resolve.shadowed_by_project(str(plugin)), "not in settings")
        (project / ".claude/settings.json").write_text('{"x": "pcx-card.py W U"}')
        self.assertTrue(pcx_resolve.shadowed_by_project(str(plugin)))
        # The project's own copy never steps aside.
        self.assertFalse(pcx_resolve.shadowed_by_project(str(hooks / "pcx-card.py")))


@unittest.skipIf(shutil.which("git") is None, "git is not installed")
class Hooks(Isolated):
    """The hook scripts as Claude Code runs them: python3 -I, payload on
    stdin, no arguments (the plugin) or the project's arguments."""

    def run_hook(self, script, payload, *args, extra_env=None):
        env = dict(os.environ, **(extra_env or {}))
        out = subprocess.run(
            [sys.executable, "-I", str(TEMPLATES / script), *args],
            input=json.dumps(payload),
            capture_output=True,
            text=True,
            env=env,
            timeout=20,
        )
        self.assertEqual(out.returncode, 0, out.stderr)
        return out.stdout

    def setUp(self):
        super().setUp()
        self.daemon = FakeDaemon()
        self.addCleanup(self.daemon.close)
        self.repo = self.tmp / "repo"
        self.repo.mkdir()
        git("init", "-q", cwd=self.repo)
        git("remote", "add", "origin", "https://github.com/a/repo.git", cwd=self.repo)

    def test_card_hook_resolves_without_arguments(self):
        out = self.run_hook(
            "pcx-card.py",
            {"cwd": str(self.repo), "hook_event_name": "SessionStart"},
            extra_env={"CLAUDE_PLUGIN_OPTION_URL": self.daemon.url},
        )
        answer = json.loads(out)["hookSpecificOutput"]
        self.assertEqual(answer["hookEventName"], "SessionStart")
        context = answer["additionalContext"]
        self.assertTrue(context.startswith("[PCX] Session: %s | Workspace: %s" % (S, W)))
        self.assertIn("Prefers short answers.", context)
        method, path, body, _ = self.daemon.requests[-1]
        self.assertEqual((method, path), ("POST", "/api/events"))
        self.assertEqual(body["scope"]["workspace_id"], W)
        self.assertEqual(body["event"]["cwd"], str(self.repo))

        # Disabled for claude-code in config.json: nothing.
        self.write("config.json", {"hosts": {"claude-code": {"enabled": False}}})
        out = self.run_hook(
            "pcx-card.py", {"cwd": str(self.repo)}, extra_env={"PC_URL": self.daemon.url}
        )
        self.assertEqual(out, "")

    def test_refused_ids_are_dropped_and_resolved_again(self):
        """Cached ids the backend refuses (a workspace it does not have) are
        taken out of the cache; the hook resolves again and goes on with the
        ids it gets."""
        key = pcx_resolve.project(str(self.repo))[0]
        gone = {"session_id": "s-gone", "workspace_id": "w-gone"}
        pcx_resolve.cache_ids(key, self.daemon.url, None, gone)
        self.daemon.refuse_workspace = ("w-gone", 404, "workspace w-gone not found")
        out = self.run_hook(
            "pcx-card.py", {"cwd": str(self.repo)}, extra_env={"PC_URL": self.daemon.url}
        )
        context = json.loads(out)["hookSpecificOutput"]["additionalContext"]
        self.assertTrue(context.startswith("[PCX] Session: %s | Workspace: %s" % (S, W)))
        paths = [p for _, p, _, _ in self.daemon.requests]
        self.assertEqual(paths, ["/api/events", "/api/projects/resolve", "/api/events"])
        cache = json.loads((self.config / "projects.json").read_text())
        self.assertEqual(cache[key]["workspace_id"], W)

        # Pinned ids are the repository's: refused, they stay, and the hook
        # fails open without asking again.
        self.daemon.requests.clear()
        (self.repo / ".post-cortex.json").write_text(json.dumps(gone))
        out = self.run_hook(
            "pcx-card.py", {"cwd": str(self.repo)}, extra_env={"PC_URL": self.daemon.url}
        )
        self.assertEqual(out, "")
        self.assertEqual([p for _, p, _, _ in self.daemon.requests], ["/api/events"])

    def test_card_hook_with_project_arguments(self):
        out = self.run_hook("pcx-card.py", {}, "W9", self.daemon.url)
        context = json.loads(out)["hookSpecificOutput"]["additionalContext"]
        self.assertNotIn("[PCX] Session", context)
        self.assertIn("workspace_id W9", context)

    def test_banner_resolves_ids_and_yields_to_a_project_banner(self):
        env = {"PC_URL": self.daemon.url, "CLAUDE_PROJECT_DIR": str(self.repo)}
        out = self.run_hook("pcx-banner.py", {"cwd": str(self.repo)}, extra_env=env)
        self.assertTrue(out.startswith("[PCX] Session: %s | Workspace: %s | Rules:" % (S, W)))

        # Unreachable backend: the rules alone.
        out = self.run_hook(
            "pcx-banner.py",
            {"cwd": str(self.repo)},
            extra_env=dict(env, PC_URL="http://127.0.0.1:9", PC_CONFIG_DIR=str(self.tmp / "empty")),
        )
        self.assertTrue(out.startswith("[PCX] Rules:"))

        # The project prints its own banner (`pcx setup --project`): nothing.
        (self.repo / ".claude").mkdir()
        settings = {"hooks": {"UserPromptSubmit": [
            {"hooks": [{"type": "command", "command": "echo '[PCX] Session: S9 | Rules: ...'"}]}
        ]}}
        (self.repo / ".claude/settings.json").write_text(json.dumps(settings))
        self.assertEqual(self.run_hook("pcx-banner.py", {"cwd": str(self.repo)}, extra_env=env), "")

    def test_compact_reinject_prints_the_ids(self):
        out = self.run_hook("pcx-compact-reinject.py", {}, "S9", "W9")
        self.assertIn("**Session:** S9\n**Workspace:** W9", out)
        out = self.run_hook(
            "pcx-compact-reinject.py",
            {"cwd": str(self.repo)},
            extra_env={"PC_URL": self.daemon.url},
        )
        self.assertIn("**Session:** %s" % S, out)
        out = self.run_hook(
            "pcx-compact-reinject.py",
            {"cwd": str(self.repo)},
            extra_env={"PC_URL": "http://127.0.0.1:9"},
        )
        self.assertIn("Mandatory Rules", out)
        self.assertNotIn("**Session:**", out)

    def test_capture_only_when_on(self):
        transcript = self.tmp / "t.jsonl"
        transcript.write_text(
            json.dumps(
                {"type": "user", "uuid": "u1", "sessionId": "c", "message": {"content": "hello"}}
            )
            + "\n"
        )
        payload = {"cwd": str(self.repo), "transcript_path": str(transcript)}
        env = {"PC_URL": self.daemon.url}
        self.assertEqual(self.run_hook("pcx-capture.py", payload, "--dry-run", extra_env=env), "")
        self.write("config.json", {"hosts": {"claude-code": {"capture": True}}})
        body = json.loads(self.run_hook("pcx-capture.py", payload, "--dry-run", extra_env=env))
        self.assertEqual(body["scope"]["session_id"], S)
        self.assertEqual(body["event"]["hook_event_name"], "Stop")
        self.assertEqual(body["event"]["payload"]["messages"][0]["text"], "hello")

    def test_file_context_asks_for_each_file_an_mcp_edit_names(self):
        payload = {
            "cwd": str(self.repo),
            "hook_event_name": "PreToolUse",
            "tool_name": "mcp__plugin_latt_latt__replace_text",
            "tool_input": {"edits": [{"file": "src/a.rs"}, {"file": "src/b.rs"}, {"file": "src/a.rs"}]},
        }
        out = self.run_hook("pcx-file-context.py", payload, extra_env={"PC_URL": self.daemon.url})
        events = [b["event"] for m, p, b, _ in self.daemon.requests if p == "/api/events"]
        self.assertEqual(
            [e["tool_input"] for e in events],
            [{"file_path": str(self.repo / "src/a.rs")}, {"file_path": str(self.repo / "src/b.rs")}],
        )
        self.assertEqual({e["hook_event_name"] for e in events}, {"PreToolUse"})
        answer = json.loads(out)["hookSpecificOutput"]
        self.assertEqual(answer["hookEventName"], "PreToolUse")
        self.assertEqual(answer["additionalContext"].count("## post-cortex cards"), 2)

    def test_a_held_edit_is_denied_with_the_file_s_memory(self):
        """The memory holds a file's first edit once: the hook denies the
        call with the memory as the reason the agent reads."""

        def answer(body):
            path = body["event"]["tool_input"]["file_path"]
            if path.endswith("a.rs"):
                return {"decision": "block", "reason": "post-cortex memory for %s" % path}
            return {}

        self.daemon.event_answer = answer
        payload = {
            "cwd": str(self.repo),
            "session_id": "agent-1",
            "hook_event_name": "PreToolUse",
            "tool_name": "MultiEdit",
            "tool_input": {"file_path": "src/a.rs", "edits": [{"file": "src/b.rs"}]},
        }
        out = json.loads(
            self.run_hook("pcx-file-context.py", payload, extra_env={"PC_URL": self.daemon.url})
        )["hookSpecificOutput"]
        self.assertEqual(out["hookEventName"], "PreToolUse")
        self.assertEqual(out["permissionDecision"], "deny")
        self.assertEqual(
            out["permissionDecisionReason"], "post-cortex memory for %s" % (self.repo / "src/a.rs")
        )
        self.assertNotIn("additionalContext", out)
        events = [b["event"] for m, p, b, _ in self.daemon.requests if p == "/api/events"]
        self.assertEqual({e["session_id"] for e in events}, {"agent-1"})

        # Nothing held and nothing known: the edit goes on, nothing printed.
        self.daemon.event_answer = lambda body: {}
        self.assertEqual(
            self.run_hook("pcx-file-context.py", payload, extra_env={"PC_URL": self.daemon.url}), ""
        )

    def test_a_read_adds_the_file_s_memory_without_sending_the_file(self):
        self.daemon.event_answer = lambda body: {
            "hookSpecificOutput": {
                "hookEventName": body["event"]["hook_event_name"],
                "additionalContext": "post-cortex memory for a.rs",
            }
        }
        payload = {
            "cwd": str(self.repo),
            "session_id": "agent-1",
            "hook_event_name": "PostToolUse",
            "tool_name": "Read",
            "tool_input": {"file_path": "a.rs"},
            "tool_response": {"file": {"content": "secret source"}},
        }
        out = json.loads(
            self.run_hook("pcx-file-context.py", payload, extra_env={"PC_URL": self.daemon.url})
        )["hookSpecificOutput"]
        self.assertEqual(out, {
            "hookEventName": "PostToolUse",
            "additionalContext": "post-cortex memory for a.rs",
        })
        (event,) = [b["event"] for m, p, b, _ in self.daemon.requests if p == "/api/events"]
        self.assertEqual(event["hook_event_name"], "PostToolUse")
        self.assertNotIn("tool_response", event)

        # A read's block is not an edit's: never a deny.
        self.daemon.event_answer = lambda body: {"decision": "block", "reason": "no"}
        self.assertEqual(
            self.run_hook("pcx-file-context.py", payload, extra_env={"PC_URL": self.daemon.url}), ""
        )

    def test_file_context_and_stop_check_fail_open(self):
        payload = {
            "cwd": str(self.repo),
            "hook_event_name": "PreToolUse",
            "tool_input": {"file_path": "a.rs"},
        }
        env = {"PC_URL": "http://127.0.0.1:9"}
        self.assertEqual(self.run_hook("pcx-file-context.py", payload, extra_env=env), "")
        self.assertEqual(
            self.run_hook("pcx-stop-check.py", {"cwd": str(self.repo)}, extra_env=env), ""
        )


if __name__ == "__main__":
    unittest.main()
