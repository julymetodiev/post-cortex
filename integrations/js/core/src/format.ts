// Text the plugins put in front of the agent beside what the memory module
// words itself (the cards and a file's memory, `PcxClient.cards` and
// `PcxClient.fileContext`, the same for every client): the ids line, as the
// Claude Code plugin prints it before the cards, the recall section and the
// `apply_patch` path parser.

import type { SearchHit } from "./client.js";
import type { Ids } from "./project.js";

/** The line that tells the agent which session and workspace to use (no instructions file carries them in global mode). */
export function idsLine(ids: Ids): string {
  return `[PCX] Session: ${ids.session_id} | Workspace: ${ids.workspace_id}`;
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

/** Texts joined by a blank line (the memory of several files); null when none. */
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
