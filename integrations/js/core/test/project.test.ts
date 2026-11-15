import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, realpathSync, symlinkSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import {
  cacheIds,
  cachedIds,
  gitConfigDir,
  gitOriginFromConfig,
  gitRoot,
  localKey,
  normalizeRemote,
  originFromConfigText,
  pinnedIds,
  project,
  projectRelative,
  uuidAfter,
} from "../src/project.js";
import { tempEnv } from "../src/testing.js";

// build-test/test/project.test.js -> integrations/testdata/project-keys.json
const here = dirname(fileURLToPath(import.meta.url));
const vectors = JSON.parse(
  readFileSync(join(here, "..", "..", "..", "..", "testdata", "project-keys.json"), "utf8"),
) as {
  remotes: Array<{ url: string; key: string | null; name: string | null }>;
  local: Array<{ root: string; key: string }>;
};

test("shared remote vectors", () => {
  assert.ok(vectors.remotes.length > 0);
  for (const v of vectors.remotes) {
    const key = normalizeRemote(v.url);
    assert.equal(key, v.key, JSON.stringify(v.url));
    if (key) assert.equal(key.slice(key.lastIndexOf("/") + 1), v.name, JSON.stringify(v.url));
  }
});

test("shared local vectors", () => {
  assert.ok(vectors.local.length > 0);
  for (const v of vectors.local) assert.equal(localKey(v.root), v.key, v.root);
});

const S = "11111111-2222-3333-4444-555555555555";
const W = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

function hasGit(): boolean {
  try {
    execFileSync("git", ["--version"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

test("not a repository: local key of the directory, its name", async () => {
  const t = tempEnv();
  try {
    const real = realpathSync.native(t.projectDir);
    const found = await project(t.projectDir, async () => null);
    // tmp dirs may sit inside a repository on some machines; only assert when not
    if (gitRoot(real) === null) {
      assert.deepEqual(found, { key: localKey(real), name: "project", root: real });
    }
  } finally {
    t.cleanup();
  }
});

test("repository with origin: remote key; subdirectories resolve to the root", { skip: !hasGit() }, async () => {
  const t = tempEnv();
  try {
    execFileSync("git", ["init", "-q", t.projectDir]);
    execFileSync("git", ["-C", t.projectDir, "remote", "add", "origin", "git@GitHub.com:me/My-Repo.git"]);
    mkdirSync(join(t.projectDir, "src", "deep"), { recursive: true });
    const found = await project(join(t.projectDir, "src", "deep"));
    assert.equal(found.key, "github.com/me/My-Repo");
    assert.equal(found.name, "My-Repo");
    assert.equal(found.root, realpathSync.native(t.projectDir));
  } finally {
    t.cleanup();
  }
});

test("repository without origin: local key of the git root", { skip: !hasGit() }, async () => {
  const t = tempEnv();
  try {
    execFileSync("git", ["init", "-q", t.projectDir]);
    mkdirSync(join(t.projectDir, "sub"));
    const found = await project(join(t.projectDir, "sub"));
    const root = realpathSync.native(t.projectDir);
    assert.deepEqual(found, { key: localKey(root), name: "project", root });
  } finally {
    t.cleanup();
  }
});

test("origin from .git/config without git, worktree pointer files too", () => {
  const t = tempEnv();
  try {
    const main = join(t.projectDir, "main");
    mkdirSync(join(main, ".git", "worktrees", "wt"), { recursive: true });
    writeFileSync(
      join(main, ".git", "config"),
      '[core]\n\tbare = false\n[remote "upstream"]\n\turl = https://x/y/z\n[remote "origin"]\n\turl = https://github.com/a/b.git # comment\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n',
    );
    assert.equal(gitOriginFromConfig(main), "https://github.com/a/b.git");
    writeFileSync(join(main, ".git", "worktrees", "wt", "commondir"), "../..\n");
    const wt = join(t.projectDir, "wt");
    mkdirSync(wt);
    writeFileSync(join(wt, ".git"), `gitdir: ${join(main, ".git", "worktrees", "wt")}\n`);
    assert.equal(gitConfigDir(wt), join(main, ".git"));
    assert.equal(gitOriginFromConfig(wt), "https://github.com/a/b.git");
    assert.equal(gitOriginFromConfig(t.projectDir), null);
  } finally {
    t.cleanup();
  }
});

test("git config parsing", () => {
  assert.equal(originFromConfigText('[remote "origin"]\n  URL = "git@h:a/b.git"\n'), "git@h:a/b.git");
  assert.equal(originFromConfigText('[remote "other"]\nurl = x\n'), null);
  assert.equal(originFromConfigText('[Remote "origin"]\n; url = no\nurl=git@h:c/d\n'), "git@h:c/d");
});

test("pinned ids: .post-cortex.json, else AGENTS.md / CLAUDE.md", () => {
  const t = tempEnv();
  try {
    const root = t.projectDir;
    assert.equal(pinnedIds(root), null);
    writeFileSync(join(root, "CLAUDE.md"), `**Session ID**  \`${S}\`\n**Workspace ID** | \`${W}\`\n`);
    assert.deepEqual(pinnedIds(root), { session_id: S, workspace_id: W });
    assert.equal(pinnedIds(root, ["AGENTS.md"]), null);
    writeFileSync(join(root, ".post-cortex.json"), JSON.stringify({ session_id: "s", workspace_id: "w" }));
    assert.deepEqual(pinnedIds(root), { session_id: "s", workspace_id: "w" });
    writeFileSync(join(root, ".post-cortex.json"), JSON.stringify({ session_id: "s" }));
    assert.deepEqual(pinnedIds(root), { session_id: S, workspace_id: W });
  } finally {
    t.cleanup();
  }
});

test("uuid after a label skips markdown and non-uuids", () => {
  assert.equal(uuidAfter(`Session ID: not-one\nSession ID | \`${S}\``, "Session ID"), S);
  assert.equal(uuidAfter("Session ID: 1234", "Session ID"), null);
});

test("cache: per key, valid for the same url only, other keys kept", () => {
  const t = tempEnv();
  try {
    writeFileSync(join(t.configDir, "projects.json"), JSON.stringify({ other: { x: 1 } }));
    assert.equal(cachedIds(t.configDir, "k", "http://a"), null);
    cacheIds(t.configDir, "k", "http://a/", { session_id: S, workspace_id: W });
    assert.deepEqual(cachedIds(t.configDir, "k", "http://a"), { session_id: S, workspace_id: W });
    assert.equal(cachedIds(t.configDir, "k", "http://b"), null);
    const file = JSON.parse(readFileSync(join(t.configDir, "projects.json"), "utf8"));
    assert.deepEqual(file.other, { x: 1 });
    assert.deepEqual(file.k, { workspace_id: W, session_id: S, url: "http://a" });
  } finally {
    t.cleanup();
  }
});

test("project-relative paths", () => {
  const t = tempEnv();
  try {
    const root = realpathSync.native(t.projectDir);
    mkdirSync(join(root, "src"));
    assert.equal(projectRelative(join(root, "src", "a.rs"), root), "src/a.rs");
    assert.equal(projectRelative("a.rs", root, join(root, "src")), "src/a.rs");
    assert.equal(projectRelative("/elsewhere/a.rs", root), "/elsewhere/a.rs");
    // through a symlinked directory
    const link = join(t.configDir, "link");
    symlinkSync(root, link);
    assert.equal(projectRelative(join(link, "src", "new.rs"), root, link), "src/new.rs");
  } finally {
    t.cleanup();
  }
});
