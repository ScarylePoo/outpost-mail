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
  const stop = core.onEvent((e) =>
    events.push(
      e.type === "accountStatus"
        ? `status:${e.status.state}`
        : e.type === "accountSetupFailed"
          ? `accountSetupFailed (${e.error})`
          : e.type,
    ),
  );
  const text = (selector: string) =>
    [...document.querySelectorAll(selector)].map((el) => el.textContent?.trim() ?? "").filter(Boolean);
  const lines: string[] = ["Outpost app self-test"];
  try {
    await sleep(800);
    await core.send({ type: "addAccount", ...JSON.parse(details) });
    // Long enough for sign-in, a first sync, and the window to catch up.
    const deadline = Date.now() + 20_000;
    while (Date.now() < deadline && !events.some((e) => e === "status:upToDate" || e.startsWith("accountSetupFailed")))
      await sleep(100);
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
    // Acting on mail through the real toolbar. The open message is the one
    // that arrived late; opening it has marked it read.
    const press = (label: string) =>
      ([...document.querySelectorAll(".toolbar button")].find((b) => b.textContent?.trim() === label) as
        | HTMLButtonElement
        | undefined)?.click();
    const firstRow = () => document.querySelector(".list-holder .row:not(.idle)");
    lines.push(`opened message is now read: ${firstRow()?.classList.contains("unread") ? "NO" : "yes"}`);
    press("Flag");
    await sleep(900);
    lines.push(`after Flag: ${firstRow()?.querySelector(".row-flag") ? "flag shown" : "NO FLAG"}`);
    press("Delete");
    await sleep(1200);
    lines.push(`after Delete: ${text(".list-holder .row:not(.idle) .row-subject").join(" | ")}`);
    lines.push(`selection moved to: ${text(".message h1").join("")}`);
    // Several at once: select everything and mark it all unread.
    document
      .querySelector(".list-holder .list")
      ?.dispatchEvent(new KeyboardEvent("keydown", { key: "a", ctrlKey: true, bubbles: true }));
    await sleep(300);
    lines.push(`after Ctrl+A: ${text(".list-header span").join("")}`);
    press("Mark unread");
    await sleep(1200);
    lines.push(`unread rows after marking all: ${document.querySelectorAll(".list-holder .row.unread:not(.idle)").length}`);
    // Give sync a moment to tell the server before the app closes.
    await sleep(2500);

    const input = document.querySelector(".search input") as HTMLInputElement;
    input.value = "zebra";
    input.dispatchEvent(new InputEvent("input", { bubbles: true }));
    await sleep(900);
    lines.push(`search "zebra": ${text(".results .row-subject").join(" | ")}`);

    // Writing and sending through the real composer.
    input.value = "";
    input.dispatchEvent(new InputEvent("input", { bubbles: true }));
    await sleep(300);
    const type = (el: Element | null | undefined, value: string) => {
      const field = el as HTMLInputElement | HTMLTextAreaElement;
      field.value = value;
      field.dispatchEvent(new InputEvent("input", { bubbles: true }));
    };
    press("Reply");
    await sleep(700);
    const replyFields = [...document.querySelectorAll(".composer-line input")] as HTMLInputElement[];
    lines.push(`reply starts as: to "${replyFields[0]?.value}" subject "${replyFields.at(-1)?.value}"`);
    const quoted = (document.querySelector(".composer-body") as HTMLTextAreaElement | null)?.value ?? "";
    lines.push(`reply quotes the original: ${quoted.includes("> ") ? "yes" : "NO"}`);
    type(document.querySelector(".composer-body"), `Written in the window.${quoted}`);
    (document.querySelector(".composer button.primary") as HTMLButtonElement | null)?.click();
    await sleep(700);
    lines.push(`undo bar: ${text(".undo-bar span").join("")}`);
    lines.push(`outbox in tree while waiting: ${text(".folders .folder-name").includes("Outbox") ? "yes" : "NO"}`);
    // Long enough for the undo pause to run out and the message to go.
    await sleep(14_000);
    lines.push(`tree after sending: ${text(".folders .folder-name").join(" | ")}`);

    // The reply and the message it answers are now one conversation.
    const top = document.querySelector(".list-holder .row:not(.idle)");
    lines.push(`conversation row: ${top?.querySelector(".row-sender")?.textContent} / ${top?.querySelector(".row-subject")?.textContent}`);
    top?.querySelector(".row-toggle")?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    await sleep(900);
    lines.push(`opened into a tree of: ${text(".tree-line .tree-sender").join(" > ")}`);
    lines.push(`reading pane shows: ${text(".turn .turn-who strong").join(" then ")} (${text(".modes button.on").join("")})`);
    lines.push(`reduced text of the reply: ${text(".turn .turn-text").at(-1)?.slice(0, 40)}`);

    // Pick the reply inside the tree (it is in Sent) and flag it: the change
    // refreshes the folder, and the conversation must stay open.
    document.querySelectorAll(".tree-line")[1]?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    await sleep(500);
    press("Flag");
    await sleep(1500);
    lines.push(
      `after flagging the reply in the tree: ${text(".tree-line").length} lines still open, picked: ${text(".tree-line.selected .tree-sender").join("")}`,
    );
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
