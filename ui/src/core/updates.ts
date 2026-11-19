// Whether a newer version of Outpost has been released. The core asks
// GitHub at most once a day (and not at all if the setting is off); this
// keeps what it found for the status bar and the settings page.

import { createSignal } from "solid-js";
import { core } from "./index";
import type { NewerVersion } from "./api";

const [update, setUpdate] = createSignal<NewerVersion>();
const [updateChecked, setUpdateChecked] = createSignal(false);
export { update, updateChecked };

let waiting: { done: () => void; failed: (error: Error) => void }[] = [];

/** Called for every core event; picks out the answer to a check. */
export function updateEvent(event: { type: string; newer?: NewerVersion | null; error?: string | null }) {
  if (event.type !== "updateChecked") return;
  if (!event.error) {
    setUpdate(event.newer ?? undefined);
    setUpdateChecked(true);
  }
  const all = waiting;
  waiting = [];
  for (const w of all) {
    if (event.error) w.failed(new Error(event.error));
    else w.done();
  }
}

/** Resolves once the answer is in: at once if it is recent enough. */
export async function checkForUpdate(force = false): Promise<void> {
  const r = await core.send({ type: "checkForUpdate", force });
  setUpdate(r.newer ?? undefined);
  setUpdateChecked(r.checked);
  if (r.checking) await new Promise<void>((done, failed) => waiting.push({ done, failed }));
}
