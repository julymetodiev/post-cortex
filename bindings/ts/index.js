// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

// A pcx module in TypeScript (docs/modules/dynamic.md, the bindings' API),
// for Deno and Node. The Rust core (the addon) serves the module to its
// host; this layer declares the module (its proto, its tools, what it uses)
// and runs its handlers on the runtime's event loop:
//
//   import { Module, Effect } from "@post-cortex/module";
//   import { file_greet_v1_greet, Greeter } from "./gen/greet/v1/greet_pb.ts";
//
//   const module = new Module({ id: "greet-ts", version: "0.1.0",
//     proto: file_greet_v1_greet, description: "Says hello." });
//   module.tool(Greeter.method.greet, { name: "greet", effect: Effect.WRITE },
//     async (ctx, request) => ({ text: `Hello, ${request.name}` }));
//   await module.run();
//
// Only message bytes cross to the core; the module's messages are its own
// generated code (@bufbuild/protobuf), never the module protocol's.

import { createRequire } from "node:module";
import process from "node:process";
import { Buffer } from "node:buffer";
import { create, fromBinary, toBinary } from "@bufbuild/protobuf";
import { FileDescriptorSetSchema } from "@bufbuild/protobuf/wkt";

const require = createRequire(import.meta.url);
const { Core } = require(`./pcx_module.${process.platform}-${process.arch}.node`);

/** What a call of an RPC does. */
export const Effect = Object.freeze({
  /** Changes nothing, in pcx or outside. */
  READ: "read",
  /** Changes state inside pcx only. */
  WRITE: "write",
  /** Acts outside pcx. */
  EXTERNAL: "external",
});

/** Who may call a tool. */
export const Visibility = Object.freeze({
  /** An agent and the module's views. */
  BOTH: "both",
  /** An agent only. */
  MODEL: "model",
  /** The module's views only. */
  APP: "app",
});

/**
 * A failed call: its `kind` (`not_found`, `denied`, `invalid_argument`, ...)
 * is what a caller acts on, its message is for a person.
 */
export class ModuleError extends Error {
  constructor(kind, message) {
    super(message);
    this.name = "ModuleError";
    this.kind = kind;
  }
}

/** The path of `method`, `/<package>.<Service>/<Method>`. */
function pathOf(method) {
  return `/${method.parent.typeName}/${method.name}`;
}

/** The file `proto` and every file it imports, imported first. */
function descriptors(proto) {
  const files = [];
  const seen = new Set();
  const add = (file) => {
    if (seen.has(file.name)) return;
    seen.add(file.name);
    for (const dependency of file.dependencies) add(dependency);
    files.push(file.proto);
  };
  add(proto);
  return Buffer.from(toBinary(FileDescriptorSetSchema, create(FileDescriptorSetSchema, { file: files })));
}

/** The module's handle on its host, within one call. */
class Host {
  #core;
  #call;

  constructor(core, call) {
    this.#core = core;
    this.#call = call;
  }

  /**
   * Call `method` (a method of a module this one uses, from its generated
   * code) with `request`, in this call's scope: its response.
   */
  async call(method, request) {
    const answered = await this.#core.hostCall(
      this.#call,
      pathOf(method),
      Buffer.from(toBinary(method.input, request)),
    );
    if (answered.failure) throw new ModuleError(answered.failure.kind, answered.failure.message);
    return fromBinary(method.output, answered.body);
  }
}

/** One call: who it is for, its deadline, the host, the settings. */
class Context {
  #module;
  #call;

  constructor(module, call) {
    this.#module = module;
    this.#call = call;
    /** The module's handle on its host. */
    this.host = new Host(module.core, call.id);
  }

  /** Who calls (a person, an agent, a module); null: unknown. */
  get author() {
    return this.#call.author ?? null;
  }

  /** The workspace it is for; null: the caller's default. */
  get workspace() {
    return this.#call.workspace ?? null;
  }

  /** The module that calls; null: a person or an agent. */
  get caller() {
    return this.#call.caller ?? null;
  }

  /** Milliseconds left before its deadline, as handed over. */
  get remaining() {
    return this.#call.remaining;
  }

  /** Whether its caller gave up (its deadline passed). */
  get cancelled() {
    return !this.#module.core.waits(this.#call.id);
  }

  /** The module's settings, a message of its `config`; null without. */
  get settings() {
    return this.#module.settings();
  }

