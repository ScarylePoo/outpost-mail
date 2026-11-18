// One message in full, exactly as it was sent: the "original".
//
// HTML mail is hostile input. The core has already reduced it to formatting
// (render.rs). Here it is additionally shown inside a sandboxed frame that
// cannot run scripts, submit forms or open windows, under a content policy
// that allows no network access at all. Links do not navigate: the pane
// shows where a link really goes first.

import { createEffect, createMemo, createSignal, For, on, onCleanup, Show } from "solid-js";
import { core, type FolderId, type MessageView } from "../core";

const FRAME_POLICY = "default-src 'none'; img-src data:; style-src 'unsafe-inline'";

const FRAME_STYLE = `
  html { color-scheme: light; }
  body {
    margin: 0; padding: 0 0 8px;
    font: 14px/1.6 system-ui, -apple-system, "Segoe UI", sans-serif;
    color: #1c1f23; background: #fff;
    overflow-wrap: anywhere;
  }
  img, table { max-width: 100%; }
  img { height: auto; }
  a { color: #1f5fbf; }
  pre { white-space: pre-wrap; }
  blockquote { margin: 8px 0; padding-left: 12px; border-left: 3px solid #dcdfe4; color: #5b626c; }
`;

function frameDocument(html: string): string {
  return (
    `<!doctype html><html><head><meta charset="utf-8">` +
    `<meta http-equiv="Content-Security-Policy" content="${FRAME_POLICY}">` +
    `<style>${FRAME_STYLE}</style></head><body>${html}</body></html>`
  );
}

export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} kB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/** Attachment names with a Save button each, and where a saved one went. */
export function Attachments(props: {
  folder: FolderId;
  id: number;
  attachments: { name: string; mimeType: string; size: number }[];
}) {
  const [saved, setSaved] = createSignal<{ path: string; name: string }>();
  const [error, setError] = createSignal("");

  function save(index: number) {
    setError("");
    core
      .send({ type: "saveAttachment", folder: props.folder, id: props.id, index })
      .then((r) => setSaved({ path: r.path, name: r.name }))
      .catch((e) => setError(String(e?.message ?? e)));
  }

  return (
    <Show when={props.attachments.length > 0}>
      <ul class="attachments">
        <For each={props.attachments}>
          {(a, index) => (
            <li title={a.mimeType}>
              <span class="attachment-name">{a.name}</span>
              <span class="attachment-size">{formatSize(a.size)}</span>
              <button onClick={() => save(index())}>Save</button>
            </li>
          )}
        </For>
      </ul>
      <Show when={saved()}>
        {(s) => (
          <div class="message-note">
            Saved to your Downloads folder as <strong>{s().name}</strong>.{" "}
            <button class="inline" onClick={() => void core.revealDownload(s().path)}>
              Show in folder
            </button>
          </div>
        )}
      </Show>
      <Show when={error()}>
        <div class="message-note message-error">Could not save the attachment: {error()}</div>
      </Show>
    </Show>
  );
}

export function MessageBody(props: {
  folder: FolderId;
  id: number;
  /** Leave the attachments out (the caller shows them itself). */
  bare?: boolean;
  onLoaded?: (view: MessageView) => void;
}) {
  const [view, setView] = createSignal<MessageView>();
  const [error, setError] = createSignal("");
  const [link, setLink] = createSignal("");
  const [copied, setCopied] = createSignal(false);
  const shown = createMemo(() => props.id);
  let request = 0;

  createEffect(
    on(shown, (id) => {
      const mine = ++request;
      setView(undefined);
      setError("");
      setLink("");
      core
        .send({ type: "openMessage", folder: props.folder, id })
        .then((r) => {
          if (mine !== request) return;
          setView(r.message);
          props.onLoaded?.(r.message);
        })
        .catch((e) => mine === request && setError(String(e?.message ?? e)));
    }),
  );

  function watchFrame(frame: HTMLIFrameElement) {
    const doc = frame.contentDocument;
    if (!doc) return;
    const fit = () => {
      frame.style.height = `${doc.documentElement.scrollHeight}px`;
    };
    fit();
    const observer = new ResizeObserver(fit);
    observer.observe(doc.documentElement);
    onCleanup(() => observer.disconnect());

    const linkAt = (target: EventTarget | null) =>
      (target as Element | null)?.closest?.("a[href]")?.getAttribute("href") ?? "";
    // A click on a link never navigates. It shows the real destination.
    doc.addEventListener("click", (e) => {
      const href = linkAt(e.target);
      if (href) {
        e.preventDefault();
        setCopied(false);
        setLink(href);
      }
    });
    doc.addEventListener("auxclick", (e) => linkAt(e.target) && e.preventDefault());
  }

  async function copyLink() {
    await navigator.clipboard.writeText(link());
    setCopied(true);
  }

  return (
    <>
      <Show when={error()}>
        <div class="message-note message-error">Could not open this message: {error()}</div>
      </Show>
      <Show when={view()}>
        {(v) => (
          <Show
            when={v().downloaded}
            fallback={
              <div class="message-note">
                This message has not been downloaded yet. It will appear here when sync reaches it.
              </div>
            }
          >
            <Show when={v().remoteContentBlocked}>
              <div class="message-note">Pictures from the internet were not loaded.</div>
            </Show>
            <Show when={!props.bare}>
              <Attachments folder={props.folder} id={props.id} attachments={v().attachments} />
            </Show>
            <Show when={v().html !== null} fallback={<pre class="message-text">{v().text ?? ""}</pre>}>
              <iframe
                class="message-frame"
                title="Message"
                sandbox="allow-same-origin"
                srcdoc={frameDocument(v().html!)}
                onLoad={(e) => watchFrame(e.currentTarget)}
              />
            </Show>
          </Show>
        )}
      </Show>
      <Show when={link()}>
        <div class="link-bar" role="status">
          <span class="link-label">This link goes to</span>
          <span class="link-target">{link()}</span>
          <Show when={/^(https?:|mailto:)/i.test(link().trim())}>
            <button
              onClick={() => core.openLink(link()).catch((e) => setError(`Could not open the link: ${e?.message ?? e}`))}
            >
              Open
            </button>
          </Show>
          <button onClick={copyLink}>{copied() ? "Copied" : "Copy"}</button>
          <button onClick={() => setLink("")}>Dismiss</button>
        </div>
      </Show>
    </>
  );
}
