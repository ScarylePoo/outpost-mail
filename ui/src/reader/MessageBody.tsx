// One message in full, exactly as it was sent: the "original".
//
// HTML mail is hostile input. The core has already reduced it to formatting
// (render.rs). Here it is additionally shown inside a sandboxed frame that
// cannot run scripts, submit forms or open windows, under a content policy
// that allows no network access at all (pictures excepted, for senders the
// user has allowed). Links do not navigate: the pane shows where a link
// really goes first.

import { createEffect, createMemo, createSignal, For, on, onCleanup, Show } from "solid-js";
import { core, type FolderId, type MessageView } from "../core";
import { protection, protectionVersion, saveProtection } from "../core/protection";
import { appearance, readingFamily } from "../core/appearance";
import { SenderCheckPanel } from "./SenderCheck";
import { InvitationCard } from "./InvitationCard";
import { createLinks, LinkBar, LinkedText } from "./links";

/** Nothing from the network, unless this sender's pictures are allowed. */
const framePolicy = (pictures: boolean) =>
  `default-src 'none'; img-src data:${pictures ? " https: http:" : ""}; style-src 'unsafe-inline'`;

// A formatted message is always shown as its sender laid it out: on a light
// page, whatever the app's colour scheme. Mail sets its own colours piecemeal
// (dark text with no background is the commonest case) and recolouring it
// reliably is not possible. The reading font and size are the default for
// text the message does not style itself.
const frameStyle = (dark: boolean) => `
  html { color-scheme: light; background: #fff; }
  body {
    margin: 0; padding: ${dark ? "12px 14px" : "0 0 8px"};
    font: ${appearance().readingSize}px/1.6 ${readingFamily()};
    color: #1c1f23; background: #fff;
    overflow-wrap: anywhere;
  }
  img, table { max-width: 100%; }
  img { height: auto; }
  a { color: #1f5fbf; }
  pre { white-space: pre-wrap; }
  blockquote { margin: 8px 0; padding-left: 12px; border-left: 3px solid #dcdfe4; color: #5b626c; }
`;

function frameDocument(html: string, pictures: boolean, dimmed: boolean): string {
  const dark = document.documentElement.dataset.dark === "true";
  return (
    `<!doctype html><html><head><meta charset="utf-8">` +
    `<meta http-equiv="Content-Security-Policy" content="${framePolicy(pictures)}">` +
    `<meta name="referrer" content="no-referrer">` +
    `<style>${frameStyle(dark)}${dimmed ? "body { opacity: 0.55; }" : ""}</style></head><body>${html}</body></html>`
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
  /** The message failed its sender checks: nothing can be saved. */
  locked?: boolean;
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
              <Show when={!props.locked}>
                <button onClick={() => save(index())}>Save</button>
              </Show>
            </li>
          )}
        </For>
      </ul>
      <Show when={props.locked}>
        <div class="message-note">Attachments are switched off because this message failed its sender checks.</div>
      </Show>
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
  const links = createLinks();
  const shown = createMemo(() => props.id);
  /** Bumped to read the same message again (after trusting it, say). */
  const [again, setAgain] = createSignal(0);
  let request = 0;
  let last: number | undefined;

  createEffect(
    on([shown, again, protectionVersion], ([id]) => {
      const mine = ++request;
      // The same message read again keeps what is on screen until the new
      // version arrives.
      if (id !== last) setView(undefined);
      last = id;
      setError("");
      setSpots([]);
      links.dismiss();
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

  /** Where each link of the formatted message lies, in the frame's own pixels. */
  const [spots, setSpots] = createSignal<{ href: string; x: number; y: number; w: number; h: number }[]>([]);

  // The frame cannot run anything, and that includes a click handler put on
  // it from out here: the browser refuses to call it. So links are caught
  // outside the frame instead. Every link's position is read from the frame
  // and an invisible link of the same size is laid over it; that one is
  // ours, and shows where the link goes. The frame is as tall as its
  // content and never scrolls, so positions inside it are positions on it.
  function watchFrame(frame: HTMLIFrameElement) {
    const doc = frame.contentDocument;
    if (!doc) return;
    const fit = () => {
      frame.style.height = `${doc.documentElement.scrollHeight}px`;
      const found: ReturnType<typeof spots> = [];
      for (const link of doc.querySelectorAll("a[href]")) {
        const href = link.getAttribute("href") ?? "";
        // A link around a picture or a table has no box of its own.
        for (const part of [link, ...link.querySelectorAll("*")]) {
          for (const r of part.getClientRects()) {
            if (r.width > 0 && r.height > 0 && found.length < 4000) found.push({ href, x: r.left, y: r.top, w: r.width, h: r.height });
          }
        }
      }
      setSpots(found);
    };
    fit();
    const observer = new ResizeObserver(fit);
    observer.observe(doc.documentElement);
    // A picture arriving can move links without changing the page's height.
    const later = [300, 1500, 5000].map((ms) => setTimeout(fit, ms));
    onCleanup(() => {
      observer.disconnect();
      later.forEach(clearTimeout);
    });
  }

  async function allowPictures(address: string) {
    const p = protection();
    const sender = address.toLowerCase();
    try {
      await saveProtection(`To always load pictures from ${sender}`, {
        ...p,
        allowImagesFrom: [...p.allowImagesFrom.filter((a) => a !== sender), sender],
      });
    } catch (e) {
      setError(String((e as Error)?.message ?? e));
    }
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
            <SenderCheckPanel folder={props.folder} view={v()} onChanged={() => setAgain((n) => n + 1)} />
            <Show when={v().invitation}>
              {(invitation) => (
                <InvitationCard
                  folder={props.folder}
                  message={v().id}
                  invitation={invitation()}
                  locked={v().restricted}
                  onChanged={() => setAgain((n) => n + 1)}
                />
              )}
            </Show>
            <Show when={v().remoteContentBlocked}>
              <div class="message-note">
                Pictures from the internet were not loaded.
                <Show when={!v().restricted && v().senderAddress}>
                  {" "}
                  <button class="inline" onClick={() => void allowPictures(v().senderAddress)}>
                    Always load pictures from {v().senderAddress}
                  </button>
                </Show>
              </div>
            </Show>
            <Show when={!props.bare}>
              <Attachments folder={props.folder} id={props.id} attachments={v().attachments} locked={v().restricted} />
            </Show>
            <Show
              when={v().html !== null}
              fallback={
                <pre class="message-text" classList={{ dimmed: v().restricted }}>
                  <Show when={!v().restricted} fallback={v().text ?? ""}>
                    <LinkedText text={v().text ?? ""} links={links} />
                  </Show>
                </pre>
              }
            >
              <div class="message-frame-holder">
                <iframe
                  class="message-frame"
                  title="Message"
                  sandbox="allow-same-origin"
                  srcdoc={frameDocument(v().html!, v().remoteImagesShown, v().restricted)}
                  onLoad={(e) => watchFrame(e.currentTarget)}
                />
                <For each={spots()}>
                  {(spot) => (
                    <a
                      class="frame-link"
                      href={spot.href}
                      title={spot.href}
                      draggable={false}
                      style={{ left: `${spot.x}px`, top: `${spot.y}px`, width: `${spot.w}px`, height: `${spot.h}px` }}
                      onClick={(e) => {
                        e.preventDefault();
                        links.follow(spot.href);
                      }}
                      onAuxClick={(e) => e.preventDefault()}
                    />
                  )}
                </For>
              </div>
            </Show>
          </Show>
        )}
      </Show>
      <LinkBar links={links} />
    </>
  );
}
