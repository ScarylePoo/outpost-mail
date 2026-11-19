// The reading pane. A message that is part of a conversation is shown as
// that conversation (Conversation.tsx); a message on its own is shown in
// full, as it was sent (MessageBody.tsx).
//
// The header is drawn at once from the list row, so selecting a message
// responds immediately; the rest follows when the core has answered.

import { createEffect, createMemo, createSignal, on, Show } from "solid-js";
import { core, type FolderId, type MessageRow, type MessageView, type ThreadView } from "../core";
import { formatLongDate } from "../list/format";
import { Conversation } from "./Conversation";
import { MessageBody } from "./MessageBody";
import { subjectsVersion } from "../core/protection";

export function Reader(props: {
  folder: FolderId;
  row: MessageRow | undefined;
  /** Called once a message has been opened and shown. */
  onOpened?: (row: MessageRow) => void;
}) {
  const [thread, setThread] = createSignal<ThreadView>();
  const [view, setView] = createSignal<MessageView>();
  const [error, setError] = createSignal("");
  let request = 0;

  // Which message is shown. A memo, so that the row being refreshed (its
  // flags changing, say) does not count as a different message: only a
  // different id reopens, and only opening marks a message read.
  const shown = createMemo(() => props.row?.id);

  createEffect(
    on([shown, subjectsVersion], ([id]) => {
      const mine = ++request;
      setThread(undefined);
      setView(undefined);
      setError("");
      if (id === undefined) return;
      core
        .send({ type: "openThread", folder: props.folder, id })
        .then((r) => {
          if (mine !== request) return;
          setThread(r.thread);
          // A conversation is "opened" as soon as it is on screen. A single
          // message is opened when its body has loaded (see below).
          if (r.thread.messages.length > 1 && props.row) props.onOpened?.(props.row);
        })
        .catch((e) => mine === request && setError(String(e?.message ?? e)));
    }),
  );

  /** The subject as the sender wrote it, once the message has been read. */
  const sendersSubject = () => {
    const v = view();
    return v?.downloaded ? (v.originalSubject ?? v.subject) : undefined;
  };

  const conversation = createMemo(() => {
    const t = thread();
    return t && t.messages.length > 1 ? t : undefined;
  });

  return (
    <Show when={props.row} fallback={<div class="reading-empty">Select a message to read it.</div>}>
      {(row) => (
        <Show
          when={conversation()}
          fallback={
            <article class="message">
              <header class="message-head">
                <h1>{row().subject || view()?.subject || "(no subject)"}</h1>
                <Show when={sendersSubject() !== undefined && sendersSubject() !== row().subject}>
                  <div class="message-renamed">The sender's subject: {sendersSubject() || "(no subject)"}</div>
                </Show>
                <div class="message-from">
                  <strong>{row().senderName}</strong>
                  <Show when={row().senderAddress && row().senderAddress !== row().senderName}>
                    <span> &lt;{row().senderAddress}&gt;</span>
                  </Show>
                </div>
                <Show when={view()?.to}>
                  <div class="message-to">To: {view()!.to}</div>
                </Show>
                <Show when={view()?.cc}>
                  <div class="message-to">Cc: {view()!.cc}</div>
                </Show>
                <div class="message-date">{formatLongDate(row().date)}</div>
              </header>
              <Show when={error()}>
                <div class="message-note message-error">Could not open this message: {error()}</div>
              </Show>
              <Show when={thread()}>
                <MessageBody
                  folder={props.folder}
                  id={row().id}
                  onLoaded={(v) => {
                    setView(v);
                    if (v.downloaded && props.row) props.onOpened?.(props.row);
                  }}
                />
              </Show>
            </article>
          }
        >
          {(t) => <Conversation thread={t()} selected={row().id} folder={props.folder} />}
        </Show>
      )}
    </Show>
  );
}
