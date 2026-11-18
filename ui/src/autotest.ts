// Unattended shell test. When the Tauri shell was started with
// OUTPOST_SHELL_TEST set, run the scroll test as soon as the list has rows and
// hand the report back to the shell, which writes it to a file and exits.
// See docs/testing.md. This is test tooling, not part of the core interface.

import { invoke } from "@tauri-apps/api/core";
import { runScrollTest } from "./bench";
import type { ListHandle } from "./list/MessageList";

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * App self-test: adds the account described by OUTPOST_PROBE_ACCOUNT, lets
 * sync run, and reports what the core says and what the window shows. This
 * is the only test that covers the window, the core and sync together.
 */
async function probe(details: string): Promise<string> {
  const { core } = await import("./core");
  const events: string[] = [];
  const stop = core.onEvent((e) => events.push(e.type === "accountStatus" ? `status:${e.status.state}` : e.type));
  const text = (selector: string) =>
    [...document.querySelectorAll(selector)].map((el) => el.textContent?.trim() ?? "").filter(Boolean);
  const lines: string[] = ["Outpost app self-test"];
  try {
    await sleep(800);
    await core.send({ type: "addAccount", ...JSON.parse(details) });
    // Long enough for sign-in, a first sync, and the window to catch up.
    const deadline = Date.now() + 20_000;
    while (Date.now() < deadline && !events.includes("status:upToDate")) await sleep(100);
    await sleep(1500);
    const accounts = (await core.send({ type: "listAccounts" })).accounts;
    lines.push(`accounts: ${accounts.map((a) => `${a.name} [${a.status.state}]`).join(", ")}`);
    lines.push(`events: ${[...new Set(events)].join(", ")}`);
    lines.push(`tree: ${text(".folders .account-name, .folders .folder-name").join(" | ")}`);
    lines.push(`list header: ${text(".list-header").join(" ")}`);
    lines.push(`rows shown: ${text(".list-holder .row:not(.idle) .row-subject").join(" | ")}`);
    lines.push(`status bar: ${text(".status-text").join(" ")}`);

    // Mail delivered while the window is open should appear by itself.
    await invoke("shell_test_report", { report: "READY" }).catch(() => {});
    const before = text(".list-holder .row:not(.idle) .row-subject").length;
    const until = Date.now() + 15_000;
    while (Date.now() < until && text(".list-holder .row:not(.idle) .row-subject").length === before) await sleep(100);
    await sleep(500);
    lines.push(`rows after new mail: ${text(".list-holder .row:not(.idle) .row-subject").join(" | ")}`);
    lines.push(`list header after: ${text(".list-header").join(" ")}`);

    // Reading and searching through the real window.
    (document.querySelector(".list-holder .row:not(.idle)") as HTMLElement | null)?.dispatchEvent(
      new MouseEvent("mousedown", { bubbles: true }),
    );
    await sleep(700);
    lines.push(`reader: ${text(".message h1").join("")} / ${text(".message-text").join("").slice(0, 60)}`);
    const input = document.querySelector(".search input") as HTMLInputElement;
    input.value = "zebra";
    input.dispatchEvent(new InputEvent("input", { bubbles: true }));
    await sleep(900);
    lines.push(`search "zebra": ${text(".results .row-subject").join(" | ")}`);
  } catch (e) {
    lines.push(`FAILED: ${e instanceof Error ? e.message : JSON.stringify(e)}`);
  }
  stop();
  return lines.join("\n");
}

export async function autoShellTest(
  list: () => ListHandle | undefined,
  onProgress: (text: string) => void,
): Promise<void> {
  if (!("__TAURI_INTERNALS__" in window)) return;
  if (!(await invoke<boolean>("shell_test_requested"))) return;
  const details = await invoke<string | null>("probe_account");
  if (details) {
    await invoke("shell_test_report", { report: await probe(details) });
    return;
  }
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
