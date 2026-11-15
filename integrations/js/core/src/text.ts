// Small helpers for turning client messages into captured turns.

import { createHash } from "node:crypto";

/** The text of a message `content`: a string, or the `text` of its `{type: "text"}` blocks joined by newlines. */
export function textOf(content: unknown): string {
  if (typeof content === "string") return content.trim();
  if (!Array.isArray(content)) return "";
  return content
    .flatMap((block: unknown) => {
      if (typeof block !== "object" || block === null) return [];
      const { type, text } = block as Record<string, unknown>;
      return type === "text" && typeof text === "string" ? [text] : [];
    })
    .join("\n")
    .trim();
}

/** A stable id from `parts` (sha256, 32 hex), for clients whose messages carry none. */
export function stableId(...parts: ReadonlyArray<string | number | undefined | null>): string {
  return createHash("sha256")
    .update(parts.map((p) => String(p ?? "")).join("\u0000"), "utf8")
    .digest("hex")
    .slice(0, 32);
}

/** Prompts a harness injects rather than the user typing them (reminders, notifications, command envelopes). */
const INJECTED = [
  /^<[a-z-]+-reminder>/i,
  /^<command-(?:name|message|args)>/i,
  /^\[SYSTEM NOTIFICATION/i,
  /^<task-notification>/i,
  /^## post-cortex cards/,
  /^post-cortex memory for /,
  /^## Related memory \(post-cortex\)/,
  /^\[PCX\] /,
];

/** Whether `text` was injected by a harness or by post-cortex itself, so capturing it would only echo memory back. */
export function isInjected(text: string): boolean {
  const trimmed = text.trim();
  return INJECTED.some((pattern) => pattern.test(trimmed));
}
