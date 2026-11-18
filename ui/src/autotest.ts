// Unattended shell test. When the Tauri shell was started with
// OUTPOST_SHELL_TEST set, run the scroll test as soon as the list has rows and
// hand the report back to the shell, which writes it to a file and exits.
// See docs/testing.md. This is test tooling, not part of the core interface.

import { invoke } from "@tauri-apps/api/core";
import { runScrollTest } from "./bench";
import type { ListHandle } from "./list/MessageList";

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

export async function autoShellTest(
  list: () => ListHandle | undefined,
  onProgress: (text: string) => void,
): Promise<void> {
  if (!("__TAURI_INTERNALS__" in window)) return;
  if (!(await invoke<boolean>("shell_test_requested"))) return;
  let report: string;
  try {
    const deadline = Date.now() + 15_000;
    while (!list()?.rowCount()) {
      if (Date.now() > deadline) throw new Error("the list never loaded");
      await sleep(50);
    }
    await sleep(1000); // let fonts and first paint settle
    report = await runScrollTest(list()!, onProgress);
  } catch (e) {
    report = `Shell test failed to run: ${e instanceof Error ? e.message : String(e)}`;
  }
  await invoke("shell_test_report", { report });
}
