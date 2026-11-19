// Links in a message: one way of following them, wherever they appear.
//
// A formatted message has real links (MessageBody.tsx catches the clicks in
// its frame). Plain text has only addresses written out; `LinkedText` finds
// them and makes them work the same way. A link never just navigates: with
// "show where a link goes" on, the destination is shown first.

import { createMemo, createSignal, For, Show } from "solid-js";
import { core } from "../core";
import { protection } from "../core/protection";

/** Only these are ever handed to the system to open. */
export const opens = (href: string) => /^(https?:|mailto:)/i.test(href.trim());

export interface Links {
  /** The destination on show, or "". */
  shown: () => string;
  error: () => string;
  /** Somebody clicked a link to `href`. */
  follow: (href: string) => void;
  open: (href: string) => void;
  dismiss: () => void;
}

export function createLinks(): Links {
  const [shown, setShown] = createSignal("");
  const [error, setError] = createSignal("");
  const open = (href: string) => {
    setError("");
    core.openLink(href).catch((e) => setError(`Could not open the link: ${e?.message ?? e}`));
  };
  return {
    shown,
    error,
    open,
    follow(href) {
      if (!protection().showLinkDestination && opens(href)) open(href);
      else setShown(href);
    },
    dismiss: () => setShown(""),
  };
}

/** "This link goes to …", with Open, Copy and Dismiss. */
export function LinkBar(props: { links: Links }) {
  const [copied, setCopied] = createSignal("");
  return (
    <>
      <Show when={props.links.error()}>
        <div class="message-note message-error">{props.links.error()}</div>
      </Show>
      <Show when={props.links.shown()}>
        {(href) => (
          <div class="link-bar" role="status">
            <span class="link-label">This link goes to</span>
            <span class="link-target">{href()}</span>
            <Show when={opens(href())}>
              <button onClick={() => props.links.open(href())}>Open</button>
            </Show>
            <button onClick={() => void navigator.clipboard.writeText(href()).then(() => setCopied(href()))}>
              {copied() === href() ? "Copied" : "Copy"}
            </button>
            <button onClick={props.links.dismiss}>Dismiss</button>
          </div>
        )}
      </Show>
    </>
  );
}

// An address in running text: http(s)://…, www.…, or an email address.
const ADDRESS = /\b(?:https?:\/\/|www\.)[^\s<>"'`]+|\b[\w.+-]+@[\w-]+(?:\.[\w-]+)+/gi;

export interface Piece {
  text: string;
  /** Where it goes, when the piece is an address. */
  href?: string;
}

/** Splits text into plain pieces and addresses. */
export function findLinks(text: string): Piece[] {
  const pieces: Piece[] = [];
  let at = 0;
  for (const match of text.matchAll(ADDRESS)) {
    let found = match[0];
    // Punctuation that ends the sentence, not the address. A closing bracket
    // belongs to the address only if it opened inside it.
    for (;;) {
      const last = found[found.length - 1]!;
      const pair = { ")": "(", "]": "[", "}": "{" }[last];
      if (".,;:!?'\"*_".includes(last) || (pair && !found.includes(pair))) found = found.slice(0, -1);
      else break;
    }
    if (found.length < 4) continue;
    const start = match.index!;
    if (start > at) pieces.push({ text: text.slice(at, start) });
    const href = /^https?:/i.test(found) ? found : found.includes("@") && !/^www\./i.test(found) ? `mailto:${found}` : `https://${found}`;
    pieces.push({ text: found, href });
    at = start + found.length;
  }
  if (at < text.length) pieces.push({ text: text.slice(at) });
  return pieces;
}

/** Plain text with the addresses in it made into links. */
export function LinkedText(props: { text: string; links: Links }) {
  const pieces = createMemo(() => findLinks(props.text));
  return (
    <For each={pieces()}>
      {(piece) =>
        piece.href ? (
          <a
            class="text-link"
            href={piece.href}
            title={piece.href}
            onClick={(e) => {
              e.preventDefault();
              props.links.follow(piece.href!);
            }}
            onAuxClick={(e) => e.preventDefault()}
          >
            {piece.text}
          </a>
        ) : (
          piece.text
        )
      }
    </For>
  );
}
