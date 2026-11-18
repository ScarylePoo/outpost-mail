// An open conversation in the message list: its messages as a tree, each
// reply under what it answers, with lines that show where the conversation
// forks.
//
// Every line has a fixed height, so the list knows how tall an open
// conversation is without measuring it (see `treeHeight`).

import { createMemo, For, Show } from "solid-js";
import type { ThreadMessage, ThreadView } from "../core";
import { firstName, treeOrder } from "../reader/Conversation";
import { formatDate } from "./format";

const LINE_HEIGHT = 46;
const GAP_HEIGHT = 22;
const PADDING = 6;
/** Replies nested deeper than this continue flat, with a marker. */
const MAX_DEPTH = 5;
const DAY = 86_400;

interface Line {
  message: ThreadMessage;
  depth: number;
  /** Deeper than the tree is drawn. */
  flattened: boolean;
  /** "3 days later", when a long silence came before this message. */
  gap: string;
}

function linesOf(thread: ThreadView): Line[] {
  const ordered = treeOrder(thread.messages);
  const depthOf = new Map<number, number>();
  let previous: ThreadMessage | undefined;
  return ordered.map((message) => {
    const parentDepth = message.parent !== null ? depthOf.get(message.parent) : undefined;
    const depth = parentDepth === undefined ? 0 : parentDepth + 1;
    depthOf.set(message.id, depth);
    const silence = previous ? message.date - previous.date : 0;
    const days = Math.round(silence / DAY);
    const gap = silence >= 2 * DAY ? (days >= 14 ? `${Math.round(days / 7)} weeks later` : `${days} days later`) : "";
    previous = message;
    return { message, depth: Math.min(depth, MAX_DEPTH), flattened: depth > MAX_DEPTH, gap };
  });
}

/** How tall the tree for this conversation is, in pixels. */
export function treeHeight(thread: ThreadView): number {
  const lines = linesOf(thread);
  return 2 * PADDING + lines.length * LINE_HEIGHT + lines.filter((l) => l.gap).length * GAP_HEIGHT;
}

export function ThreadTree(props: {
  thread: ThreadView;
  /** The message shown in the reading pane, if it is one of these. */
  selected: number | undefined;
  onSelect: (message: ThreadMessage) => void;
}) {
  const lines = createMemo(() => linesOf(props.thread));
  return (
    <div class="tree" style={{ height: `${treeHeight(props.thread)}px`, padding: `${PADDING}px 0` }}>
      <For each={lines()}>
        {(line) => (
          <>
            <Show when={line.gap}>
              <div class="tree-gap" style={{ height: `${GAP_HEIGHT}px` }}>
                <span />
                {line.gap}
                <span />
              </div>
            </Show>
            <div
              class="tree-line"
              classList={{ unread: line.message.unread, selected: line.message.id === props.selected }}
              style={{ height: `${LINE_HEIGHT}px` }}
              onMouseDown={(e) => {
                // The click is on this message, not on the conversation row.
                e.stopPropagation();
                props.onSelect(line.message);
              }}
            >
              <For each={Array.from({ length: line.depth })}>
                {(_, level) => (
                  <span
                    class="tree-guide"
                    classList={{
                      elbow: level() === line.depth - 1,
                      guessed: level() === line.depth - 1 && line.message.guessed,
                    }}
                  />
                )}
              </For>
              <div class="tree-body">
                <div class="tree-top">
                  <span class="row-dot" />
                  <Show when={line.flattened}>
                    <span class="tree-deeper" title="A reply further down this branch">
                      ↳
                    </span>
                  </Show>
                  <span class="tree-sender">
                    {line.message.mine ? "You" : firstName(line.message.senderName) || line.message.senderAddress}
                  </span>
                  <Show when={line.message.guessed}>
                    <span class="chip guess">Placed by subject</span>
                  </Show>
                  <Show when={line.message.added.length > 0}>
                    <span class="chip people">+ {line.message.added.map(firstName).join(", ")}</span>
                  </Show>
                  <Show when={line.message.dropped.length > 0}>
                    <span class="chip people">− {line.message.dropped.map(firstName).join(", ")}</span>
                  </Show>
                  <Show when={line.message.attachments.length > 0}>
                    <span class="tree-file" title={line.message.attachments.map((a) => a.name).join(", ")}>
                      📎 {line.message.attachments[0]!.name}
                    </span>
                  </Show>
                  <span class="row-date">{formatDate(line.message.date)}</span>
                </div>
                <div class="tree-preview">{line.message.text.replace(/\s+/g, " ").slice(0, 200)}</div>
              </div>
            </div>
          </>
        )}
      </For>
    </div>
  );
}
