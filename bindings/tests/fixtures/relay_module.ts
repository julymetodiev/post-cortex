// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

// The bindings' test module in TypeScript (tests/bindings.rs): Relay, which
// calls Helper through its host.

import { Effect, Module, ModuleError } from "@post-cortex/module";
import { file_relay_v1_relay, Helper, Relay } from "./gen/relay/v1/relay_pb.ts";

const module = new Module({
  id: "relay",
  version: "0.1.0",
  proto: file_relay_v1_relay,
  description: "Relays to the helper: the bindings' test module.",
  uses: ["helper"],
});

module.tool(Relay.method.upper, { name: "relay_upper", effect: Effect.READ }, async (ctx, request) => {
  if (!request.text) throw new ModuleError("invalid_argument", "there is no text to relay");
  const answer = await ctx.host.call(Helper.method.upper, request);
  return { text: `${answer.text} for ${ctx.author}` };
});

module.tool(Relay.method.wait, { effect: Effect.READ }, async (ctx, request) => {
  await new Promise((done) => setTimeout(done, request.waitMs));
  return { text: ctx.cancelled ? "cancelled" : "waited" };
});

const held: Uint8Array[] = [];

module.tool(Relay.method.probe, { effect: Effect.READ }, async (_ctx, request) => {
  const Deno = (globalThis as any).Deno;
  const [action, ...rest] = request.text.split(":");
  const arg = rest.join(":");
  try {
    if (action === "spawn") {
      await new Deno.Command("/bin/echo").output();
    } else if (action === "connect") {
      const connected = Deno.connect({ hostname: "1.1.1.1", port: 53 });
      const late = new Promise<string>((done) => setTimeout(() => done("timed out"), 2000));
      const first = await Promise.race([connected, late]);
      if (first === "timed out") return { text: "timed out" };
      (first as { close(): void }).close();
    } else if (action === "read") {
      Deno.readFileSync(arg);
    } else if (action === "write") {
      Deno.writeTextFileSync(arg, "x");
    } else if (action === "alloc") {
      held.push(new Uint8Array(Number(arg) * 1024 * 1024).fill(1));
      await new Promise((done) => setTimeout(done, 5000));
    } else if (action === "files") {
      const opened = [];
      try {
        while (true) opened.push(Deno.openSync("/dev/null"));
      } catch {
        const count = opened.length;
        for (const f of opened) f.close();
        return { text: String(count) };
      }
    } else if (action === "pid") {
      return { text: String(Deno.pid) };
    }
  } catch (e) {
    return { text: `refused: ${e}` };
  }
  return { text: "allowed" };
});

await module.run();
