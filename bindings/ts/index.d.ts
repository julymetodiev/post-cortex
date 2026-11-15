// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

// The types of @post-cortex/module (index.js): a pcx module in TypeScript.

import type { DescFile, DescMessage, DescMethod, Message, MessageInitShape, MessageShape } from "@bufbuild/protobuf";

/** What a call of an RPC does. */
export declare const Effect: {
  /** Changes nothing, in pcx or outside. */
  readonly READ: "read";
  /** Changes state inside pcx only. */
  readonly WRITE: "write";
  /** Acts outside pcx. */
  readonly EXTERNAL: "external";
};
export type Effect = (typeof Effect)[keyof typeof Effect];

/** Who may call a tool. */
export declare const Visibility: {
  /** An agent and the module's views. */
  readonly BOTH: "both";
  /** An agent only. */
  readonly MODEL: "model";
  /** The module's views only. */
  readonly APP: "app";
};
export type Visibility = (typeof Visibility)[keyof typeof Visibility];

/** A failed call: its `kind` (`not_found`, `denied`, ...) is what a caller acts on. */
export declare class ModuleError extends Error {
  constructor(kind: string, message: string);
  readonly kind: string;
}

/** The module's handle on its host, within one call. */
export interface Host {
  /** Call `method` of a module this one uses with `request`: its response. */
  call<M extends DescMethod>(method: M, request: MessageShape<M["input"]>): Promise<MessageShape<M["output"]>>;
}

/** One call: who it is for, its deadline, the host, the settings. */
export interface Context<Settings = unknown> {
  /** The module's handle on its host. */
  readonly host: Host;
  /** Who calls (a person, an agent, a module); null: unknown. */
  readonly author: string | null;
  /** The workspace it is for; null: the caller's default. */
  readonly workspace: string | null;
  /** The module that calls; null: a person or an agent. */
  readonly caller: string | null;
  /** Milliseconds left before its deadline, as handed over. */
  readonly remaining: number;
  /** Whether its caller gave up (its deadline passed). */
  readonly cancelled: boolean;
  /** The module's settings, a message of its `config`; null without. */
  readonly settings: Settings | null;
  /** Emit the event `name` the module declares, of `schema`: its id. */
  publish<S extends DescMessage>(name: string, schema: S, event: MessageShape<S>): Promise<string>;
  /** The module's log, which its host keeps (stderr). */
  log(...args: unknown[]): void;
}

/** An event the module emits. */
export interface Emitted {
  readonly name: string;
  readonly schema: DescMessage;
  readonly description?: string;
}

/** What a module declares. */
export interface Declaring {
  readonly id: string;
  readonly version: string;
  /** The generated file of its proto (`file_greet_v1_greet`). */
  readonly proto: DescFile;
  readonly description?: string;
  /** The modules it calls. */
  readonly uses?: string[];
  readonly emits?: Emitted[];
  readonly follows?: string[];
  /** The schema of its settings. */
  readonly config?: DescMessage | null;
  /** The calls it runs at once (default 8). */
  readonly concurrency?: number;
}

/** How a tool is declared. */
export interface ToolOptions {
  /** Its tool's name (empty: an RPC only its views or other modules call). */
  readonly name?: string;
  readonly effect: Effect;
  readonly visibility?: Visibility;
}

/** A module: what it declares, its tools' handlers, and its run. */
export declare class Module {
  constructor(declaring: Declaring);
  readonly id: string;
  /** Declare `method` (from its generated code), handled by `handler`. */
  tool<M extends DescMethod>(
    method: M,
    options: ToolOptions,
    handler: (ctx: Context, request: MessageShape<M["input"]>) => Promise<MessageInitShape<M["output"]>>,
  ): this;
  /** Declare the RPC `path` of its proto, handled by `handler`, whose request type it annotates. */
  tool<Request extends Message, Response extends Message>(
    path: string,
    options: ToolOptions,
    handler: (ctx: Context, request: Request) => Promise<Response>,
  ): this;
  /** Serve the module on the socket its host gives, until the host stops it. */
  run(): Promise<void>;
}
