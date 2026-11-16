#!/usr/bin/env python3
"""pcx-stop-check.py on transcripts written by hand."""

import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest

HOOK = pathlib.Path(__file__).resolve().parents[2] / "crates/post-cortex-cli/templates/pcx-stop-check.py"
LOG = "mcp__plugin_post-cortex_post-cortex__update_conversation_context"


def user(text):
    return {"type": "user", "message": {"content": text}}


def tool(name, **args):
    return {"type": "assistant", "message": {"content": [{"type": "tool_use", "name": name, "input": args}]}}


class StopCheck(unittest.TestCase):
    def run_hook(self, entries, *flags, **payload):
        with tempfile.TemporaryDirectory() as root:
            root = str(pathlib.Path(root).resolve())
            transcript = pathlib.Path(root) / "t.jsonl"
            # "{root}" in a path is the project.
            lines = (json.dumps(e).replace("{root}", root) for e in entries)
            transcript.write_text("\n".join(lines))
            payload = json.loads(json.dumps(payload).replace("{root}", root))
            data = dict({"transcript_path": str(transcript), "cwd": root}, **payload)
            # With a session id the hook does not look for a project's own copy.
            # "{root}/cfg" is Claude Code's config directory.
            env = dict(os.environ, CLAUDE_CONFIG_DIR=root + "/cfg")
            out = subprocess.run(
                [sys.executable, "-I", str(HOOK), *flags, "S"],
                input=json.dumps(data),
                capture_output=True,
                text=True,
                timeout=20,
                env=env,
            )
            self.assertEqual(out.returncode, 0, out.stderr)
            self.assertEqual(out.stderr, "")
            return root, out.stdout

    def blocks(self, entries, words=""):
        _, out = self.run_hook(entries)
        self.assertTrue(out, "expected a block")
        body = json.loads(out)
        self.assertEqual(body["decision"], "block")
        self.assertIn(words, body["reason"])

    def passes(self, entries):
        _, out = self.run_hook(entries)
        self.assertEqual(out, "")

    def test_a_question_passes(self):
        self.passes([user("why?")])

    def test_reading_passes(self):
        self.passes([user("why?"), tool("Read", file_path="/a"), tool("mcp__plugin_mimir_mimir__grep", pattern="a")])

    def test_any_shell_command_blocks(self):
        self.blocks([user("measure"), tool("Bash", command="python3 probe.py")], "ran `python3 probe.py`")

    def test_a_command_that_only_reads_passes(self):
        for command in (
            "ls -la",
            "sleep 1; true",
            "cd /p && git -C /p status --short | head",
            "git log --oneline -2 && git worktree list",
            "grep -n x a.py 2>/dev/null | tail -5",
            "cat a.log 2>&1 | grep -E 'rc=' | head\nwc -l a b",
            "FOO=1 head -3 < in.txt",
            "find . -name '*.rs' | sort",
            "sed -n 1,20p a.md",
            "grep -oE '^#+ `?[a-z_]+`?$' a.md | sort -u | head",
            "L=$(ls -t logs | head -1); grep -c PASS $L",
            "until grep -q Summary a.log; do sleep 10; done; tail -3 a.log",
            "for f in a b; do wc -l $f; done",
            "diff <(sort a) <(sort b)",
            "echo $((1 + 2))",
        ):
            with self.subTest(command=command):
                self.passes([user("look"), tool("Bash", command=command)])
        self.passes([user("look"), tool("mcp__plugin_post-cortex_post-cortex__bash", command="ls")])

    def test_a_command_that_does_more_than_read_blocks(self):
        for command in (
            "python3 probe.py",
            "ls; rm a",
            "ls\nrm a",
            "echo x > a.txt",
            "grep x a >> out",
            "cat $(make)",
            "sed -i s/a/b/ f",
            "find . -delete",
            "git commit -m x",
            "git -C /p push",
            "ssh host ls",
            "ls | xargs rm",
            "echo 'unclosed",
            "L=$(make); cat $L",
            "echo `rm a`",
            "until make; do sleep 1; done",
            "for f in a b; do rm $f; done",
            "cat $(ls",
        ):
            with self.subTest(command=command):
                self.blocks([user("do"), tool("Bash", command=command)], "ran `")

    def test_a_shell_command_with_no_text_passes(self):
        self.passes([user("measure"), tool("Bash", command="")])

    def test_a_shell_command_through_mcp_blocks(self):
        shell = "mcp__plugin_post-cortex_post-cortex__"
        self.blocks([user("run"), tool(shell + "bash", command="make")], "ran `make`")
        self.blocks([user("run"), tool(shell + "tmux_start", command="serve")], "ran `serve`")
        self.blocks([user("run"), tool(shell + "tmux_send", session="s", text="make")], "ran `make`")

    def test_reading_a_shell_session_passes(self):
        self.passes([user("q"), tool("mcp__plugin_post-cortex_post-cortex__tmux_read", session="s")])

    def test_a_log_after_it_passes(self):
        self.passes([user("measure"), tool("Bash", command="make"), tool(LOG, session_id="S")])

    def test_a_command_after_the_log_blocks(self):
        self.blocks([user("measure"), tool(LOG), tool("Bash", command="make")])

    def test_only_this_turn_counts(self):
        self.passes([user("measure"), tool("Bash", command="make"), user("and now?")])

    def test_an_mcp_edit_by_symbol_blocks(self):
        self.blocks([user("fix"), tool("mcp__plugin_mimir_mimir__edit_symbols", symbol="f")], "edited f,")

    def test_a_record_covers_only_the_files_it_names(self):
        edits = [
            user("fix"),
            tool("Edit", file_path="{root}/src/a.rs"),
            tool("mcp__plugin_latt_latt__replace_text", edits=[{"file": "src/b.rs"}, {"file": "src/c.rs"}]),
        ]
        self.blocks(edits + [tool(LOG, content={"file": "a.rs", "changes": "x"})], "edited b.rs, c.rs,")
        self.passes(edits + [tool(LOG, updates=[{"content": {"file": "src/a.rs, b.rs and c.rs"}}])])

    def test_an_edit_after_its_record_blocks_again(self):
        self.blocks(
            [user("fix"), tool("Write", file_path="{root}/a.rs"), tool(LOG, content={"file": "a.rs"}),
             tool("Write", file_path="{root}/a.rs")],
            "edited a.rs,",
        )

    def test_an_edit_by_symbol_is_covered_by_naming_it(self):
        self.passes([user("fix"), tool("mcp__plugin_latt_latt__edit_symbols", edits=[{"symbol": "Typed::scan"}]),
                     tool(LOG, content={"changes": "Typed::scan reads pages"})])

    def test_an_edit_in_git_passes(self):
        self.passes([user("fix"), tool("Write", file_path="{root}/.git/config")])

    def test_every_mcp_edit_blocks(self):
        for name in ("insert_after", "edit_lines", "replace_text", "write_file"):
            self.blocks([user("fix"), tool("mcp__plugin_mimir_mimir__" + name, file="src/a.rs")], "edited a.rs")
        for name in ("edit", "write"):
            self.blocks([user("fix"), tool("mcp__plugin_post-cortex_post-cortex__" + name, path="a.rs")], "edited a.rs")

    def test_an_mcp_edit_outside_the_project_passes(self):
        self.passes([user("fix"), tool("mcp__plugin_post-cortex_post-cortex__edit", path="/elsewhere/a.rs")])

    def test_a_tool_named_alike_passes(self):
        self.passes([user("q"), tool("mcp__plugin_post-cortex_post-cortex__task_update", task="t")])

    def test_an_edit_outside_the_project_passes(self):
        self.passes([user("fix"), tool("Write", file_path="/elsewhere/a.rs")])

    def test_a_write_into_claude_codes_auto_memory_blocks(self):
        """Found 2026-11-16: "Remember: ..." went to Claude Code's own
        memory/MEMORY.md, outside the project, and nothing objected."""
        memory = "{root}/cfg/projects/-Users-a-veil/memory/"
        self.blocks(
            [user("Remember: we publish every Sunday"), tool("Write", file_path=memory + "publishing.md"),
             tool("Edit", file_path=memory + "MEMORY.md")],
            "This turn wrote publishing.md, MEMORY.md went to Claude Code's built-in auto memory",
        )
        self.blocks([user("x"), tool("Write", file_path=memory + "a.md")], "update_conversation_context, session_id S")
        # Recorded in post-cortex after it: passes.
        self.passes([user("x"), tool("Write", file_path=memory + "a.md"), tool(LOG, content={"title": "Sunday"})])
        # Elsewhere in the config directory (here inside the project): no
        # auto memory.
        _, out = self.run_hook([user("x"), tool("Write", file_path="{root}/cfg/projects/-Users-a-veil/t.jsonl")])
        self.assertNotIn("auto memory", json.loads(out)["reason"])

    def test_a_custom_auto_memory_directory_counts(self):
        def entries(root):
            return [user("x"), tool("Write", file_path=root + "/notes/m.md")]

        _, out = self.run_hook_with_settings({"autoMemoryDirectory": "{root}/notes"}, entries)
        self.assertIn("m.md went to Claude Code's built-in auto memory", json.loads(out)["reason"])
        _, out = self.run_hook_with_settings({}, entries)
        self.assertEqual(out, "")

    def run_hook_with_settings(self, settings, entries_of):
        """run_hook with Claude Code's user settings.json written first."""
        with tempfile.TemporaryDirectory() as root:
            root = str(pathlib.Path(root).resolve())
            cfg = pathlib.Path(root) / "cfg"
            cfg.mkdir()
            (cfg / "settings.json").write_text(json.dumps(settings).replace("{root}", root))
            transcript = pathlib.Path(root) / "t.jsonl"
            transcript.write_text("\n".join(json.dumps(e) for e in entries_of(root)))
            out = subprocess.run(
                [sys.executable, "-I", str(HOOK), "S"],
                input=json.dumps({"transcript_path": str(transcript), "cwd": root + "/proj"}),
                capture_output=True,
                text=True,
                timeout=20,
                env=dict(os.environ, CLAUDE_CONFIG_DIR=str(cfg)),
            )
            self.assertEqual(out.returncode, 0, out.stderr)
            return root, out.stdout

    def test_a_task_ending_blocks(self):
        self.blocks([user("<task-notification>\n<task-id>x</task-id>\n<status>completed</status>")], "background task")

    def test_a_task_ending_with_a_log_passes(self):
        ended = "<task-notification>\n<task-id>x</task-id>\n<status>failed</status>"
        self.passes([user(ended), tool("Read", file_path="/o"), tool(LOG)])

    def test_a_monitor_event_passes(self):
        self.passes([user("<task-notification>\n<task-id>m</task-id>\n<event>59s: 549 crates</event>")])

    def test_a_correction_without_supersedes_blocks(self):
        said = {"question": "Was P7 verified?", "answer": "No: the record before turned out wrong."}
        self.blocks([user("check"), tool(LOG, content=said)], "`supersedes`")
        self.blocks([user("check"), tool(LOG, updates=[{"content": {"title": "x"}},
                                                       {"content": {"title": "Outdated: Postgres"}}])],
                    "(\"Outdated\")")

    def test_a_correction_with_supersedes_passes(self):
        said = {"answer": "The earlier check was wrong."}
        self.passes([user("check"), tool(LOG, content=said, supersedes=["a"])])
        self.passes([user("check"), tool(LOG, updates=[{"content": said, "supersedes": ["a"]}])])

    def test_a_record_that_corrects_nothing_passes(self):
        self.passes([user("check"), tool(LOG, content={"problem": "wrongly named", "solution": "renamed"})])
        self.passes([user("check"), tool(LOG, content={"changes": "shown only for a record that no longer holds"})])
        self.passes([user("check"), tool(LOG, content={"changes": "the hook asks for supersedes on a correction"})])

    def test_a_correction_and_an_unrecorded_command_say_both(self):
        _, out = self.run_hook([user("m"), tool(LOG, content={"answer": "it was wrong"}),
                                tool("Bash", command="make")])
        reason = json.loads(out)["reason"]
        self.assertIn("ran `make`", reason)
        self.assertIn("`supersedes`", reason)

    def test_it_asks_once(self):
        _, out = self.run_hook([user("m"), tool("Bash", command="make")], stop_hook_active=True)
        self.assertEqual(out, "")


