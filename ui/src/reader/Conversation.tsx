// A conversation in the reading pane: a stack of its messages, each reduced
// to what its sender wrote. Quoted history and signatures are collapsed
// behind links that say how much is hidden, and "Show original" gives any
// message back exactly as it was sent.
//
// Three ways to read it, one click each:
//   This branch   the messages leading to the selected one, and the replies
//                 to it (the default)
//   Whole thread  everything, replies under what they answer
//   By date       everything, oldest first

import { createEffect, createMemo, createSignal, For, on, Show } from "solid-js";
import type { FolderId, ThreadMessage, ThreadView } from "../core";
import { formatDate, formatLongDate } from "../list/format";
import { Attachments, MessageBody } from "./MessageBody";

type Mode = "branch" | "thread" | "date";

const DAY = 86_400;

function firstName(name: string): string {
  return name.trim().split(/\s+/)[0] ?? name;
}

/** "Ann Lee <ann@x.com>, bob@y.org" as "Ann, bob": who, without the detail. */
function shortNames(recipients: string): string {
  const names = recipients
    .split(/,(?![^<]*>)/)
    .map((part) => part.trim())
    .filter(Boolean)
    .map((part) => {
      const name = part.replace(/<[^>]*>/, "").replace(/"/g, "").trim();
      return name ? firstName(name) : (part.replace(/[<>]/g, "").split("@")[0] ?? part);
    });
  return names.length > 6 ? `${names.slice(0, 5).join(", ")} and ${names.length - 5} more` : names.join(", ");
}

/** Everything in tree order: each message followed by its replies. */
function treeOrder(messages: ThreadMessage[]): ThreadMessage[] {
  const present = new Set(messages.map((m) => m.id));
  const children = new Map<number | null, ThreadMessage[]>();
  for (const m of messages) {
    const parent = m.parent !== null && present.has(m.parent) ? m.parent : null;
    children.set(parent, [...(children.get(parent) ?? []), m]);
  }
  const out: ThreadMessage[] = [];
  const visit = (parent: number | null) => {
    for (const m of children.get(parent) ?? []) {
      out.push(m);
      visit(m.id);
    }
  };
  visit(null);
  return out;
}

/** The messages leading to `selected`, then it, then the replies below it. */
function branchOf(messages: ThreadMessage[], selected: number): ThreadMessage[] {
  const byId = new Map(messages.map((m) => [m.id, m]));
  const up: ThreadMessage[] = [];
  let at = byId.get(selected);
  const seen = new Set<number>();
  while (at && !seen.has(at.id)) {
    seen.add(at.id);
    up.unshift(at);
    at = at.parent !== null ? byId.get(at.parent) : undefined;
  }
  const below = treeOrder(messages);
  const start = below.findIndex((m) => m.id === selected);
  const down: ThreadMessage[] = [];
  if (start >= 0) {
    const inside = new Set([selected]);
    for (const m of below.slice(start + 1)) {
      if (m.parent !== null && inside.has(m.parent)) {
        inside.add(m.id);
        down.push(m);
      }
    }
  }
  return [...up, ...down];
}

function Message(props: {
  message: ThreadMessage;
  all: ThreadMessage[];
  current: boolean;
  listFolder: FolderId;
}) {
  const m = () => props.message;
  const [quoted, setQuoted] = createSignal(false);
  const [signature, setSignature] = createSignal(false);
  const [original, setOriginal] = createSignal(false);
  const parent = createMemo(() => props.all.find((x) => x.id === m().parent));
  const context = createMemo(() => {
    const p = parent();
    if (!p) return m().recipients ? `to ${shortNames(m().recipients)}` : "";
    if (p.senderAddress === m().senderAddress) return "replying to their own earlier message";
    return `replying to ${firstName(p.senderName)}`;
  });
  let element!: HTMLElement;

  // Bring the selected message into view when the conversation opens.
  createEffect(
    on(
      () => props.current,
      (current) => current && queueMicrotask(() => element.scrollIntoView({ block: "nearest" })),
    ),
  );

  return (
    <article class="turn" classList={{ current: props.current, unread: m().unread }} ref={element}>
      <header class="turn-head">
        <span class="turn-who">
          <strong>{m().senderName}</strong>
          <Show when={m().senderAddress && m().senderAddress !== m().senderName}>
            <span class="turn-address">{m().senderAddress}</span>
          </Show>
          <span class="turn-context">{context()}</span>
        </span>
        <span class="turn-date" title={formatLongDate(m().date)}>
          {formatDate(m().date)}
        </span>
      </header>
      <Show when={m().guessed || m().added.length > 0 || m().dropped.length > 0 || m().folder !== props.listFolder}>
        <div class="turn-chips">
          <Show when={m().guessed}>
            <span class="chip guess">Placed by subject, no reply header</span>
          </Show>
          <Show when={m().added.length > 0}>
            <span class="chip people">+ {m().added.join(", ")} added</span>
          </Show>
          <Show when={m().dropped.length > 0}>
            <span class="chip people">− {m().dropped.join(", ")} dropped</span>
          </Show>
          <Show when={m().folder !== props.listFolder && m().folderName}>
            <span class="chip place">in {m().folderName}</span>
          </Show>
        </div>
      </Show>

      <Show
        when={!original()}
        fallback={<MessageBody folder={m().folder} id={m().id} bare />}
      >
        <Show when={m().text} fallback={<p class="turn-empty">{m().downloaded ? "(nothing new written)" : ""}</p>}>
          <div class="turn-text">{m().text}</div>
        </Show>
        <Show when={!m().downloaded}>
          <div class="message-note">Only the start of this message has been downloaded so far.</div>
        </Show>
        <Show when={quoted()}>
          <pre class="turn-hidden">{m().quoted}</pre>
        </Show>
        <Show when={signature()}>
          <pre class="turn-hidden">{m().signature}</pre>
        </Show>
      </Show>

      <Attachments folder={m().folder} id={m().id} attachments={m().attachments} />

      <Show when={m().downloaded}>
        <div class="turn-links">
          <Show when={m().quotedLines > 0 && !original()}>
            <button onClick={() => setQuoted((v) => !v)}>
              {quoted() ? "Hide" : "Show"} quoted text ({m().quotedLines} line{m().quotedLines === 1 ? "" : "s"})
            </button>
          </Show>
          <Show when={m().signature && !original()}>
            <button onClick={() => setSignature((v) => !v)}>{signature() ? "Hide" : "Show"} signature</button>
          </Show>
          <button onClick={() => setOriginal((v) => !v)}>{original() ? "Show reduced" : "Show original"}</button>
        </div>
      </Show>
    </article>
  );
}

export function Conversation(props: { thread: ThreadView; selected: number; folder: FolderId }) {
  const [mode, setMode] = createSignal<Mode>("branch");
  const all = () => props.thread.messages;

  const shown = createMemo(() => {
    switch (mode()) {
      case "branch":
        return branchOf(all(), props.selected);
      case "thread":
        return treeOrder(all());
      case "date":
        return [...all()].sort((a, b) => a.date - b.date);
    }
  });

  const summary = createMemo(() => {
    const dates = all().map((m) => m.date);
    const [from, to] = [Math.min(...dates), Math.max(...dates)];
    const names = [...new Set(all().map((m) => firstName(m.senderName)))];
    const span = to - from < DAY ? formatDate(to) : `${formatDate(from)} to ${formatDate(to)}`;
    return `${span} · ${names.join(", ")}`;
  });

  const note = createMemo(() => {
    const n = all().length;
    const hidden = "Quoted text and signatures are hidden.";
    if (mode() === "branch" && shown().length < n) {
      return `Showing the branch you selected: ${shown().length} of ${n} messages. Whole thread shows all ${n}. ${hidden}`;
    }
    return `Showing all ${n} messages${mode() === "date" ? ", oldest first" : ""}. ${hidden}`;
  });

  /** "3 days later", when a long silence separates two messages. */
  const gapBefore = (index: number) => {
    if (index === 0) return "";
    const gap = shown()[index]!.date - shown()[index - 1]!.date;
    if (gap < 2 * DAY) return "";
    const days = Math.round(gap / DAY);
    return days >= 14 ? `${Math.round(days / 7)} weeks later` : `${days} days later`;
  };

  return (
    <div class="conversation">
      <header class="conversation-head">
        <div class="conversation-title">
          <h1>{props.thread.subject || "(no subject)"}</h1>
          <div class="conversation-meta">{summary()}</div>
        </div>
        <div class="modes" role="group" aria-label="How to show the conversation">
          <button classList={{ on: mode() === "branch" }} onClick={() => setMode("branch")}>
            This branch
          </button>
          <button classList={{ on: mode() === "thread" }} onClick={() => setMode("thread")}>
            Whole thread
          </button>
          <button classList={{ on: mode() === "date" }} onClick={() => setMode("date")}>
            By date
          </button>
        </div>
      </header>
      <p class="conversation-note">{note()}</p>
      <For each={shown()}>
        {(message, index) => (
          <>
            <Show when={gapBefore(index())}>
              <div class="gap">
                <span />
                {gapBefore(index())}
                <span />
              </div>
            </Show>
            <Message message={message} all={all()} current={message.id === props.selected} listFolder={props.folder} />
          </>
        )}
      </For>
    </div>
  );
}
