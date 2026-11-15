// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

// The greet example in TypeScript, on the bindings (docs/modules/dynamic.md).
// The host runs it as a process module: `deno run --allow-ffi --allow-read
// --allow-env module.ts` (or `node module.ts`).

import { Effect, Module } from "@post-cortex/module";
import { file_greet_v1_greet, Greeter } from "./gen/greet/v1/greet_pb.ts";

const module = new Module({
  id: "greet-ts",
  version: "0.1.0",
  proto: file_greet_v1_greet,
  description: "Says hello: the greet example, in TypeScript.",
});

module.tool(Greeter.method.greet, { name: "greet", effect: Effect.READ }, async (_ctx, request) => ({
  text: `Hello, ${request.name}`,
}));

await module.run();
