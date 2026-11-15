// Text the plugins put in front of the agent. Identical to HookEnv in
// crates/post-cortex-daemon/src/bin/pcx/clients/common.rs (cards_text,
// file_context_text, files_context_text, patch_paths), so every client sees
// the same words.

import type { SearchHit } from "./client.js";
import type { Ids } from "./project.js";

type Json = Record<string, unknown>;

function field(value: unknown, key: string): unknown {
  return typeof value === "object" && value !== null ? (value as Json)[key] : undefined;
}

function str(value: unknown): string {
  return typeof value === "string" ? value : "";
}

/** serde_json's Display of a value, as Rust's `format!("{}", card["version"])` prints it. */
function display(value: unknown): string {
  return JSON.stringify(value ?? null) ?? "null";
}

/** The line that tells the agent which session and workspace to use (no instructions file carries them in global mode). */
export function idsLine(ids: Ids): string {
  return `[PCX] Session: ${ids.session_id} | Workspace: ${ids.workspace_id}`;
}

/**
 * The user card and the project card (`GET /api/cards`), with how to write
 * the project card when there is none yet. With `ids`, the ids line comes
 * first, as the global Claude Code hook prints it.
 */
export function cardsText(cards: unknown, workspaceId: string, ids?: Ids): string {
  const parts: string[] = [];
  if (ids) parts.push(idsLine(ids));
  for (const [key, title] of [
    ["user", "User card"],
    ["workspace", "Project card"],
  ] as const) {
    const card = field(cards, key);
    const text = str(field(card, "text")).trim();
    if (text) parts.push(`### ${title} (v${display(field(card, "version"))})\n${text}`);
  }
  if (!str(field(field(cards, "workspace"), "text")).trim()) {
    parts.push(
      "No project card yet. Once you know the stack, conventions and current goals, " +
        "write a short one (max 2 KB): manage_workspace action update_card, workspace_id " +
        `${workspaceId}, expected_version 0.`,
    );
  }
  return `## post-cortex cards\n\n${parts.join("\n\n")}`;
}

function recordLine(record: unknown): string {
  const when = [...str(field(record, "timestamp"))].slice(0, 10).join("");
  let text = str(field(record, "title"));
  const description = str(field(record, "description")).trim();
  if (description) text += `: ${[...description].slice(0, 200).join("")}`;
  const validity = field(record, "validity");
  if (typeof field(validity, "valid_to") === "string") {
    const reason = field(validity, "reason");
    text += ` (no longer holds: ${typeof reason === "string" ? reason : "ended"})`;
  }
  return `- ${when} ${text} [entry ${str(field(record, "id"))}]`;
}

/**
 * What the memory says about `path` (`GET /api/file`): past problems,
 * decisions and dependents; null when nothing is known.
 */
export function fileContextText(view: unknown, path: string): string | null {
  const list = (key: string): string[] => {
    const items = field(view, key);
    return Array.isArray(items) ? items.slice(0, 3).map(recordLine) : [];
  };
  const parts: string[] = [];
  const problems = list("problems");
  if (problems.length) parts.push(`Problems solved here before:\n${problems.join("\n")}`);
  const decisions = list("decisions");
  if (decisions.length) parts.push(`Decisions and requirements about it:\n${decisions.join("\n")}`);
  const dependents = field(view, "dependents");
  const names = Array.isArray(dependents)
    ? dependents
        .slice(0, 10)
        .map((d) => field(d, "entity"))
        .filter((e): e is string => typeof e === "string")
    : [];
  if (names.length) parts.push(`Depends on it: ${names.join(", ")}`);
  if (!parts.length) return null;
  return `post-cortex memory for ${path}:\n\n${parts.join("\n\n")}`;
}

/** Heading of the recall section; {@link isInjected} skips text that starts with it. */
export const RECALL_HEADING = "## Related memory (post-cortex)";

/**
 * Search hits for the user's prompt as a short section, one line per hit
 * (date, title, snippet up to 200 characters, entry id); null when none.
 */
export function recallText(hits: readonly SearchHit[]): string | null {
  if (!hits.length) return null;
  const lines = hits.map((hit) => {
    const when = [...hit.timestamp].slice(0, 10).join("");
    const snippet = hit.snippet.trim();
    const text = snippet ? `${hit.title}: ${[...snippet].slice(0, 200).join("")}` : hit.title;
    return `- ${when} ${text} [entry ${hit.id}]`;
  });
  return `${RECALL_HEADING}\n\n${lines.join("\n")}`;
}

/** {@link fileContextText} texts joined by a blank line; null when none. */
export function joinTexts(texts: ReadonlyArray<string | null>): string | null {
  const found = texts.filter((t): t is string => t !== null);
  return found.length ? found.join("\n\n") : null;
}

/**
 * The files an `apply_patch` patch adds or updates, in order and once each:
 * the paths of `*** Update File:`, `*** Add File:` and `*** Move to:` lines
 * (deleted files are skipped).
 */
export function patchPaths(patch: string): string[] {
  const paths: string[] = [];
  for (const raw of patch.split("\n")) {
    const line = raw.trimStart();
    for (const prefix of ["*** Update File:", "*** Add File:", "*** Move to:"]) {
      if (!line.startsWith(prefix)) continue;
      const path = line.slice(prefix.length).trim();
      if (path && !paths.includes(path)) paths.push(path);
      break;
    }
  }
  return paths;
}
