// The main window: folder tree, message list, reading pane, status bar.

import { createMemo, createSignal, For, Match, onCleanup, onMount, Show, Switch } from "solid-js";
import { core, type AccountInfo, type FolderInfo, type MessageRow, type SyncStatus } from "./core";
import { runScrollTest } from "./bench";
import { MessageList, type ListHandle } from "./list/MessageList";
import { SearchResults } from "./list/SearchResults";
import { Reader } from "./reader/Reader";
import { AddAccount, PasswordPrompt } from "./accounts/AccountPanels";
import { autoShellTest } from "./autotest";

type Panel = { kind: "none" } | { kind: "add" } | { kind: "password"; account: string };

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
  const searching = () => query().trim().length > 0;
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
    if (panel().kind !== "none") setPanel({ kind: "none" });
  }

  onMount(() => {
    core
      .send({ type: "ping" })
      .then((r) => setCoreVersion(r.coreVersion))
      .catch((e) => setNotice(`Core not responding: ${e?.message ?? e}`));
    void loadAccounts();
    // Development aids, applied once the first folder is open:
    // `?panel=add` opens the add-account panel, `?q=words` starts a search.
    const params = new URLSearchParams(location.search);
    void loadFolders().then(() => {
      if (params.get("panel") === "add") setPanel({ kind: "add" });
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
          foldersTimer = setTimeout(() => void loadFolders(), 100);
          break;
        case "accountAdded":
          void loadAccounts();
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

    // And `?select=N` opens the Nth message once the list has loaded.
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

  const canSync = () => {
    const state = account()?.status.state;
    return state !== undefined && state !== "notSyncing" && state !== "needsPassword";
  };

  return (
    <div class="app">
      <header class="toolbar">
        <span class="brand">Outpost</span>
        <button onClick={() => setPanel({ kind: "add" })}>Add account</button>
        <button onClick={syncNow} disabled={!canSync()} title="Check every folder for changes now">
          Sync now
        </button>
        <span class="toolbar-note">Preview build: read-only. Nothing you do here changes your mail.</span>
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
      </nav>

      <main class="list-pane">
        <Show when={folder()} keyed>
          {(f) => (
            <>
              <div class="list-header">
                <strong>{f.name}</strong>
                <span>{f.total.toLocaleString()} messages</span>
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
                    setMessage(undefined);
                  }}
                  onKeyDown={(e) => e.key === "Escape" && setQuery("")}
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
                  onSelect={(row) => !searching() && setMessage(row)}
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
          <Match when={folderId()}>{(id) => <Reader folder={id()} row={message()} />}</Match>
        </Switch>
      </section>

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
