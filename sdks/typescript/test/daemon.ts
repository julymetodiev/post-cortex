// Copyright (c) 2026 Chronicle Labs Ltd.
// MIT License

// A throwaway post-cortex daemon for the tests: `PCX_BIN` or the workspace's
// target/debug/pcx, its own HOME and RocksDB directory, the shared Hugging Face cache.

import { spawn, type ChildProcess } from "node:child_process";
import { existsSync, mkdtempSync, openSync, readFileSync, rmSync } from "node:fs";
import { createServer } from "node:net";
import { homedir, tmpdir } from "node:os";
import { join, resolve } from "node:path";

import { Client, NotFound, PostCortexError } from "../src/index.js";

const REPO = resolve(import.meta.dirname, "..", "..", "..");

export function binary(): string | undefined {
  const path = process.env.PCX_BIN ?? join(REPO, "target", "debug", "pcx");
  return existsSync(path) ? path : undefined;
}

function freePort(): Promise<number> {
  return new Promise((ok, fail) => {
    const server = createServer();
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      server.close(() => (typeof address === "object" && address ? ok(address.port) : fail()));
    });
  });
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

export class Daemon {
  target = "";
  private proc?: ChildProcess;
  private dir = "";

  static async start(token?: string): Promise<Daemon> {
    const d = new Daemon();
    await d.run(token);
    return d;
  }

  private async run(token?: string): Promise<void> {
    const bin = binary();
    if (!bin) throw new Error("no pcx binary");
    this.dir = mkdtempSync(join(tmpdir(), "pcx-sdk-ts-"));
    const [port, grpcPort] = [await freePort(), await freePort()];
    const env: NodeJS.ProcessEnv = {
      ...process.env,
      HOME: this.dir,
      HF_HOME: process.env.HF_HOME ?? join(homedir(), ".cache", "huggingface"),
      PC_DATA_DIR: join(this.dir, "data"),
      PC_STORAGE_BACKEND: "rocksdb",
      PC_HOST: "127.0.0.1",
      PC_PORT: String(port),
      PC_GRPC_PORT: String(grpcPort),
      RUST_LOG: "warn",
    };
    delete env.PC_AUTH_TOKEN;
    if (token) env.PC_AUTH_TOKEN = token;
    const log = openSync(join(this.dir, "daemon.log"), "w");
    this.proc = spawn(bin, ["start"], { env, stdio: ["ignore", log, log] });
    this.target = `127.0.0.1:${grpcPort}`;

    const deadline = Date.now() + 120_000;
    for (;;) {
      if (this.proc.exitCode !== null) {
        throw new Error(`pcx exited: ${readFileSync(join(this.dir, "daemon.log"), "utf8").slice(-2000)}`);
      }
      const c = new Client({ target: this.target, token: token ?? "", timeoutMs: 2000 });
      try {
        if ((await c.health()).healthy) break;
      } catch (e) {
        if (!(e instanceof PostCortexError)) throw e;
      } finally {
        c.close();
      }
      if (Date.now() > deadline) throw new Error("pcx did not come up in 120 s");
      await sleep(500);
    }
    // Load the embedding model once, so no test runs into its deadline.
    const warm = new Client({ target: this.target, token: token ?? "", timeoutMs: 600_000 });
    try {
      await warm.search("warm up", { limit: 1 });
    } catch {
      // A daemon without embeddings: search tests skip themselves.
    } finally {
      warm.close();
    }
  }

  async stop(): Promise<void> {
    const proc = this.proc;
    if (proc && proc.exitCode === null) {
      const exited = new Promise((r) => proc.once("exit", r));
      proc.kill("SIGTERM");
      await Promise.race([exited, sleep(20_000)]);
      if (proc.exitCode === null) proc.kill("SIGKILL");
    }
    rmSync(this.dir, { recursive: true, force: true });
  }
}

/** Retry `check` until it returns a truthy value (background work on the daemon). */
export async function eventually<T>(check: () => Promise<T | undefined | false>, timeoutMs = 30_000): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    let value: T | undefined | false;
    try {
      value = await check();
    } catch (e) {
      if (!(e instanceof NotFound)) throw e;
      value = undefined;
    }
    if (value) return value;
    if (Date.now() > deadline) throw new Error("condition not met in time");
    await sleep(200);
  }
}
