// A meeting invitation inside a message: what, when, where and from whom,
// with Accept, Maybe and Decline. Answering puts the event in a calendar
// and mails the answer to the organiser.

import { createSignal, For, Show } from "solid-js";
import { core, type FolderId, type InvitationView } from "../core";
import { dayOfDate, dayOfMoment, DAY, formatDay, formatTime, sameDay } from "../organizer/dates";
import { findPlace, placeKey, places } from "../organizer/store";

function when(invitation: InvitationView): string {
  const e = invitation.event;
  if (e.allDay) {
    const first = dayOfDate(e.start);
    const last = dayOfDate(Math.max(e.start, e.end - DAY));
    return sameDay(first, last) ? `${formatDay(first)}, all day` : `${formatDay(first)} to ${formatDay(last)}`;
  }
  const [first, last] = [dayOfMoment(e.start), dayOfMoment(e.end)];
  return sameDay(first, last)
    ? `${formatDay(first)}, ${formatTime(e.start)} to ${formatTime(e.end)}`
    : `${formatDay(first)} ${formatTime(e.start)} to ${formatDay(last)} ${formatTime(e.end)}`;
}

export function InvitationCard(props: {
  folder: FolderId;
  message: number;
  invitation: InvitationView;
  /** The message failed its sender checks: it cannot be answered. */
  locked: boolean;
  onChanged: () => void;
}) {
  const calendars = () => places("events");
  const [place, setPlace] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [note, setNote] = createSignal("");
  const chosen = () => findPlace(calendars(), place()) ?? calendars()[0];
  const organizer = () => props.invitation.organizerName || props.invitation.organizer;

  function answer(answer: "accepted" | "tentative" | "declined" | "remove") {
    const where = chosen();
    if (!where) return setNote("Add a calendar account in Settings first, so there is a calendar to put it in.");
    setBusy(true);
    setNote("");
    core
      .send({ type: "answerInvitation", folder: props.folder, id: props.message, answer, account: where.account, collection: where.collection.id })
      .then(() => {
        setNote(
          answer === "remove"
            ? "Removed from your calendar."
            : answer === "declined"
              ? `Declined. ${organizer()} is being told.`
              : `Added to ${where.collection.name}. ${organizer()} is being told.`,
        );
        props.onChanged();
      })
      .catch((e) => setNote(String(e?.message ?? e)))
      .finally(() => setBusy(false));
  }

  return (
    <div class="invitation" classList={{ cancelled: props.invitation.kind === "cancel" }}>
      <div class="invitation-kind">{props.invitation.kind === "cancel" ? "This meeting was cancelled" : "Invitation"}</div>
      <strong class="invitation-title">{props.invitation.event.summary || "(no title)"}</strong>
      <div>
        {when(props.invitation)}
        <Show when={props.invitation.recurring}> · repeats</Show>
      </div>
      <Show when={props.invitation.event.location}>
        <div>{props.invitation.event.location}</div>
      </Show>
      <Show when={organizer()}>
        <div class="invitation-from">
          Organised by {organizer()}
          <Show when={props.invitation.organizerName && props.invitation.organizer}> ({props.invitation.organizer})</Show>
        </div>
      </Show>
      <Show
        when={!props.locked}
        fallback={<div class="invitation-from">It cannot be answered because this message failed its sender checks.</div>}
      >
        <Show
          when={props.invitation.kind === "request"}
          fallback={
            <Show when={props.invitation.inCalendar}>
              <div class="invitation-actions">
                <button disabled={busy()} onClick={() => answer("remove")}>
                  Remove from my calendar
                </button>
              </div>
            </Show>
          }
        >
          <div class="invitation-actions">
            <button class="primary" disabled={busy()} onClick={() => answer("accepted")}>
              Accept
            </button>
            <button disabled={busy()} onClick={() => answer("tentative")}>
              Maybe
            </button>
            <button disabled={busy()} onClick={() => answer("declined")}>
              Decline
            </button>
            <Show when={calendars().length > 1}>
              <select aria-label="Calendar to put it in" value={place() || (chosen() ? placeKey(chosen()!) : "")} onChange={(e) => setPlace(e.currentTarget.value)}>
                <For each={calendars()}>{(p) => <option value={placeKey(p)}>{p.label}</option>}</For>
              </select>
            </Show>
            <Show when={props.invitation.inCalendar}>
              <span class="invitation-from">Already in your calendar.</span>
            </Show>
          </div>
        </Show>
      </Show>
      <Show when={note()}>
        <div class="invitation-from" role="status">
          {note()}
        </div>
      </Show>
    </div>
  );
}
