// Project resolution: which post-cortex session and workspace a directory
// belongs to. Same algorithm as pcx/project.rs and templates/pcx_resolve.py;
// the vectors in integrations/testdata/project-keys.json are asserted by all
// of them.
//
// Project key:
//   1. the git remote `origin` of the repository the directory is in,
//      normalised (no scheme, user or port, no `.git`, `git@host:a/b` as
//      `host/a/b`, host lowercased);
//   2. no such remote: `local:<sha256 of the absolute git root, 16 hex>`;
//   3. not a git repository: the same with the directory.
// The name is the last segment of the remote path, else the root's name.
//
// Ids, first hit wins:
//   1. `.post-cortex.json` {"session_id","workspace_id"} at the root;
//   2. the ids in an instructions file at the root (AGENTS.md, CLAUDE.md);
//   3. the cache projects.json in the config directory, keyed by project key,
//      valid for the same backend URL;
//   4. POST /api/projects/resolve, whose answer is cached.

import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, realpathSync, renameSync, statSync, writeFileSync } from "node:fs";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";

import { clean, readJsonObject } from "./config.js";

/** The file with a project's fixed ids, at its root. */
export const PINNED_FILE = ".post-cortex.json";
/** The cache of resolved projects, in the config directory. */
export const CACHE_FILE = "projects.json";
/** Instructions files a project-mode setup wrote the ids into. */
export const INSTRUCTION_FILES = ["AGENTS.md", "CLAUDE.md"] as const;

export interface Ids {
  session_id: string;
  workspace_id: string;
}

export interface Project {
  key: string;
  name: string;
  /** The git root, else the directory itself (real path). */
  root: string;
}

const SCHEME = /^([A-Za-z][A-Za-z0-9+.-]*):\/\/(.*)$/s;

/** `host/path` of a git remote URL, or null when it names no host. */
export function normalizeRemote(raw: string | null | undefined): string | null {
  const url = (raw ?? "").trim();
  if (!url || url.includes("\\")) return null;
  let host: string;
  let path: string;
  const match = SCHEME.exec(url);
  if (match) {
    const scheme = match[1] ?? "";
    const rest = match[2] ?? "";
    if (scheme.toLowerCase() === "file") return null;
    const slash = rest.indexOf("/");
    const authority = slash === -1 ? rest : rest.slice(0, slash);
    path = slash === -1 ? "" : rest.slice(slash + 1);
    const at = authority.lastIndexOf("@");
    const hostPort = at === -1 ? authority : authority.slice(at + 1);
    host = hostPort.startsWith("[")
      ? `[${hostPort.slice(1).split("]")[0] ?? ""}]`
      : (hostPort.split(":")[0] ?? "");
  } else {
    // scp-like `[user@]host:path`: a colon before any slash.
    const colon = url.indexOf(":");
    const slash = url.indexOf("/");
    if (colon <= 0 || (slash !== -1 && slash < colon)) return null;
    const userHost = url.slice(0, colon);
    const at = userHost.lastIndexOf("@");
    host = at === -1 ? userHost : userHost.slice(at + 1);
    if ([...host].length < 2) return null; // a Windows drive letter
    path = url.slice(colon + 1);
  }
  host = host.toLowerCase();
  path = path.replace(/^\/+|\/+$/g, "");
  if (path.endsWith(".git")) path = path.slice(0, -4).replace(/\/+$/, "");
  if (!host || !path) return null;
  return `${host}/${path}`;
}

/** `local:<first 16 hex of sha256(root)>`. */
export function localKey(root: string): string {
  return `local:${createHash("sha256").update(root, "utf8").digest("hex").slice(0, 16)}`;
}

/** The nearest directory at or above `dir` holding `.git` (a directory, or the file of a worktree or submodule). */
export function gitRoot(dir: string): string | null {
  let current = dir;
  for (;;) {
    if (existsSync(join(current, ".git"))) return current;
    const parent = dirname(current);
    if (parent === current) return null;
    current = parent;
  }
}

/** `remote.origin.url` through `git`, or null when git is missing or has none. */
export function gitOriginViaGit(root: string, timeoutMs = 3000): Promise<string | null> {
  return new Promise((done) => {
    try {
      execFile(
        "git",
        ["-C", root, "config", "--get", "remote.origin.url"],
        { timeout: timeoutMs, windowsHide: true },
        (error, stdout) => done(error ? null : (clean(String(stdout)) ?? null)),
      );
    } catch {
      done(null);
    }
  });
}

/** The git directory whose `config` holds the remotes: `.git`, or for a worktree or submodule the one its `.git` file points to (its common dir). */
export function gitConfigDir(root: string): string | null {
  const dotGit = join(root, ".git");
  try {
    if (statSync(dotGit).isDirectory()) return dotGit;
    const pointer = /^gitdir:\s*(.+)$/m.exec(readFileSync(dotGit, "utf8"));
    if (!pointer?.[1]) return null;
    const gitDir = resolve(root, pointer[1].trim());
    try {
      const common = readFileSync(join(gitDir, "commondir"), "utf8").trim();
      if (common) return resolve(gitDir, common);
    } catch {
      // no commondir: a submodule, whose git dir has its own config
    }
    return gitDir;
  } catch {
    return null;
  }
}

