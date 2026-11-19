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
    const { dav, ...mail } = JSON.parse(details);
    await core.send({ type: "addAccount", ...mail });
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
    // Shift-click and Ctrl-click onto unread messages: the selection grows,
    // and nothing is opened or marked read on the way.
    const rowsNow = () => [...document.querySelectorAll(".list-holder .row:not(.idle)")] as HTMLElement[];
    const unreadAt = rowsNow().findIndex((r, i) => i > 0 && r.classList.contains("unread"));
    const unreadBefore = document.querySelectorAll(".list-holder .row.unread:not(.idle)").length;
    rowsNow()[unreadAt]?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, shiftKey: true }));
    await sleep(1500);
    const afterShift = text(".list-header span").join("");
    const other = rowsNow().findIndex((r, i) => i > unreadAt && r.classList.contains("unread"));
    rowsNow()[other]?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, ctrlKey: true }));
    await sleep(1500);
    lines.push(
      `shift-click to an unread message (row ${unreadAt + 1}): ${afterShift}; then ctrl-click another (row ${other + 1}): ${text(".list-header span").join("")}; ` +
        `unread rows ${unreadBefore} -> ${document.querySelectorAll(".list-holder .row.unread:not(.idle)").length}; reading pane: ${text(".reading-empty").join("") || "A MESSAGE IS OPEN"}`,
    );
    rowsNow()[0]?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    await sleep(500);
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

    // Sender checks. Message 2 was delivered as a forgery; the rest pass.
    const open = async (subject: string) => {
      const row = [...document.querySelectorAll(".list-holder .row:not(.idle)")].find((r) =>
        r.querySelector(".row-subject")?.textContent?.startsWith(subject),
      );
      row?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
      await sleep(1200);
    };
    const account = (await core.send({ type: "listAccounts" })).accounts[0];
    lines.push(`checks believed from: ${account?.checkingServer} (confirmed: ${account?.checkingServerConfirmed})`);
    lines.push(`rows marked as failed: ${text(".list-holder .row:not(.idle)").filter((t) => t.includes("Failed checks")).length}`);
    await open("Selftest message 2");
    lines.push(`forged message: ${text(".failed h2").join("")} | ${text(".failed-who dd").join(" ; ")}`);
    lines.push(`its checks: ${text(".failed .check-lines li").join(" / ").replace(/\s+/g, " ")}`);
    lines.push(`its actions: ${text(".failed-actions button").join(" | ")}`);
    lines.push(`green checks on it: ${document.querySelectorAll(".message .check-label.verified").length}`);
    await open("Selftest message 3");
    lines.push(`genuine message: ${text(".message .check-label").join("")} (banner: ${document.querySelectorAll(".failed").length})`);

    // Pictures and links. Message 6 is formatted, with a picture on the
    // test's own web server; message 7 is plain text with addresses in it.
    await open("Selftest message 6");
    const frame = () => (document.querySelector(".message-frame") as HTMLIFrameElement | null)?.contentDocument;
    lines.push(`picture message at first: ${text(".message > .message-note").join(" ")} (pictures in it: ${frame()?.querySelectorAll("img[src]").length})`);
    const clickLink = async () => {
      // A real click lands on whatever is on top at that point of the link.
      const box = frame()?.querySelector("a b")?.getBoundingClientRect();
      const outer = document.querySelector(".message-frame")?.getBoundingClientRect();
      if (box && outer) (document.elementFromPoint(outer.left + box.left + box.width / 2, outer.top + box.top + box.height / 2) as HTMLElement | null)?.click();
      await sleep(300);
      const shown = text(".link-target").join("") || "NOTHING SHOWN";
      ([...document.querySelectorAll(".link-bar button")].find((b) => b.textContent === "Dismiss") as HTMLElement | undefined)?.click();
      await sleep(100);
      return shown;
    };
    lines.push(`link in a formatted message goes to: ${await clickLink()}`);
    (document.querySelector(".message > .message-note button") as HTMLElement | null)?.click();
    await sleep(2500);
    const picture = frame()?.querySelector("img") as HTMLImageElement | null | undefined;
    lines.push(`after "Always load pictures": ${picture?.naturalWidth ? "picture loaded" : "PICTURE NOT LOADED"}`);
    lines.push(`the same link once the pictures are in: ${await clickLink()} (links in it: ${frame()?.querySelectorAll("a[href]").length}, frame ${(document.querySelector(".message-frame") as HTMLElement | null)?.style.height} tall)`);
    await open("Selftest message 7");
    lines.push(`addresses made into links in plain text: ${text(".message .text-link, .turn .text-link").join(" | ") || "NONE"}`);
    (document.querySelectorAll(".message .text-link, .turn .text-link")[1] as HTMLElement | undefined)?.click();
    await sleep(300);
    lines.push(`the second one goes to: ${text(".link-target").join("") || "NOTHING SHOWN"}`);
    const p0 = await core.send({ type: "getProtection" });
    lines.push(`senders allowed pictures: ${p0.protection.allowImagesFrom.join(", ")}`);

    // The settings window: every section on one page, narrowed by search.
    press("Settings");
    await sleep(500);
    lines.push(`settings sections: ${text(".settings-nav button").join(" | ")}`);
    lines.push(`settings shown first: ${text(".settings-body h2").join(" | ")} (${text(".settings-body h3").join(", ")})`);
    // Appearance: a colour scheme and a size are applied to the window.
    ([...document.querySelectorAll(".settings-nav button")].find((b) => b.textContent?.trim() === "Appearance") as
      | HTMLElement
      | undefined)?.click();
    await sleep(300);
    const scheme = [...document.querySelectorAll(".scheme")].find((b) => b.textContent?.includes("Slate")) as HTMLElement | undefined;
    scheme?.click();
    await sleep(300);
    const root = document.documentElement;
    lines.push(`scheme after choosing Slate: ${root.dataset.scheme} (dark: ${root.dataset.dark}, pane ${getComputedStyle(root).getPropertyValue("--pane").trim()})`);
    const kept = await core.send({ type: "getAppearance" });
    lines.push(`scheme kept by the core: ${kept.appearance.scheme}`);
    // The size setting zooms the whole window: the same window is then
    // fewer CSS pixels wide.
    const wide = window.innerWidth;
    await core.setZoom(1.25);
    await sleep(400);
    lines.push(`window at 125%: ${wide} -> ${window.innerWidth} px wide (${wide / window.innerWidth > 1.2 ? "zoomed" : "NOT ZOOMED"})`);
    await core.setZoom(1);
    await sleep(200);
    lines.push(`fonts found on this computer: ${document.querySelectorAll(".settings-body select option").length > 12 ? "yes" : "few"}`);
    await core.send({ type: "setAppearance", appearance: { ...kept.appearance, scheme: "system" } });
    const rowHeight = () => (document.querySelector(".list-holder .row:not(.idle)") as HTMLElement | null)?.style.height ?? "?";
    const snug = rowHeight();
    await (await import("./core/appearance")).saveAppearance({ ...kept.appearance, scheme: "system", rowSpacing: 16 });
    await sleep(400);
    lines.push(`a row at "Airy" spacing: ${snug} -> ${rowHeight()} tall`);
    await (await import("./core/appearance")).saveAppearance({ ...kept.appearance, scheme: "system" });
    const search = document.querySelector(".settings-search") as HTMLInputElement | null;
    if (search) {
      search.value = "signature";
      search.dispatchEvent(new InputEvent("input", { bubbles: true }));
    }
    await sleep(300);
    lines.push(`settings matching "signature": ${text(".settings-body h2").join(" | ")} (${text(".settings-body .setting").length} settings)`);
    lines.push(`menus: ${text(".menubar .menu-title").join(" | ")}`);
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await sleep(300);
    lines.push(`settings closed by Escape: ${document.querySelector(".settings") ? "NO" : "yes"}`);

    // Calendars, tasks and contacts, against the test's own server.
    if (dav) {
      await core.send({ type: "addDavAccount", url: dav.url, username: dav.username, password: dav.password, remember: false });
      const until = Date.now() + 15_000;
      while (Date.now() < until && !events.some((e) => e === "davAccountAdded" || e === "davAccountSetupFailed")) await sleep(100);
      await sleep(1500);
      const tab = (label: string) =>
        ([...document.querySelectorAll(".switcher button")].find((b) => b.textContent?.trim() === label) as HTMLButtonElement | undefined)?.click();
      const type = (selector: string, value: string) => {
        const input = document.querySelector(selector) as HTMLInputElement | null;
        if (!input) return false;
        input.value = value;
        input.dispatchEvent(new InputEvent("input", { bubbles: true }));
        return true;
      };
      tab("Calendar");
      await sleep(700);
      lines.push(`calendar sidebar: ${text(".side-collection").join(" | ")}`);
      ([...document.querySelectorAll(".organizer-head button")].find((b) => b.textContent?.trim() === "New event") as HTMLButtonElement | undefined)?.click();
      await sleep(400);
      type(".organizer-side .panel input[type=text]", "Made in the window, with a comma");
      (document.querySelector(".organizer-side .panel button.primary") as HTMLButtonElement | null)?.click();
      await sleep(1500);
      lines.push(`calendar shows today: ${text(".month-day.today .month-event").join(" | ")}`);
      tab("Tasks");
      await sleep(500);
      lines.push(`tasks before: ${text(".task-list li .task-title").join(" | ") || "(none)"}`);
      type(".task-add input", "Buy milk");
      (document.querySelector(".task-add") as HTMLFormElement | null)?.requestSubmit();
      await sleep(1200);
      lines.push(`tasks after adding one: ${text(".task-list li .task-title").join(" | ")}`);
      (document.querySelector(".task-list li input[type=checkbox]") as HTMLInputElement | null)?.click();
      await sleep(1200);
      lines.push(`ticked off: ${document.querySelectorAll(".task-list li.done").length} of ${document.querySelectorAll(".task-list li").length}`);
      tab("People");
      await sleep(700);
      lines.push(`people: ${text(".people-list .person strong").join(" | ")}`);
      // Time for the changes to reach the server before the app closes.
      await sleep(2500);
      tab("Mail");
    } else {
      lines.push("calendar: not tested here (no calendar server was given)");
    }
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
