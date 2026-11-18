// Search results: the newest matches, in the same row design as the list.
// The core returns them already in order, a couple of hundred at most, so
// this is a plain list with no virtualisation.

import { createEffect, createSignal, For, on, onCleanup, Show } from "solid-js";
import { core, type FolderId, type MessageRow } from "../core";
import { formatDate } from "./format";

const LIMIT = 200;

export function SearchResults(props: {
  /** A folder's id to search that folder, or an account's id for all of it. */
  scope: FolderId;
  text: string;
  onSelect: (row: MessageRow | undefined) => void;
}) {
  const [rows, setRows] = createSignal<MessageRow[]>([]);
  const [state, setState] = createSignal<"searching" | "done" | "error">("searching");
  const [error, setError] = createSignal("");
  const [selected, setSelected] = createSignal<number>();
  let request = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;

  createEffect(
    on(
      () => [props.scope, props.text] as const,
      ([scope, text]) => {
        const mine = ++request;
        clearTimeout(timer);
        setState("searching");
        // A short pause, so that typing a word is one search, not six.
        timer = setTimeout(() => {
          core
            .send({ type: "search", scope, text, limit: LIMIT })
            .then((r) => {
              if (mine !== request) return;
              setRows(r.rows);
              setState("done");
            })
            .catch((e) => {
              if (mine !== request) return;
              setError(String(e?.message ?? e));
              setState("error");
            });
        }, 120);
      },
    ),
  );
  onCleanup(() => clearTimeout(timer));

  return (
    <div class="list results" role="listbox" aria-label="Search results" tabindex="0">
      <For each={rows()}>
        {(row) => (
          <div
            class="row result"
            role="option"
            aria-selected={selected() === row.id}
            classList={{ selected: selected() === row.id, unread: row.unread }}
            onMouseDown={() => {
              setSelected(row.id);
              props.onSelect(row);
            }}
          >
            <div class="row-top">
              <span class="row-dot" />
              <span class="row-sender">{row.senderName}</span>
              <Show when={row.hasAttachment}>
                <span class="row-clip" title="Has attachment">
                  📎
                </span>
              </Show>
              <span class="row-date">{formatDate(row.date)}</span>
            </div>
            <div class="row-subject">{row.subject}</div>
            <div class="row-preview result-preview">{row.preview}</div>
          </div>
        )}
      </For>
      <Show when={state() === "done" && rows().length === 0}>
        <div class="list-note">Nothing found.</div>
      </Show>
      <Show when={state() === "done" && rows().length === LIMIT}>
        <div class="results-more">Showing the newest {LIMIT} matches. Add a word to narrow it down.</div>
      </Show>
      <Show when={state() === "error"}>
        <div class="list-note list-error">Search failed: {error()}</div>
      </Show>
    </div>
  );
}
