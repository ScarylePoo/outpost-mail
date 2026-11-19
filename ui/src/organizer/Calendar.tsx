// The calendar: a month of days with every calendar's events laid over one
// another in its colour, and a panel to make or change an event.

import { createEffect, createMemo, createSignal, For, on, Show } from "solid-js";
import { core, type EventContent, type EventRow } from "../core";
import {
  addDays,
  DAY,
  dateValue,
  dayNumber,
  dayOfDate,
  dayOfMoment,
  formatDay,
  formatMonth,
  formatTime,
  fromInput,
  moment,
  monthGrid,
  sameDay,
  startOfDay,
  timeInput,
  today,
  toInput,
  weekdayNames,
  type Day,
} from "./dates";
import { davVersion, findPlace, placeKey, places } from "./store";
import { inkOn } from "./colors";

const FALLBACK_COLOR = "#5b626c";

/** What the editing panel holds. */
interface Draft {
  account: string;
  /** `null` for an event not saved yet. */
  id: number | null;
  place: string;
  recurring: boolean;
  summary: string;
  location: string;
  description: string;
  allDay: boolean;
  startDay: string;
  startTime: string;
  endDay: string;
  endTime: string;
}

function errorText(e: unknown): string {
  return String((e as { message?: string })?.message ?? e);
}

/** The days an event covers, as the grid counts them. */
function span(event: EventRow): [number, number] {
  if (event.allDay) {
    return [dayNumber(dayOfDate(event.start)), dayNumber(dayOfDate(Math.max(event.start, event.end - DAY)))];
  }
  // An event that ends exactly at midnight does not reach into that day.
  const last = event.end > event.start ? event.end - 1 : event.end;
  return [dayNumber(dayOfMoment(event.start)), dayNumber(dayOfMoment(last))];
}

