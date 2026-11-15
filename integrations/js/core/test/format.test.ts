import assert from "node:assert/strict";
import { test } from "node:test";

import { RECALL_HEADING, cardsText, fileContextText, idsLine, joinTexts, patchPaths, recallText } from "../src/format.js";
import { isInjected } from "../src/text.js";

const W = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
const S = "11111111-2222-3333-4444-555555555555";

test("cards text as common.rs prints it", () => {
  const cards = { user: { text: " be brief \n", version: 2 }, workspace: { text: "Rust.", version: 5 } };
  assert.equal(
    cardsText(cards, W),
    "## post-cortex cards\n\n### User card (v2)\nbe brief\n\n### Project card (v5)\nRust.",
  );
});

test("no project card: how to write one; null cards and missing versions", () => {
  assert.equal(
    cardsText({ user: null, workspace: null }, W),
    "## post-cortex cards\n\nNo project card yet. Once you know the stack, conventions and current goals, " +
      `write a short one (max 2 KB): manage_workspace action update_card, workspace_id ${W}, expected_version 0.`,
  );
  // serde_json Display: a missing version prints `null`, a string keeps its quotes
  assert.match(cardsText({ user: { text: "x" } }, W), /### User card \(vnull\)\nx/);
  assert.match(cardsText({ user: { text: "x", version: "3" } }, W), /### User card \(v"3"\)\nx/);
});

test("ids line comes first in global mode", () => {
  const text = cardsText({ workspace: { text: "p", version: 1 } }, W, { session_id: S, workspace_id: W });
  assert.equal(
    text,
    `## post-cortex cards\n\n[PCX] Session: ${S} | Workspace: ${W}\n\n### Project card (v1)\np`,
  );
  assert.equal(idsLine({ session_id: S, workspace_id: W }), `[PCX] Session: ${S} | Workspace: ${W}`);
});

test("file context text", () => {
  const long = "é".repeat(250);
  const view = {
    problems: [
      { id: "e1", title: "Fixed race", description: ` ${long} `, timestamp: "2026-10-01T12:00:00Z" },
      { id: "e2", title: "Old", validity: { valid_to: "2026-10-02", reason: "superseded" } },
      { id: "e3", title: "Gone", validity: { valid_to: "2026-10-02" } },
      { id: "e4", title: "fourth is cut" },
    ],
    decisions: [{ title: "Use tokio", timestamp: "2026-09-01" }],
    dependents: [{ entity: "a.rs" }, { entity: 3 }, {}, { entity: "b.rs" }],
  };
  assert.equal(
    fileContextText(view, "src/x.rs"),
    "post-cortex memory for src/x.rs:\n\n" +
      "Problems solved here before:\n" +
      `- 2026-10-01 Fixed race: ${"é".repeat(200)} [entry e1]\n` +
      "-  Old (no longer holds: superseded) [entry e2]\n" +
      "-  Gone (no longer holds: ended) [entry e3]\n\n" +
      "Decisions and requirements about it:\n" +
      "- 2026-09-01 Use tokio [entry ]\n\n" +
      "Depends on it: a.rs, b.rs",
  );
});

test("nothing known about a file is null", () => {
  assert.equal(fileContextText({ problems: [], decisions: [], dependents: [] }, "a"), null);
  assert.equal(fileContextText({}, "a"), null);
  assert.equal(fileContextText(null, "a"), null);
});

test("joined texts", () => {
  assert.equal(joinTexts([null, "a", null, "b"]), "a\n\nb");
  assert.equal(joinTexts([null]), null);
});

test("patch paths come from the file headers (common.rs vector)", () => {
  const patch =
    "*** Begin Patch\n*** Update File: src/a.rs\n@@\n-x\n+y\n*** Add File: src/b.rs\n+new\n*** Delete File: src/c.rs\n*** Update File: src/a.rs\n*** Move to: src/d.rs\n*** End Patch\n";
  assert.deepEqual(patchPaths(patch), ["src/a.rs", "src/b.rs", "src/d.rs"]);
  assert.deepEqual(patchPaths("ls -la"), []);
  assert.deepEqual(patchPaths("  *** Update File:  spaced.rs \r\n"), ["spaced.rs"]);
});

test("recall text: one line per hit, snippets cut at 200 characters; none is null", () => {
  const hit = (id: string, title: string, snippet: string) => ({
    id,
    content_type: "DecisionPoint",
    title,
    snippet,
    score: 0.5,
    timestamp: "2026-10-02T09:00:00Z",
    author: null,
    session_id: S,
  });
  const text = recallText([hit("e1", "Use RocksDB", " Embedded. "), hit("e2", "Only a title", ""), hit("e3", "Long", "x".repeat(250))]);
  assert.equal(
    text,
    `${RECALL_HEADING}\n\n- 2026-10-02 Use RocksDB: Embedded. [entry e1]\n- 2026-10-02 Only a title [entry e2]\n- 2026-10-02 Long: ${"x".repeat(200)} [entry e3]`,
  );
  assert.ok(isInjected(text ?? ""), "never captured back");
  assert.equal(recallText([]), null);
});
