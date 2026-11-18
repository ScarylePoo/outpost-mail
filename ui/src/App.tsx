// Milestone 0 main window: the three panes, fed by the core, with the shell
// test on the toolbar. The real window is built in milestone 3.

import { createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { core, type FolderInfo, type MessageRow } from "./core";
import { runScrollTest } from "./bench";
import { MessageList, type ListHandle } from "./list/MessageList";
import { Reader } from "./reader/Reader";
import { autoShellTest } from "./autotest";

export function App() {
  const [folders, setFolders] = createSignal<FolderInfo[]>([]);
  const [folder, setFolder] = createSignal<FolderInfo>();
  const [message, setMessage] = createSignal<MessageRow>();
  const [coreVersion, setCoreVersion] = createSignal("…");
  const [status, setStatus] = createSignal("Ready");
  const [report, setReport] = createSignal("");
  const [testing, setTesting] = createSignal(false);
  const [copied, setCopied] = createSignal(false);
  let list: ListHandle | undefined;

  onMount(() => {
    core
      .send({ type: "ping" })
      .then((r) => setCoreVersion(r.coreVersion))
      .catch((e) => setStatus(`Core not responding: ${e?.message ?? e}`));
    core
      .send({ type: "listFolders" })
      .then((r) => {
        setFolders(r.folders);
        setFolder(r.folders.find((f) => f.depth > 0));
      })
      .catch((e) => setStatus(`Could not list folders: ${e?.message ?? e}`));

    const stop = core.onEvent((event) => {
      const name = folders().find((f) => f.id === event.folder)?.name ?? event.folder;
      if (event.type === "syncProgress") {
        setStatus(
          `Syncing ${name}: ${event.done.toLocaleString()} of ${event.total.toLocaleString()}`,
        );
      } else {
        setStatus(`${name} is up to date`);
      }
    });
    onCleanup(stop);

    void autoShellTest(() => list, setStatus);

    // Development aid: `?select=N` opens the Nth message once the list has
    // loaded, so a particular state can be reached from a URL.
    const wanted = Number(new URLSearchParams(location.search).get("select") ?? NaN);
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

  async function scrollTest() {
    if (!list || testing()) return;
    setTesting(true);
    setReport("");
    try {
      setReport(await runScrollTest(list, setStatus));
      setStatus("Scroll test finished");
    } finally {
      setTesting(false);
    }
  }

  function demoSync() {
    const f = folder();
    if (f) void core.send({ type: "startDemoSync", folder: f.id });
  }

  async function copyReport() {
    await navigator.clipboard.writeText(report());
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  }

  return (
    <div class="app">
      <header class="toolbar">
        <span class="brand">Outpost</span>
        <button onClick={scrollTest} disabled={testing() || !folder()?.total}>
          Run scroll test
        </button>
        <button onClick={demoSync} disabled={!folder()}>
          Test core events
        </button>
        <span class="toolbar-note">Preview build: read-only. Nothing you do here changes your mail.</span>
      </header>

      <nav class="folders" aria-label="Folders">
        <For each={folders()}>
          {(f) => (
            <Show when={f.depth > 0} fallback={<div class="account">{f.name}</div>}>
              <button
                class="folder"
                classList={{ current: f.id === folder()?.id }}
                style={{ "padding-left": `${f.depth * 14}px` }}
                onClick={() => {
                  setMessage(undefined);
                  setFolder(f);
                }}
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
        <Show when={folder()}>
          {(f) => (
            <>
              <div class="list-header">
                <strong>{f().name}</strong>
                <span>{f().total.toLocaleString()} messages</span>
              </div>
              <MessageList folder={f().id} onSelect={setMessage} handle={(h) => (list = h)} />
            </>
          )}
        </Show>
      </main>

      <section class="reading" aria-label="Reading pane">
        <Show when={report()}>
          <div class="report">
            <div class="report-head">
              <strong>Shell test result</strong>
              <button onClick={copyReport}>{copied() ? "Copied" : "Copy"}</button>
              <button onClick={() => setReport("")}>Close</button>
            </div>
            <pre>{report()}</pre>
          </div>
        </Show>
        <Show when={!report()}>
          <Show when={folder()}>{(f) => <Reader folder={f().id} row={message()} />}</Show>
        </Show>
      </section>

      <footer class="statusbar">
        <span>{status()}</span>
        <span class="statusbar-right">
          core {coreVersion()} · {core.name}
        </span>
      </footer>
    </div>
  );
}