class Remind(unittest.TestCase):
    """--remind, the PostToolUse hook of the edit tools."""

    run_hook = StopCheck.run_hook

    def remind(self, done, now, use_id="now"):
        """What --remind says after `now` (a file), `done` files edited before."""
        entries = [user("fix")] + [tool("Write", file_path="{root}/f%d.rs" % i) for i in range(done)]
        entries.append({"type": "assistant", "message": {"content": [
            {"type": "tool_use", "id": use_id, "name": "Write", "input": {"file_path": "{root}/%s" % now}}]}})
        _, out = self.run_hook(entries, "--remind", tool_name="Write", tool_use_id=use_id,
                               tool_input={"file_path": "{root}/%s" % now})
        return json.loads(out)["hookSpecificOutput"]["additionalContext"] if out else None

    def test_few_files_are_not_reminded(self):
        self.assertIsNone(self.remind(6, "new.rs"))

    def test_the_eighth_file_reminds_naming_them(self):
        said = self.remind(7, "new.rs")
        self.assertIn("8 files edited and not recorded yet", said)
        self.assertIn("f0.rs", said)
        self.assertIn("new.rs", said)

    def test_a_file_edited_again_reminds_no_more(self):
        self.assertIsNone(self.remind(8, "f3.rs"))

    def test_a_record_starts_the_count_again(self):
        entries = [user("fix")] + [tool("Write", file_path="{root}/f%d.rs" % i) for i in range(9)]
        entries.append(tool(LOG, content={"file": " ".join("f%d.rs" % i for i in range(9))}))
        _, out = self.run_hook(entries, "--remind", tool_name="Write", tool_use_id="x",
                               tool_input={"file_path": "{root}/g.rs"})
        self.assertEqual(out, "")

    def test_a_write_into_the_auto_memory_is_reminded_at_once(self):
        said = self.remind(0, "cfg/projects/-Users-a-veil/memory/MEMORY.md")
        self.assertIn("[PCX] MEMORY.md went to Claude Code's built-in auto memory", said)
        self.assertIn("update_conversation_context, session_id S", said)


if __name__ == "__main__":
    unittest.main()
