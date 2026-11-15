// The module Kilo loads. Its default export is the server plugin function
// with the module descriptor's fields (`id`, `server`) on it, so it loads
// in both plugin loaders:
//
// - current loaders read a default *object* `{ id, server }` (v1 modules) and
//   otherwise call every export as a legacy plugin function: a function is
//   not an object to them, so this one is called as a legacy plugin;
// - older loaders (e.g. Kilo 7.1) only call every export as a function and
//   reject a plain `{ id, server }` object.
//
// Nothing else is exported, because every export counts as a plugin.

import type { Plugin, PluginModule } from "@kilocode/plugin";

import { server } from "./server.js";

const plugin: Plugin & PluginModule = Object.assign<Plugin, PluginModule>(
  (input, options) => server(input, options),
  { id: "post-cortex", server },
);

export default plugin;
