import assert from "node:assert/strict";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";

import { CLOUD_URL, SELF_URL, configDir, hostSettings, resolveBackend } from "../src/config.js";
import { tempEnv } from "../src/testing.js";

function setup(config?: unknown, credentials?: unknown) {
  const t = tempEnv();
  if (config !== undefined) t.writeConfig(config);
  if (credentials !== undefined) {
    writeFileSync(join(t.configDir, "credentials.json"), JSON.stringify(credentials));
  }
  return t;
}

test("config dir is PC_CONFIG_DIR, else ~/.post-cortex; blank counts as unset", () => {
  assert.equal(configDir({ PC_CONFIG_DIR: "/x/y" }), "/x/y");
  assert.match(configDir({ PC_CONFIG_DIR: "  " }), /\.post-cortex$/);
  assert.match(configDir({}), /\.post-cortex$/);
});

test("missing config.json means cloud, with no token", () => {
  const t = setup();
  try {
    assert.deepEqual(resolveBackend({ env: t.env }), { profile: "cloud", url: CLOUD_URL });
  } finally {
    t.cleanup();
  }
});

test("profile: option, else PC_PROFILE, else active, else cloud", () => {
  const t = setup(
    { active: "work", profiles: { work: { url: "https://work.example/" }, self: { url: "http://h:1" } } },
    { work: { token: " w " }, self: { token: "s" }, cloud: { token: "c" } },
  );
  try {
    const env = t.env;
    assert.deepEqual(resolveBackend({ env }), { profile: "work", url: "https://work.example", token: "w" });
    assert.deepEqual(resolveBackend({ env: { ...env, PC_PROFILE: "self" } }), {
      profile: "self",
      url: "http://h:1",
      token: "s",
    });
    assert.deepEqual(resolveBackend({ env: { ...env, PC_PROFILE: "self" }, profile: "cloud" }), {
      profile: "cloud",
      url: CLOUD_URL,
      token: "c",
    });
  } finally {
    t.cleanup();
  }
});

test("built-in self URL, PC_URL wins over the profile, PC_AUTH_TOKEN over credentials", () => {
  const t = setup({ active: "self" }, { self: { token: "stored" } });
  try {
    assert.deepEqual(resolveBackend({ env: t.env }), { profile: "self", url: SELF_URL, token: "stored" });
    assert.deepEqual(
      resolveBackend({ env: { ...t.env, PC_URL: "http://other:9/", PC_AUTH_TOKEN: "env" } }),
      { profile: "self", url: "http://other:9", token: "env" },
    );
    assert.deepEqual(resolveBackend({ env: { ...t.env, PC_AUTH_TOKEN: "   " } }), {
      profile: "self",
      url: SELF_URL,
      token: "stored",
    });
  } finally {
    t.cleanup();
  }
});

test("unknown profile without PC_URL resolves to nothing; with PC_URL it works", () => {
  const t = setup({ active: "nope" });
  try {
    assert.equal(resolveBackend({ env: t.env }), undefined);
    assert.deepEqual(resolveBackend({ env: { ...t.env, PC_URL: "http://x" } }), { profile: "nope", url: "http://x" });
  } finally {
    t.cleanup();
  }
});

test("garbage files count as missing", () => {
  const t = setup();
  try {
    writeFileSync(join(t.configDir, "config.json"), "{not json");
    writeFileSync(join(t.configDir, "credentials.json"), "[1,2]");
    assert.deepEqual(resolveBackend({ env: t.env }), { profile: "cloud", url: CLOUD_URL });
  } finally {
    t.cleanup();
  }
});

test("hosts.<author>: enabled default true, capture default false, workspace", () => {
  const t = setup({ hosts: { pi: { enabled: false, capture: true, workspace: " team " }, kilo: "x" } });
  try {
    assert.deepEqual(hostSettings("pi", { dir: t.configDir }), { enabled: false, capture: true, workspace: "team" });
    assert.deepEqual(hostSettings("kilo", { dir: t.configDir }), { enabled: true, capture: false });
    assert.deepEqual(hostSettings("opencode", { dir: t.configDir }), { enabled: true, capture: false });
  } finally {
    t.cleanup();
  }
});