/** `url` of `[remote "origin"]` in a git config file's text. */
export function originFromConfigText(text: string): string | null {
  let inOrigin = false;
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line || line.startsWith("#") || line.startsWith(";")) continue;
    const section = /^\[\s*([^\s\]"]+)(?:\s+"((?:[^"\\]|\\.)*)")?\s*\]/.exec(line);
    if (section) {
      inOrigin = (section[1] ?? "").toLowerCase() === "remote" && section[2] === "origin";
      continue;
    }
    if (!inOrigin) continue;
    const entry = /^([A-Za-z][A-Za-z0-9-]*)\s*=\s*(.*)$/.exec(line);
    if (entry && (entry[1] ?? "").toLowerCase() === "url") {
      let value = (entry[2] ?? "").replace(/\s+[#;].*$/, "").trim();
      if (value.startsWith('"') && value.endsWith('"') && value.length >= 2) value = value.slice(1, -1);
      return clean(value) ?? null;
    }
  }
  return null;
}

/** `remote.origin.url` read from the repository's config file (no git needed). */
export function gitOriginFromConfig(root: string): string | null {
  const dir = gitConfigDir(root);
  if (!dir) return null;
  try {
    return originFromConfigText(readFileSync(join(dir, "config"), "utf8"));
  } catch {
    return null;
  }
}

/** `remote.origin.url` of the repository at `root`: `git`, else its config file. */
export async function gitOrigin(root: string): Promise<string | null> {
  return (await gitOriginViaGit(root)) ?? gitOriginFromConfig(root);
}

function realPath(dir: string): string {
  try {
    return realpathSync.native(dir);
  } catch {
    return resolve(dir);
  }
}

/** The project `dir` is in. */
export async function project(
  dir: string,
  origin: (root: string) => Promise<string | null> = gitOrigin,
): Promise<Project> {
  const real = realPath(dir);
  const root = gitRoot(real);
  if (root !== null) {
    const key = normalizeRemote(await origin(root));
    if (key) return { key, name: key.slice(key.lastIndexOf("/") + 1), root };
  }
  const base = root ?? real;
  return { key: localKey(base), name: basename(base) || base, root: base };
}

function idsOf(value: unknown): Ids | null {
  if (typeof value !== "object" || value === null) return null;
  const record = value as Record<string, unknown>;
  const session = clean(record.session_id);
  const workspace = clean(record.workspace_id);
  return session && workspace ? { session_id: session, workspace_id: workspace } : null;
}

const UUID = /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/;

/** The first UUID after `label` in `text` (the markdown of the setup templates around it). */
export function uuidAfter(text: string, label: string): string | null {
  let from = 0;
  for (;;) {
    const start = text.indexOf(label, from);
    if (start === -1) return null;
    const rest = text.slice(start + label.length).replace(/^[^A-Za-z0-9_]*/, "");
    const candidate = rest.slice(0, 36);
    if (UUID.test(candidate)) return candidate;
    from = start + label.length;
  }
}

/** The ids fixed in the repository at `root`: `.post-cortex.json`, else the first of `files` naming both ids. */
export function pinnedIds(root: string, files: readonly string[] = INSTRUCTION_FILES): Ids | null {
  const pinned = idsOf(readJsonObject(join(root, PINNED_FILE)));
  if (pinned) return pinned;
  for (const name of files) {
    let text: string;
    try {
      text = readFileSync(join(root, name), "utf8");
    } catch {
      continue;
    }
    const session = uuidAfter(text, "Session ID");
    const workspace = uuidAfter(text, "Workspace ID");
    if (session && workspace) return { session_id: session, workspace_id: workspace };
  }
  return null;
}

const noSlash = (url: string): string => url.replace(/\/+$/, "");

/** The cached ids of `key` for the backend at `url`. */
export function cachedIds(configDir: string, key: string, url: string): Ids | null {
  const entry = readJsonObject(join(configDir, CACHE_FILE))[key];
  if (typeof entry !== "object" || entry === null) return null;
  const cachedUrl = (entry as Record<string, unknown>).url;
  if (typeof cachedUrl !== "string" || noSlash(cachedUrl) !== noSlash(url)) return null;
  return idsOf(entry);
}

/** Remember the ids of `key` for the backend at `url` (best effort). */
export function cacheIds(configDir: string, key: string, url: string, ids: Ids): void {
  try {
    const path = join(configDir, CACHE_FILE);
    const cache = readJsonObject(path);
    cache[key] = { workspace_id: ids.workspace_id, session_id: ids.session_id, url: noSlash(url) };
    mkdirSync(configDir, { recursive: true });
    const tmp = `${path}.${process.pid}.tmp`;
    writeFileSync(tmp, `${JSON.stringify(cache, null, 2)}\n`);
    renameSync(tmp, path);
  } catch {
    // a cache that cannot be written only costs a request next time
  }
}

/**
 * `path` relative to the project `root` when it is inside it (absolute, or
 * relative to `dir`), else as given. The daemon's file view is keyed by
 * project-relative paths.
 */
export function projectRelative(path: string, root: string, dir: string = root): string {
  const absolute = isAbsolute(path) ? path : resolve(dir, path);
  const inside = (candidate: string): string | null => {
    const rel = relative(root, candidate);
    return rel && !rel.startsWith("..") && !isAbsolute(rel) ? rel.split(sep).join("/") : null;
  };
  // `dir` or `path` may go through a symlink the real root does not
  // (/tmp vs /private/tmp on macOS), and a new file may not exist yet.
  const candidates = [absolute, realPath(absolute), resolve(realPath(dir), relative(dir, absolute))];
  for (const candidate of candidates) {
    const rel = inside(candidate);
    if (rel !== null) return rel;
  }
  return path;
}
