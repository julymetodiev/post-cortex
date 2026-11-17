import assert from "node:assert/strict";
import { test } from "node:test";

import { RECALL_HEADING, idsLine, joinTexts, patchPaths, recallText } from "../src/format.js";
import { isInjected } from "../src/text.js";

const W = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
const S = "11111111-2222-3333-4444-555555555555";

test("ids line, as the Claude Code plugin prints it", () => {
  assert.equal(idsLine({ session_id: S, workspace_id: W }), `[PCX] Session: ${S} | Workspace: ${W}`);
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
