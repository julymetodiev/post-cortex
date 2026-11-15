// Pi loads the default export: `export default function (pi: ExtensionAPI)`.
// Options come from `hosts.pi` in ~/.post-cortex/config.json and the PC_*
// environment variables (Pi passes extensions no options).

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import { postCortex } from "./extension.js";

export default function postCortexExtension(pi: ExtensionAPI): void {
  postCortex()(pi);
}
