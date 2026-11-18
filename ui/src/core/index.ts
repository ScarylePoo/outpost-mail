import type { Transport } from "./api";
import { stubTransport } from "./stub";
import { tauriTransport } from "./tauri";

export * from "./api";

/** The core, through whichever shell is hosting the interface. */
export const core: Transport = "__TAURI_INTERNALS__" in window ? tauriTransport : stubTransport;