export function Calendar() {
  const [shown, setShown] = createSignal({ year: today().year, month: today().month });
  const [events, setEvents] = createSignal<EventRow[]>([]);
  const [picked, setPicked] = createSignal<Day>(today());
  const [draft, setDraft] = createSignal<Draft>();
  const [error, setError] = createSignal("");
  let request = 0;

  const grid = createMemo(() => monthGrid(shown().year, shown().month));

  createEffect(
    on([grid, davVersion], ([days]) => {
      const mine = ++request;
      // A day either side, so events are found whatever the time zone.
      const from = startOfDay(days[0]!) - DAY;
      const to = startOfDay(addDays(days[41]!, 1)) + DAY;
      core
        .send({ type: "listEvents", from, to })
        .then((r) => mine === request && setEvents(r.events))
        .catch((e) => mine === request && setError(errorText(e)));
    }),
  );

  /** Events by day number. */
  const byDay = createMemo(() => {
    const map = new Map<number, EventRow[]>();
    for (const event of events()) {
      const [first, last] = span(event);
      for (let d = first; d <= last; d++) {
        if (!map.has(d)) map.set(d, []);
        map.get(d)!.push(event);
      }
    }
    for (const list of map.values()) list.sort((a, b) => Number(b.allDay) - Number(a.allDay) || a.start - b.start);
    return map;
  });

  const move = (months: number) => {
    const m = shown().month - 1 + months;
    setShown({ year: shown().year + Math.floor(m / 12), month: ((m % 12) + 12) % 12 + 1 });
  };

  function create(day: Day) {
    const all = places("events");
    if (all.length === 0) {
      setError("Add a calendar account in Settings first, or switch a calendar on.");
      return;
    }
    setError("");
    setPicked(day);
    setDraft({
      account: all[0]!.account,
      id: null,
      place: placeKey(all[0]!),
      recurring: false,
      summary: "",
      location: "",
      description: "",
      allDay: false,
      startDay: toInput(day),
      startTime: "09:00",
      endDay: toInput(day),
      endTime: "10:00",
    });
  }

  function open(event: EventRow) {
    setError("");
    core
      .send({ type: "openDavItem", account: event.account, id: event.id })
      .then((r) => {
        const e = r.item.event;
        if (!e) return;
        const day = (unix: number) => toInput(e.allDay ? dayOfDate(unix) : dayOfMoment(unix));
        setDraft({
          account: event.account,
          id: event.id,
          place: `${event.account}#${r.item.collection}`,
          recurring: r.item.recurring,
          summary: e.summary,
          location: e.location,
          description: e.description,
          allDay: e.allDay,
          startDay: day(e.start),
          startTime: e.allDay ? "09:00" : timeInput(e.start),
          // An all-day event's end is the day after its last day.
          endDay: day(e.allDay ? Math.max(e.start, e.end - DAY) : e.end),
          endTime: e.allDay ? "10:00" : timeInput(e.end),
        });
      })
      .catch((e) => setError(errorText(e)));
  }

  function save(e: Event) {
    e.preventDefault();
    const d = draft();
    if (!d) return;
    const place = findPlace(places("events"), d.place);
    const [startDay, endDay] = [fromInput(d.startDay), fromInput(d.endDay)];
    if (!startDay || !endDay) return setError("The dates are not complete.");
    if (!d.summary.trim()) return setError("Give the event a title.");
    const content: EventContent = d.allDay
      ? { summary: d.summary, location: d.location, description: d.description, allDay: true, start: dateValue(startDay), end: dateValue(endDay) + DAY }
      : { summary: d.summary, location: d.location, description: d.description, allDay: false, start: moment(startDay, d.startTime), end: moment(endDay, d.endTime) };
    if (content.end < content.start || (d.allDay && content.end <= content.start)) return setError("The event ends before it starts.");
    const collection = place?.collection.id ?? Number(d.place.split("#")[1]);
    core
      .send({ type: "saveEvent", account: place?.account ?? d.account, collection, id: d.id, event: content })
      .then(() => setDraft(undefined))
      .catch((e) => setError(errorText(e)));
  }

  function remove() {
    const d = draft();
    if (!d || d.id === null) return;
    const what = d.recurring ? `"${d.summary}" and all its repeats` : `"${d.summary}"`;
    if (!window.confirm(`Delete ${what}?`)) return;
    core
      .send({ type: "deleteDavItem", account: d.account, id: d.id })
      .then(() => setDraft(undefined))
      .catch((e) => setError(errorText(e)));
  }

  const set = <K extends keyof Draft>(key: K, value: Draft[K]) => setDraft({ ...draft()!, [key]: value });

  return (
    <div class="organizer calendar">
      <div class="calendar-main">
        <header class="organizer-head">
          <h1>{formatMonth({ ...shown(), day: 1 })}</h1>
          <button onClick={() => move(-1)} aria-label="Previous month">
            ‹
          </button>
          <button
            onClick={() => {
              setShown({ year: today().year, month: today().month });
              setPicked(today());
            }}
          >
            Today
          </button>
          <button onClick={() => move(1)} aria-label="Next month">
            ›
          </button>
          <span class="organizer-gap" />
          <button class="primary" onClick={() => create(picked())}>
            New event
          </button>
        </header>
        <Show when={error() && !draft()}>
          <p class="panel-error" role="alert">
            {error()}
          </p>
        </Show>
        <div class="month-names">
          <For each={weekdayNames()}>{(name) => <span>{name}</span>}</For>
        </div>
        <div class="month" role="grid" aria-label={formatMonth({ ...shown(), day: 1 })}>
          <For each={grid()}>
            {(day) => {
              const list = () => byDay().get(dayNumber(day)) ?? [];
              return (
                <div
                  class="month-day"
                  role="gridcell"
                  classList={{
                    other: day.month !== shown().month,
                    today: sameDay(day, today()),
                    picked: sameDay(day, picked()),
                  }}
                  onClick={() => setPicked(day)}
                  onDblClick={() => create(day)}
                  title={`${formatDay(day)}. Double-click for a new event.`}
                >
                  <span class="month-number">{day.day}</span>
                  <For each={list().slice(0, 4)}>
                    {(event) => (
                      <button
                        class="month-event"
                        classList={{ "all-day": event.allDay }}
                        style={{
                          "--event": event.color ?? FALLBACK_COLOR,
                          "--event-ink": inkOn(event.color ?? FALLBACK_COLOR),
                        }}
                        onClick={(e) => {
                          e.stopPropagation();
                          setPicked(day);
                          open(event);
                        }}
                        title={`${event.summary}${event.location ? ` · ${event.location}` : ""}`}
                      >
                        <Show when={!event.allDay && dayNumber(dayOfMoment(event.start)) === dayNumber(day)}>
                          <span class="month-time">{formatTime(event.start)}</span>
                        </Show>
                        {event.summary || "(no title)"}
                      </button>
                    )}
                  </For>
                  <Show when={list().length > 4}>
                    <span class="month-more">{list().length - 4} more</span>
                  </Show>
                </div>
              );
            }}
          </For>
        </div>
      </div>

      <aside class="organizer-side">
        <Show
          when={draft()}
          fallback={
            <div class="agenda">
              <h2>{formatDay(picked())}</h2>
              <For each={byDay().get(dayNumber(picked())) ?? []} fallback={<p class="panel-hint flush">Nothing on this day.</p>}>
                {(event) => (
                  <button class="agenda-event" style={{ "--event": event.color ?? FALLBACK_COLOR }} onClick={() => open(event)}>
                    <span class="agenda-time">
                      {event.allDay ? "All day" : `${formatTime(event.start)} to ${formatTime(event.end)}`}
                      <Show when={event.recurring}> · repeats</Show>
                    </span>
                    <strong>{event.summary || "(no title)"}</strong>
                    <Show when={event.location}>
                      <span class="agenda-place">{event.location}</span>
                    </Show>
                  </button>
                )}
              </For>
            </div>
          }
        >
          {(d) => (
            <form class="panel" onSubmit={save}>
              <h1>{d().id === null ? "New event" : "Event"}</h1>
              <Show when={d().recurring}>
                <p class="panel-note">
                  This event repeats. What you change here changes every occurrence; its dates below are the first
                  one's. Repeat rules cannot be edited in Outpost yet.
                </p>
              </Show>
              <label>
                Title
                <input type="text" value={d().summary} onInput={(e) => set("summary", e.currentTarget.value)} ref={(el) => setTimeout(() => el.focus())} />
              </label>
              <Show when={d().id === null}>
                <label>
                  Calendar
                  <select value={d().place} onChange={(e) => set("place", e.currentTarget.value)}>
                    <For each={places("events")}>{(p) => <option value={placeKey(p)}>{p.label}</option>}</For>
                  </select>
                </label>
              </Show>
              <label class="check">
                <input type="checkbox" checked={d().allDay} onChange={(e) => set("allDay", e.currentTarget.checked)} />
                All day
              </label>
              <div class="panel-row">
                <label class="grow">
                  Starts
                  <input type="date" required value={d().startDay} onInput={(e) => set("startDay", e.currentTarget.value)} />
                </label>
                <Show when={!d().allDay}>
                  <label>
                    At
                    <input type="time" required value={d().startTime} onInput={(e) => set("startTime", e.currentTarget.value)} />
                  </label>
                </Show>
              </div>
              <div class="panel-row">
                <label class="grow">
                  Ends
                  <input type="date" required value={d().endDay} onInput={(e) => set("endDay", e.currentTarget.value)} />
                </label>
                <Show when={!d().allDay}>
                  <label>
                    At
                    <input type="time" required value={d().endTime} onInput={(e) => set("endTime", e.currentTarget.value)} />
                  </label>
                </Show>
              </div>
              <label>
                Where
                <input type="text" value={d().location} onInput={(e) => set("location", e.currentTarget.value)} />
              </label>
              <label>
                Notes
                <textarea rows={5} value={d().description} onInput={(e) => set("description", e.currentTarget.value)} />
              </label>
              <Show when={error()}>
                <p class="panel-error" role="alert">
                  {error()}
                </p>
              </Show>
              <div class="panel-actions">
                <button type="submit" class="primary">
                  Save
                </button>
                <button
                  type="button"
                  onClick={() => {
                    setDraft(undefined);
                    setError("");
                  }}
                >
                  Cancel
                </button>
                <Show when={d().id !== null}>
                  <button type="button" class="danger" onClick={remove}>
                    Delete
                  </button>
                </Show>
              </div>
            </form>
          )}
        </Show>
      </aside>
    </div>
  );
}
