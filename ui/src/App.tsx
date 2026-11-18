// The main window: folder tree, message list, reading pane, status bar.

import { createEffect, createMemo, createSignal, For, Match, onCleanup, onMount, Show, Switch } from "solid-js";
import {
  core,
  type AccountInfo,
  type DraftContent,
  type FolderInfo,
  type MessageRow,
  type SyncStatus,
} from "./core";
import { Composer, EMPTY_DRAFT } from "./compose/Composer";
import { runScrollTest } from "./bench";
import { MessageList, type ListHandle } from "./list/MessageList";
import { SearchResults } from "./list/SearchResults";
import { Reader } from "./reader/Reader";
import { AddAccount, PasswordPrompt } from "./accounts/AccountPanels";
import { autoShellTest } from "./autotest";
import { asking, general, loadProtection, messageActions } from "./core/protection";
import { PasswordAsk, Settings } from "./settings/Settings";
import { MenuBar, type Menu } from "./MenuBar";
import { pressed, SHORTCUTS } from "./shortcuts";

type Panel =
  | { kind: "none" }
  | { kind: "add" }
  | { kind: "settings"; find?: string }
  | { kind: "password"; account: string }
  | { kind: "compose"; account: string; draft: DraftContent; key: number };

/** The account a folder (or account) id belongs to: `acct/<name>`. */
function accountOf(id: string): string {
  return id.split("/").slice(0, 2).join("/");
}

function statusText(status: SyncStatus | undefined): string {
  switch (status?.state) {
    case undefined:
      return "";
    case "notSyncing":
      return "Not syncing: this mail is only stored here";
    case "indexing":
      return status.detail || "Reading the mail on this computer…";
    case "needsPassword":
      return "Password needed to sync";
    case "connecting":
      return "Connecting…";
    case "syncing":
      return status.detail || "Syncing…";
    case "upToDate":
      return "Up to date";
    case "offline":
      return `Offline. ${status.detail}`;
    case "signInFailed":
      return `Sign-in failed. ${status.detail}`;
  }
}

