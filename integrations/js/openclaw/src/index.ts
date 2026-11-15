import { definePluginEntry } from "openclaw/plugin-sdk/plugin-entry";

import { PLUGIN_ID, register } from "./plugin.js";

export default definePluginEntry({
  id: PLUGIN_ID,
  name: "post-cortex",
  description: "Long-term memory from post-cortex: user and project cards in every prompt, related records for the request, optional turn capture",
  register(api) {
    register(api);
  },
});
