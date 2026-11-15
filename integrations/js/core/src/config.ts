// Client config: which post-cortex backend a plugin talks to.
//
// The files and rules are the ones pcx implements in
// crates/post-cortex-daemon/src/bin/pcx/profiles.rs:
//
// - The directory is $PC_CONFIG_DIR when set and not blank, else
//   ~/.post-cortex.
// - config.json: {"active", "profiles": {"<name>": {"url"}}, "hosts":
//   {"<author>": {"enabled", "capture", "workspace"}}}; every key optional,
//   a missing file means the built-in defaults (cloud active).
// - credentials.json: {"<profile>": {"token"}}.
// - Profile: the one asked for (plugin option), else $PC_PROFILE, else
//   `active`, else "cloud".
// - URL: $PC_URL, else the profile's url, else the built-in default
//   ("cloud" and "self"); an unknown profile without $PC_URL resolves to
//   nothing.
// - Token: $PC_AUTH_TOKEN, else credentials.json for the profile. (pcx
//   also falls back to daemon.toml's auth_token for "self"; plugins do not
//   read daemon.toml: set PC_AUTH_TOKEN or run `pcx login --profile self`.)
//
// Blank values count as unset everywhere. Nothing here throws.

import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

/** The hosted post-cortex service. */
export const CLOUD_URL = "https://api.pcx.is";
/** The self-hosted daemon's default address (`pcx` reads daemon.toml). */
export const SELF_URL = "http://127.0.0.1:3737";
export const CLOUD = "cloud";
export const SELF_HOSTED = "self";

/** The environment the resolution reads. */
export type Env = Readonly<Record<string, string | undefined>>;

type JsonObject = Record<string, unknown>;

/** `value` trimmed, or undefined when it is not a non-blank string. */
export function clean(value: unknown): string | undefined {
  if (typeof value !== "string") return undefined;
  const trimmed = value.trim();
  return trimmed ? trimmed : undefined;
}

function isObject(value: unknown): value is JsonObject {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** The JSON object in `path`, or {} when it is missing, unreadable or not an object. */
export function readJsonObject(path: string): JsonObject {
  try {
    const value: unknown = JSON.parse(readFileSync(path, "utf8"));
    return isObject(value) ? value : {};
  } catch {
    return {};
  }
}

/** `$PC_CONFIG_DIR`, else `~/.post-cortex`. */
export function configDir(env: Env = process.env): string {
  return clean(env.PC_CONFIG_DIR) ?? join(homedir(), ".post-cortex");
}

const trimSlashes = (url: string): string => url.replace(/\/+$/, "");

/** A resolved backend: where it is and the bearer token to send, if any. */
export interface Backend {
  profile: string;
  /** Base URL without a trailing slash. */
  url: string;
  token?: string;
}

export interface ResolveOptions {
  /** The profile asked for (wins over $PC_PROFILE and `active`). */
  profile?: string;
  env?: Env;
  /** The config directory (default: {@link configDir}). */
  dir?: string;
}

/** The URL of profile `name` in `config`, else the built-in default. */
export function profileUrl(config: JsonObject, name: string): string | undefined {
  const profiles = isObject(config.profiles) ? config.profiles : {};
  const entry = profiles[name];
  const url = isObject(entry) ? clean(entry.url) : undefined;
  if (url) return trimSlashes(url);
  if (name === CLOUD) return CLOUD_URL;
  if (name === SELF_HOSTED) return SELF_URL;
  return undefined;
}

/** The token `pcx login` stored for `profile`. */
export function storedToken(dir: string, profile: string): string | undefined {
  const entry = readJsonObject(join(dir, "credentials.json"))[profile];
  return isObject(entry) ? clean(entry.token) : undefined;
}

/** The backend to talk to, or undefined for an unknown profile without $PC_URL. */
export function resolveBackend(options: ResolveOptions = {}): Backend | undefined {
  const env = options.env ?? process.env;
  const dir = options.dir ?? configDir(env);
  const config = readJsonObject(join(dir, "config.json"));
  const profile =
    clean(options.profile) ?? clean(env.PC_PROFILE) ?? clean(config.active) ?? CLOUD;
  const explicit = clean(env.PC_URL);
  const url = explicit ? trimSlashes(explicit) : profileUrl(config, profile);
  if (!url) return undefined;
  const token = clean(env.PC_AUTH_TOKEN) ?? storedToken(dir, profile);
  return token ? { profile, url, token } : { profile, url };
}

/** `hosts.<author>` of config.json, with the defaults applied. */
export interface HostSettings {
  /** Default true. */
  enabled: boolean;
  /** Capture turns into the session's message stream. Default false. */
  capture: boolean;
  /** Workspace name a newly resolved project goes into. */
  workspace?: string;
}

export function hostSettings(
  author: string,
  options: { env?: Env; dir?: string } = {},
): HostSettings {
  const dir = options.dir ?? configDir(options.env ?? process.env);
  const hosts = readJsonObject(join(dir, "config.json")).hosts;
  const host = isObject(hosts) && isObject(hosts[author]) ? (hosts[author] as JsonObject) : {};
  const workspace = clean(host.workspace);
  return {
    enabled: host.enabled !== false,
    capture: host.capture === true,
    ...(workspace ? { workspace } : {}),
  };
}