  /** Emit the event `name` the module declares (its `emits`), of `schema`: its id. */
  async publish(name, schema, event) {
    const published = await this.#module.core.publish(
      this.#call.id,
      name,
      schema.typeName,
      Buffer.from(toBinary(schema, event)),
    );
    if (published.failure) throw new ModuleError(published.failure.kind, published.failure.message);
    return published.id;
  }

  /** The module's log, which its host keeps (stderr). */
  log(...args) {
    console.error(`${this.#module.id}:`, ...args);
  }
}

/** A module: what it declares, its tools' handlers, and its run. */
export class Module {
  #tools = new Map();
  #settings;

  /**
   * `proto` is the generated file of its proto (`file_greet_v1_greet`);
   * `emits` its events as { name, schema, description }; `config` the
   * schema of its settings; `concurrency` the calls it runs at once.
   */
  constructor({
    id,
    version,
    proto,
    description = "",
    uses = [],
    emits = [],
    follows = [],
    config = null,
    concurrency = 8,
  }) {
    if (concurrency < 1) throw new RangeError("a module runs at least one call at once");
    Object.assign(this, { id, version, proto, description, uses, emits, follows, config, concurrency });
    this.core = null;
  }

  /**
   * Declare `method` (from its generated code) or the RPC `path` of its
   * proto, handled by `async (ctx, request) => response`; `name` its
   * tool's (empty: an RPC only its views or other modules call).
   */
  tool(method, { name = "", effect, visibility = Visibility.BOTH }, handler) {
    const path = typeof method === "string" ? method : pathOf(method);
    if (this.#tools.has(path)) throw new Error(`${path} has a handler already`);
    const declared = this.proto.services
      .flatMap((service) => service.methods)
      .find((m) => pathOf(m) === path);
    if (!declared) throw new Error(`${this.proto.name} declares no method ${path}`);
    if (declared.methodKind !== "unary") throw new Error(`${path} is not unary`);
    this.#tools.set(path, { path, name, effect, visibility, method: declared, handler });
    return this;
  }

  /** The settings, read once from the host's start. */
  settings() {
    if (this.config === null) return null;
    if (this.#settings === undefined) {
      const { message, body } = this.core.config();
      if (message && message !== this.config.typeName) {
        throw new ModuleError("internal", `the host gave settings of ${message}, not ${this.config.typeName}`);
      }
      this.#settings = fromBinary(this.config, body);
    }
    return this.#settings;
  }

  async #answer(call) {
    const tool = this.#tools.get(call.method);
    if (!tool) {
      this.core.fail(call.id, "not_found", `${this.id} serves no method ${call.method}`);
      return;
    }
    let request;
    try {
      request = fromBinary(tool.method.input, call.body);
    } catch (e) {
      this.core.fail(call.id, "invalid_argument", `the request is not a ${tool.method.input.typeName}: ${e}`);
      return;
    }
    try {
      const response = await tool.handler(new Context(this, call), request);
      // A message, or what makes one.
      const answer = create(tool.method.output, response);
      this.core.complete(call.id, Buffer.from(toBinary(tool.method.output, answer)));
    } catch (e) {
      if (e instanceof ModuleError) {
        this.core.fail(call.id, e.kind, e.message);
      } else {
        // A defect of the handler: the call fails, the module goes on.
        console.error(`${this.id}: ${call.method} failed`, e);
        this.core.fail(call.id, "internal", `${call.method}: ${e}`);
      }
    }
  }

  async #work() {
    for (let call = await this.core.nextCall(); call; call = await this.core.nextCall()) {
      await this.#answer(call);
    }
  }

  /** Serve the module on the socket its host gives, until the host stops it. */
  async run() {
    this.core = new Core({
      id: this.id,
      version: this.version,
      description: this.description,
      // The services it serves: those of its RPCs.
      services: [...new Set([...this.#tools.values()].map((t) => t.method.parent.typeName))],
      descriptors: descriptors(this.proto),
      rpcs: [...this.#tools.values()].map((t) => ({
        method: t.path,
        tool: t.name,
        effect: t.effect,
        visibility: t.visibility,
      })),
      uses: this.uses,
      emits: this.emits.map((e) => ({ name: e.name, message: e.schema.typeName, description: e.description ?? "" })),
      follows: this.follows,
      configType: this.config ? this.config.typeName : "",
    });
    const workers = Array.from({ length: this.concurrency }, () => this.#work());
    await this.core.serve();
    await Promise.all(workers);
  }
}