export function App() {
  const [folders, setFolders] = createSignal<FolderInfo[]>([]);
  const [accounts, setAccounts] = createSignal<AccountInfo[]>([]);
  const [folderId, setFolderId] = createSignal<string>();
  const [message, setMessage] = createSignal<MessageRow>();
  /** Whether the list shows one row per conversation or one per message. */
  // The setting says how folders open; the button on the list switches for
  // the time being, and that choice is remembered on this computer.
  const [layoutChoice, setLayoutChoice] = createSignal(localStorage.getItem("outpost.layout"));
  const conversations = () => (layoutChoice() ? layoutChoice() !== "messages" : general().conversations);
  function chooseLayout(grouped: boolean) {
    setLayoutChoice(grouped ? "conversations" : "messages");
    setMessage(undefined);
    try {
      localStorage.setItem("outpost.layout", grouped ? "conversations" : "messages");
    } catch {
      // Not remembered, then; it still applies for this run.
    }
  }
  /** How many messages are selected in the list. */
  const [count, setCount] = createSignal(0);
  const [panel, setPanel] = createSignal<Panel>({ kind: "none" });
  const [query, setQuery] = createSignal("");
  const [everywhere, setEverywhere] = createSignal(false);
  const [coreVersion, setCoreVersion] = createSignal("…");
  const [notice, showNotice] = createSignal("");
  let noticeTimer: ReturnType<typeof setTimeout> | undefined;
  /** Shows a one-off message in the status bar, then lets sync status back. */
  function setNotice(text: string) {
    showNotice(text);
    clearTimeout(noticeTimer);
    if (text) noticeTimer = setTimeout(() => showNotice(""), 8000);
  }
  const [report, setReport] = createSignal("");
  const [testing, setTesting] = createSignal(false);
  const [copied, setCopied] = createSignal(false);
  let list: ListHandle | undefined;
  let foldersTimer: ReturnType<typeof setTimeout> | undefined;
  let listTimer: ReturnType<typeof setTimeout> | undefined;

  const folder = createMemo(() => folders().find((f) => f.id === folderId()));
  const account = createMemo(() => {
    const id = folderId();
    return id ? accounts().find((a) => a.id === accountOf(id)) : undefined;
  });
  const passwordAccount = createMemo(() => {
    const p = panel();
    return p.kind === "password" ? accounts().find((a) => a.id === p.account) : undefined;
  });
  const settingsOpen = createMemo(() => {
    const p = panel();
    return p.kind === "settings" ? p : undefined;
  });
  const searching = () => query().trim().length > 0;
  /** The junk folder of the account on show, if it has one. */
  const junkFolder = () => {
    const id = folderId();
    return id ? folders().find((f) => f.role === "junk" && accountOf(f.id) === accountOf(id)) : undefined;
  };
  const needsSignIn = (a: AccountInfo) => a.status.state === "needsPassword" || a.status.state === "signInFailed";

  function loadFolders(select?: (all: FolderInfo[]) => FolderInfo | undefined) {
    return core
      .send({ type: "listFolders" })
      .then((r) => {
        setFolders(r.folders);
        const wanted = select?.(r.folders);
        if (wanted) {
          openFolder(wanted.id);
        } else if (!r.folders.some((f) => f.id === folderId())) {
          // The open folder is gone (or nothing was open): go to an inbox.
          const first = r.folders.find((f) => f.role === "inbox") ?? r.folders.find((f) => f.depth > 0);
          if (first) openFolder(first.id);
        }
      })
      .catch((e) => setNotice(`Could not list folders: ${e?.message ?? e}`));
  }

  function loadAccounts() {
    return core
      .send({ type: "listAccounts" })
      .then((r) => setAccounts(r.accounts))
      .catch(() => {});
  }

  function openFolder(id: string) {
    if (id === folderId()) return;
    setMessage(undefined);
    setQuery("");
    setFolderId(id);
    // A message being written stays open while the user looks elsewhere.
    if (["add", "password", "settings"].includes(panel().kind)) setPanel({ kind: "none" });
  }

  onMount(() => {
    core
      .send({ type: "ping" })
      .then((r) => setCoreVersion(r.coreVersion))
      .catch((e) => setNotice(`Core not responding: ${e?.message ?? e}`));
    void loadAccounts();
    void loadProtection();
    // What the reading pane's failed-message view can ask for.
    messageActions.remove = () => void remove();
    messageActions.moveToJunk = () => {
      const junk = junkFolder();
      if (junk) void moveTo(junk.id);
    };
    messageActions.hasJunk = () => junkFolder() !== undefined && folder()?.role !== "junk";
    // Development aids, applied once the first folder is open:
    // `?panel=add` opens the add-account panel, `?q=words` starts a search.
    const params = new URLSearchParams(location.search);
    void loadFolders().then(() => {
      if (params.get("panel") === "add") setPanel({ kind: "add" });
      if (params.get("panel") === "protection") setPanel({ kind: "settings", find: "protection" });
      if (params.get("panel") === "settings") setPanel({ kind: "settings", find: params.get("find") ?? "" });
      if (params.get("q")) setQuery(params.get("q")!);
    });

    const stop = core.onEvent((event) => {
      switch (event.type) {
        case "accountStatus":
          setAccounts((all) => all.map((a) => (a.id === event.account ? { ...a, status: event.status } : a)));
          break;
        case "folderChanged":
          // Sync reports changes in bursts. Counts in the tree and the open
          // list each catch up once the burst pauses.
          clearTimeout(foldersTimer);
          foldersTimer = setTimeout(() => void loadFolders(), 400);
          if (event.folder === folderId()) {
            clearTimeout(listTimer);
            listTimer = setTimeout(() => list?.refresh(), 250);
          }
          break;
        case "foldersChanged":
          clearTimeout(foldersTimer);
          foldersTimer = setTimeout(() => {
            void loadFolders();
            // Also sent when the server whose sender checks are believed
            // changes, which changes what the list shows.
            void loadAccounts();
            if (accountOf(folderId() ?? "") === event.account) list?.refresh();
          }, 100);
          break;
        case "accountAdded":
          void loadAccounts();
          break;
        case "accountRemoved":
          setMessage(undefined);
          void loadAccounts();
          void loadFolders();
          break;
        case "newMail":
          if (general().notifyNewMail) {
            const title = event.count === 1 ? event.sender || "New message" : `${event.count} new messages`;
            const body = event.count === 1 ? event.subject || "(no subject)" : `Newest: ${event.sender}: ${event.subject || "(no subject)"}`;
            void core.notify(title, body).catch(() => {});
          }
          break;
        case "notice":
          setNotice(event.text);
          break;
        case "accountSetupFailed":
          break;
      }
    });
    onCleanup(() => {
      stop();
      clearTimeout(foldersTimer);
      clearTimeout(listTimer);
    });

    void autoShellTest(() => list, setNotice);

    // And `?select=N` opens the Nth message once the list has loaded, and
    // `?open=N` opens the conversation at row N into its tree.
    const toOpen = Number(new URLSearchParams(location.search).get("open") ?? NaN);
    if (Number.isInteger(toOpen) && toOpen >= 0) {
      const opener = setInterval(() => {
        if (list && list.rowCount() > toOpen) {
          clearInterval(opener);
          setTimeout(() => list?.toggle(toOpen), 300);
        }
      }, 50);
      onCleanup(() => clearInterval(opener));
    }
    const wanted = Number(params.get("select") ?? NaN);
    if (Number.isInteger(wanted) && wanted >= 0) {
      const timer = setInterval(() => {
        if (list && list.rowCount() > wanted) {
          clearInterval(timer);
          list.select(wanted);
        }
      }, 50);
      onCleanup(() => clearInterval(timer));
    }
  });

  function accountAdded(id: string) {
    setPanel({ kind: "none" });
    void loadAccounts();
    void loadFolders();
    // Show the new account's inbox as soon as the server has listed it.
    const waitForInbox = setInterval(() => {
      const inbox = folders().find((f) => accountOf(f.id) === id && f.role === "inbox");
      if (inbox) {
        clearInterval(waitForInbox);
        openFolder(inbox.id);
      }
    }, 200);
    setTimeout(() => clearInterval(waitForInbox), 30_000);
  }

  function syncNow() {
    const a = account();
    if (!a) return;
    core.send({ type: "syncNow", account: a.id }).catch((e) => setNotice(String(e?.message ?? e)));
  }

  async function scrollTest() {
    if (!list || testing()) return;
    setTesting(true);
    setReport("");
    try {
      setReport(await runScrollTest(list, setNotice));
      setNotice("Scroll test finished");
    } finally {
      setTesting(false);
    }
  }

  async function copyReport() {
    await navigator.clipboard.writeText(report());
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  }

  // ---- acting on the selected message -----------------------------------

  function send(command: Parameters<typeof core.send>[0]) {
    core.send(command).catch((e) => setNotice(String(e?.message ?? e)));
  }

  /** The selected message and its folder, if the account can be changed. */
  function target() {
    const row = message();
    const id = folderId();
    if (!row || !id || !canSync()) return undefined;
    return { row, folder: id };
  }

  /**
   * Every selected message: the whole selection in the list, or the one
   * message open from a search result.
   */
  async function chosen(): Promise<{ folder: string; row: MessageRow; ids: number[] } | undefined> {
    const t = target();
    if (!t) return undefined;
    if (searching() || !list || count() <= 1) return { ...t, ids: [t.row.id] };
    try {
      return { ...t, ids: await list.selectedIds() };
    } catch (e) {
      setNotice(String((e as Error)?.message ?? e));
      return undefined;
    }
  }

  async function markRead(read: boolean) {
    const c = await chosen();
    if (c) send({ type: "changeFlags", folder: c.folder, ids: c.ids, seen: read, flagged: null });
  }

  async function toggleFlag() {
    const c = await chosen();
    if (c) send({ type: "changeFlags", folder: c.folder, ids: c.ids, seen: null, flagged: !c.row.flagged });
  }

  async function archive() {
    const c = await chosen();
    if (c) send({ type: "archiveMessages", folder: c.folder, ids: c.ids });
  }

  async function moveTo(destination: string) {
    const c = await chosen();
    if (c && destination) send({ type: "moveMessages", ids: c.ids, to: destination });
  }

  async function remove() {
    const c = await chosen();
    if (!c) return;
    // From the trash, deleting is for good, which is worth a question.
    // A message picked inside a conversation may be in another folder; only
    // what is actually in the trash is removed for good.
    const elsewhere = c.ids.length === 1 && c.row.inFolder !== undefined && c.row.inFolder !== c.folder;
    const permanently = folder()?.role === "trash" && !elsewhere;
    const what = c.ids.length > 1 ? `these ${c.ids.length.toLocaleString()} messages` : `"${c.row.subject || "(no subject)"}"`;
    if (permanently && !window.confirm(`Permanently delete ${what}? This cannot be undone.`)) return;
    send({ type: "deleteMessages", folder: c.folder, ids: c.ids, permanently });
  }

  /** Opening a message marks it read, as in Outlook. */
  function opened(row: MessageRow) {
    const id = folderId();
    if (row.unread && id && canSync()) {
      send({ type: "changeFlags", folder: id, ids: [row.id], seen: true, flagged: null });
    }
  }

  // ---- folders ------------------------------------------------------------

  /** The account (or folder) a new folder is being named under, if any. */
  const [naming, setNaming] = createSignal<string>();

  function createFolder(parent: string, name: string) {
    setNaming(undefined);
    if (!name.trim()) return;
    core
      .send({ type: "createFolder", parent, name })
      .then((r) => loadFolders((all) => all.find((f) => f.id === r.folder)))
      .catch((e) => setNotice(String(e?.message ?? e)));
  }

  /** An ordinary folder with nothing inside it can be deleted. */
  const deletable = createMemo(() => {
    const f = folder();
    if (!f || f.role !== null || !canSync()) return false;
    const all = folders();
    const next = all[all.findIndex((x) => x.id === f.id) + 1];
    return !next || next.depth <= f.depth;
  });

  function deleteFolder() {
    const f = folder();
    if (!f) return;
    const mail = f.total > 0 ? ` and the ${f.total.toLocaleString()} message${f.total === 1 ? "" : "s"} in it` : "";
    if (!window.confirm(`Delete the folder "${f.name}"${mail}? This cannot be undone.`)) return;
    send({ type: "deleteFolder", folder: f.id });
  }

  // ---- writing ------------------------------------------------------------

  const [undo, setUndo] = createSignal<{ account: string; id: number; subject: string }>();
  let undoTimer: ReturnType<typeof setTimeout> | undefined;
  let composeKey = 0;

  /** The account to write from: the open folder's, if it can send. */
  function writingAccount() {
    const a = account();
    return a && canSync() ? a : undefined;
  }

  function compose(draft: DraftContent, accountId?: string) {
    const id = accountId ?? writingAccount()?.id;
    if (id) setPanel({ kind: "compose", account: id, draft, key: ++composeKey });
  }

  function respond(mode: "reply" | "replyAll" | "forward") {
    const t = target();
    if (!t) return;
    core
      .send({ type: "composeReply", folder: t.folder, id: t.row.id, mode })
      .then((r) => compose(r.draft))
      .catch((e) => setNotice(String(e?.message ?? e)));
  }

  /** Reopens a draft, or a message left in the outbox, for editing. */
  function editDraft(id: number, accountId?: string) {
    const folderOfAccount = folderId();
    if (!folderOfAccount) return;
    core
      .send({ type: "editDraft", folder: folderOfAccount, id })
      .then((r) => compose(r.draft, accountId))
      .catch((e) => setNotice(String(e?.message ?? e)));
  }

  function sent(accountId: string, id: number, subject: string) {
    setPanel({ kind: "none" });
    clearTimeout(undoTimer);
    if (general().undoSendSeconds > 0) setUndo({ account: accountId, id, subject });
    if (general().undoSendSeconds === 0) return;
    undoTimer = setTimeout(() => setUndo(undefined), general().undoSendSeconds * 1000);
  }

  function undoSend() {
    const pending = undo();
    if (!pending) return;
    clearTimeout(undoTimer);
    setUndo(undefined);
    core
      .send({ type: "cancelSend", account: pending.account, id: pending.id })
      .then((r) => {
        if (r.cancelled) editDraft(pending.id, pending.account);
        else setNotice("Too late to undo: the message has been sent.");
      })
      .catch((e) => setNotice(String(e?.message ?? e)));
  }

  const composing = createMemo(() => {
    const p = panel();
    if (p.kind !== "compose") return undefined;
    const a = accounts().find((a) => a.id === p.account);
    return a ? { account: a, draft: p.draft, key: p.key } : undefined;
  });
  const editable = () => ["drafts", "outbox"].includes(folder()?.role ?? "");

  /** Writes the open message to the downloads folder as a mail file. */
  function exportMessage() {
    const t = target() ?? (message() && folderId() ? { row: message()!, folder: folderId()! } : undefined);
    if (!t) return;
    core
      .send({ type: "exportMessage", folder: t.folder, id: t.row.id })
      .then((r) => setNotice(`Saved to your Downloads folder as ${r.name}`))
      .catch((e) => setNotice(String(e?.message ?? e)));
  }

  function focusSearch() {
    setPanel({ kind: "none" });
    queueMicrotask(() => (document.querySelector(".search input") as HTMLInputElement | null)?.focus());
  }

  /** What each shortcut does, and whether it can be done right now. */
  const actions: Record<string, { run: () => void; can: () => boolean }> = {
    new: { run: () => compose(EMPTY_DRAFT), can: () => writingAccount() !== undefined },
    reply: { run: () => respond("reply"), can: () => target() !== undefined },
    replyAll: { run: () => respond("replyAll"), can: () => target() !== undefined },
    forward: { run: () => respond("forward"), can: () => target() !== undefined },
    delete: { run: () => void remove(), can: () => target() !== undefined },
    archive: { run: () => void archive(), can: () => target() !== undefined },
    flag: { run: () => void toggleFlag(), can: () => target() !== undefined },
    read: { run: () => void markRead(true), can: () => target() !== undefined },
    unread: { run: () => void markRead(false), can: () => target() !== undefined },
    search: { run: focusSearch, can: () => folderId() !== undefined },
    sync: { run: syncNow, can: () => canSync() },
    print: { run: () => window.print(), can: () => message() !== undefined },
    settings: { run: () => setPanel({ kind: "settings" }), can: () => true },
    close: { run: () => setPanel({ kind: "none" }), can: () => ["settings", "add", "password"].includes(panel().kind) },
  };

  // Outlook's shortcuts. Most apply only when the keyboard is not in a text
  // field and no panel is open; a few always do.
  function onKey(e: KeyboardEvent) {
    const typing = (e.target as HTMLElement | null)?.closest?.("input, textarea, select, [contenteditable]");
    const busy = Boolean(typing) || panel().kind !== "none";
    for (const shortcut of SHORTCUTS) {
      if (shortcut.elsewhere || !pressed(e, shortcut.keys)) continue;
      const action = actions[shortcut.id];
      if (!action || (busy && !shortcut.always) || !action.can()) return;
      action.run();
      e.preventDefault();
      return;
    }
  }
  onMount(() => {
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });

  /** Folders of the open account that messages can be moved into. */
  const destinations = createMemo(() => {
    const id = folderId();
    return id ? folders().filter((f) => f.depth > 0 && accountOf(f.id) === accountOf(id) && f.id !== id) : [];
  });

  const canSync = () => {
    const state = account()?.status.state;
    return state !== undefined && state !== "notSyncing" && state !== "needsPassword";
  };

  // The tray icon shows how much unread mail the inboxes hold.
  createEffect(() => {
    const unread = folders()
      .filter((f) => f.role === "inbox")
      .reduce((sum, f) => sum + f.unread, 0);
    void core.setUnread(unread).catch(() => {});
  });

  const key = (id: string) => SHORTCUTS.find((s) => s.id === id)?.keys;
  const entry = (id: string, label: string) => ({ label, keys: key(id), run: actions[id]!.run, disabled: !actions[id]!.can() });
  const menus = (): Menu[] => [
    {
      title: "File",
      entries: [
        entry("new", "New message"),
        { label: "Add account…", run: () => setPanel({ kind: "add" }) },
        null,
        { label: "Save message as a file", run: exportMessage, disabled: !message() },
        entry("print", "Print…"),
        null,
        entry("settings", "Settings"),
      ],
    },
    {
      title: "View",
      entries: [
        { label: "Conversations", run: () => chooseLayout(true), checked: conversations() },
        { label: "Single messages", run: () => chooseLayout(false), checked: !conversations() },
        null,
        entry("search", "Search"),
      ],
    },
    {
      title: "Message",
      entries: [
        entry("reply", "Reply"),
        entry("replyAll", "Reply to all"),
        entry("forward", "Forward"),
        null,
        entry("read", "Mark as read"),
        entry("unread", "Mark as unread"),
        entry("flag", message()?.flagged ? "Unflag" : "Flag"),
        null,
        entry("archive", "Archive"),
        {
          label: "Move to junk",
          run: () => messageActions.moveToJunk?.(),
          disabled: !target() || !messageActions.hasJunk?.(),
        },
        entry("delete", "Delete"),
      ],
    },
    {
      title: "Tools",
      entries: [
        entry("sync", "Check for new mail"),
        null,
        { label: "Protection settings", run: () => setPanel({ kind: "settings", find: "protection" }) },
        { label: "Account settings", run: () => setPanel({ kind: "settings", find: "account" }) },
      ],
    },
    {
      title: "Help",
      entries: [
        { label: "Keyboard shortcuts", run: () => setPanel({ kind: "settings", find: "shortcut" }) },
        { label: "About Outpost", run: () => setPanel({ kind: "settings", find: "about" }) },
      ],
    },
  ];

  return (
    <div class="app">
      <MenuBar menus={menus()} />
      <header class="toolbar">
        <span class="brand">Outpost</span>
        <button onClick={() => setPanel({ kind: "add" })}>Add account</button>
        <button onClick={() => setPanel({ kind: "settings" })} title="Settings (Ctrl+,)">
          Settings
        </button>
        <button onClick={syncNow} disabled={!canSync()} title="Check every folder for changes now">
          Sync now
        </button>
        <span class="toolbar-gap" />
        <button class="primary" onClick={() => compose(EMPTY_DRAFT)} disabled={!writingAccount()} title="New message (Ctrl+N)">
          New
        </button>
        <Show
          when={editable()}
          fallback={
            <>
              <button onClick={() => respond("reply")} disabled={!target()} title="Reply (Ctrl+R)">
                Reply
              </button>
              <button onClick={() => respond("replyAll")} disabled={!target()} title="Reply to all (Ctrl+Shift+R)">
                Reply all
              </button>
              <button onClick={() => respond("forward")} disabled={!target()} title="Forward (Ctrl+F)">
                Forward
              </button>
            </>
          }
        >
          <button onClick={() => message() && editDraft(message()!.id)} disabled={!target()}>
            Edit
          </button>
        </Show>
        <span class="toolbar-gap" />
        <button onClick={remove} disabled={!target()} title="Delete (Del)">
          Delete
        </button>
        <button onClick={archive} disabled={!target()} title="Archive (Backspace)">
          Archive
        </button>
        <select
          class="move"
          disabled={!target()}
          title="Move to another folder"
          onChange={(e) => {
            void moveTo(e.currentTarget.value);
            e.currentTarget.value = "";
          }}
        >
          <option value="">Move to…</option>
          <For each={destinations()}>
            {(f) => <option value={f.id}>{"\u00a0\u00a0".repeat(f.depth - 1) + f.name}</option>}
          </For>
        </select>
        <button
          onClick={() => void markRead(message()?.unread ?? false)}
          disabled={!target()}
          title="Mark read (Ctrl+Q) or unread (Ctrl+U)"
        >
          {message()?.unread ? "Mark read" : "Mark unread"}
        </button>
        <button onClick={toggleFlag} disabled={!target()} title="Flag (Insert)">
          {message()?.flagged ? "Unflag" : "Flag"}
        </button>
        <span class="toolbar-note" />
        <button class="quiet" onClick={scrollTest} disabled={testing() || searching() || !folder()?.total}>
          Scroll test
        </button>
      </header>

      <nav class="folders" aria-label="Folders">
        <For each={folders()}>
          {(f) => (
            <Show
              when={f.depth > 0}
              fallback={
                <div class="account">
                  <span class="account-name">{f.name}</span>
                  <Show when={accounts().find((a) => a.id === f.id && a.host)}>
                    <button class="new-folder" title="New folder" onClick={() => setNaming(f.id)}>
                      +
                    </button>
                  </Show>
                  <Show when={accounts().find((a) => a.id === f.id && needsSignIn(a))}>
                    <button class="sign-in" onClick={() => setPanel({ kind: "password", account: f.id })}>
                      Sign in
                    </button>
                  </Show>
                </div>
              }
            >
              <button
                class="folder"
                classList={{ current: f.id === folderId() }}
                style={{ "padding-left": `${f.depth * 14}px` }}
                onClick={() => openFolder(f.id)}
              >
                <span class="folder-name">{f.name}</span>
                <Show when={f.unread > 0}>
                  <span class="folder-count">{f.unread.toLocaleString()}</span>
                </Show>
              </button>
            </Show>
          )}
        </For>
        <Show when={naming()}>
          {(parent) => (
            <form
              class="folder-naming"
              onSubmit={(e) => {
                e.preventDefault();
                createFolder(parent(), new FormData(e.currentTarget).get("name")?.toString() ?? "");
              }}
            >
              <input
                name="name"
                type="text"
                placeholder="Name of the new folder"
                aria-label="Name of the new folder"
                ref={(el) => queueMicrotask(() => el.focus())}
                onKeyDown={(e) => e.key === "Escape" && setNaming(undefined)}
                onBlur={() => setNaming(undefined)}
              />
            </form>
          )}
        </Show>
      </nav>

      <main class="list-pane">
        <Show when={folder()} keyed>
          {(f) => (
            <>
              <div class="list-header">
                <strong>{f.name}</strong>
                <button
                  class="inline layout"
                  title="Show one row per conversation, or one per message"
                  onClick={() => chooseLayout(!conversations())}
                >
                  {conversations() ? "Conversations" : "Messages"}
                </button>
                <span>
                  <Show when={count() > 1 && !searching()} fallback={<>{f.total.toLocaleString()} messages</>}>
                    {count().toLocaleString()} of {f.total.toLocaleString()} selected
                  </Show>
                  <Show when={deletable()}>
                    <button class="inline delete-folder" onClick={deleteFolder}>
                      Delete folder
                    </button>
                  </Show>
                </span>
              </div>
            </>
          )}
        </Show>
        <Show when={folderId()}>
          {(id) => (
            <>
              <div class="search">
                <input
                  type="search"
                  placeholder={everywhere() ? "Search all folders" : `Search ${folder()?.name ?? "this folder"}`}
                  aria-label="Search"
                  value={query()}
                  onInput={(e) => {
                    setQuery(e.currentTarget.value);
                    // Back to the list: its selection is the message again.
                    setMessage(e.currentTarget.value.trim() ? undefined : list?.selectedRow());
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Escape") {
                      setQuery("");
                      setMessage(list?.selectedRow());
                    }
                  }}
                />
                <button class="search-scope" title="Choose where to search" onClick={() => setEverywhere((v) => !v)}>
                  {everywhere() ? "All folders" : "This folder"}
                </button>
              </div>
              <Show when={searching()}>
                <SearchResults scope={everywhere() ? accountOf(id()) : id()} text={query()} onSelect={setMessage} />
              </Show>
              {/* Kept alive while searching, so it is where it was afterwards. */}
              <div class="list-holder" classList={{ hidden: searching() }}>
                <MessageList
                  folder={id()}
                  conversations={conversations()}
                  previewLines={general().previewLines}
                  showRecipients={["sent", "drafts", "outbox"].includes(folder()?.role ?? "")}
                  onSelect={(row) => !searching() && setMessage(row)}
                  onCount={setCount}
                  handle={(h) => (list = h)}
                />
              </div>
            </>
          )}
        </Show>
      </main>

      <section class="reading" aria-label="Reading pane">
        <Switch>
          <Match when={panel().kind === "add"}>
            <AddAccount onDone={accountAdded} onCancel={() => setPanel({ kind: "none" })} />
          </Match>
          <Match when={settingsOpen()} keyed>
            {(open) => (
              <Settings
                accounts={accounts()}
                coreVersion={coreVersion()}
                find={open.find}
                onAccountsChanged={() => {
                  void loadAccounts();
                  list?.refresh();
                }}
                onAddAccount={() => setPanel({ kind: "add" })}
                onPassword={(account) => setPanel({ kind: "password", account })}
                onClose={() => setPanel({ kind: "none" })}
              />
            )}
          </Match>
          <Match when={composing()} keyed>
            {(c) => (
              <Composer
                account={c.account}
                initial={c.draft}
                onSent={(id, subject) => sent(c.account.id, id, subject)}
                onClose={(note) => {
                  setPanel({ kind: "none" });
                  if (note) setNotice(note);
                }}
                onSendingSet={() => void loadAccounts()}
              />
            )}
          </Match>
          <Match when={passwordAccount()}>
            {(a) => (
              <PasswordPrompt
                account={a()}
                onDone={() => setPanel({ kind: "none" })}
                onCancel={() => setPanel({ kind: "none" })}
              />
            )}
          </Match>
          <Match when={report()}>
            <div class="report">
              <div class="report-head">
                <strong>Scroll test result</strong>
                <button onClick={copyReport}>{copied() ? "Copied" : "Copy"}</button>
                <button onClick={() => setReport("")}>Close</button>
              </div>
              <pre>{report()}</pre>
            </div>
          </Match>
          <Match when={folderId()}>{(id) => <Reader folder={id()} row={message()} onOpened={opened} />}</Match>
        </Switch>
      </section>

      <Show when={asking()}>{(a) => <PasswordAsk why={a().why} error={a().error} answer={a().answer} />}</Show>

      <Show when={undo()}>
        {(u) => (
          <div class="undo-bar" role="status">
            <span>
              Sending "{u().subject || "(no subject)"}" in a few seconds.
            </span>
            <button onClick={undoSend}>Undo</button>
          </div>
        )}
      </Show>

      <footer class="statusbar">
        <span
          class="status-text"
          classList={{ problem: ["offline", "signInFailed"].includes(account()?.status.state ?? "") }}
        >
          {notice() || statusText(account()?.status)}
        </span>
        <span class="statusbar-right">
          core {coreVersion()} · {core.name}
        </span>
      </footer>
    </div>
  );
}
