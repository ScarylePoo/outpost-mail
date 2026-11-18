// Tasks: one list across every task list, unfinished first by due day.

import { createEffect, createSignal, For, on, Show } from "solid-js";
import { core, type TaskRow } from "../core";
import { dateValue, dayNumber, dayOfDate, dayOfMoment, formatShort, fromInput, today, toInput } from "./dates";
import { davVersion, findPlace, places } from "./store";

interface Draft {
  account: string;
  id: number | null;
  place: string;
  summary: string;
  description: string;
  due: string;
  completed: boolean;
}

function errorText(e: unknown): string {
  return String((e as { message?: string })?.message ?? e);
}

/** The day a task is due, whether it was given as a day or as a moment. */
const dueDay = (task: TaskRow) => (task.due === null ? undefined : task.dueAllDay ? dayOfDate(task.due) : dayOfMoment(task.due));

export function Tasks() {
  const [tasks, setTasks] = createSignal<TaskRow[]>([]);
  const [draft, setDraft] = createSignal<Draft>();
  const [quick, setQuick] = createSignal("");
  const [error, setError] = createSignal("");
  let request = 0;

  createEffect(
    on(davVersion, () => {
      const mine = ++request;
      core
        .send({ type: "listTasks" })
        .then((r) => mine === request && setTasks(r.tasks))
        .catch((e) => mine === request && setError(errorText(e)));
    }),
  );

  function add(e: Event) {
    e.preventDefault();
    const summary = quick().trim();
    const place = places("tasks")[0];
    if (!summary) return;
    if (!place) return setError("Add a calendar account with a task list in Settings first.");
    setError("");
    core
      .send({ type: "saveTask", account: place.account, collection: place.collection.id, id: null, task: { summary, description: "", due: null, completed: false } })
      .then(() => setQuick(""))
      .catch((e) => setError(errorText(e)));
  }

  /** Ticks a task off, or back on, keeping everything else about it. */
  function tick(task: TaskRow, completed: boolean) {
    core
      .send({ type: "openDavItem", account: task.account, id: task.id })
      .then((r) => {
        if (!r.item.task) return;
        return core.send({ type: "saveTask", account: task.account, collection: task.collection, id: task.id, task: { ...r.item.task, completed } });
      })
      .catch((e) => setError(errorText(e)));
  }

  function open(task: TaskRow) {
    setError("");
    core
      .send({ type: "openDavItem", account: task.account, id: task.id })
      .then((r) => {
        const t = r.item.task;
        if (!t) return;
        const day = dueDay(task);
        setDraft({
          account: task.account,
          id: task.id,
          place: `${task.account}#${task.collection}`,
          summary: t.summary,
          description: t.description,
          due: day ? toInput(day) : "",
          completed: t.completed,
        });
      })
      .catch((e) => setError(errorText(e)));
  }

  function save(e: Event) {
    e.preventDefault();
    const d = draft();
    if (!d) return;
    if (!d.summary.trim()) return setError("Say what the task is.");
    const place = findPlace(places("tasks"), d.place);
    const day = fromInput(d.due);
    core
      .send({
        type: "saveTask",
        account: place?.account ?? d.account,
        collection: place?.collection.id ?? Number(d.place.split("#")[1]),
        id: d.id,
        task: { summary: d.summary, description: d.description, due: day ? dateValue(day) : null, completed: d.completed },
      })
      .then(() => setDraft(undefined))
      .catch((e) => setError(errorText(e)));
  }

  function remove() {
    const d = draft();
    if (!d || d.id === null || !window.confirm(`Delete "${d.summary}"?`)) return;
    core
      .send({ type: "deleteDavItem", account: d.account, id: d.id })
      .then(() => setDraft(undefined))
      .catch((e) => setError(errorText(e)));
  }

  const set = <K extends keyof Draft>(key: K, value: Draft[K]) => setDraft({ ...draft()!, [key]: value });
  const open_count = () => tasks().filter((t) => !t.completed).length;

  return (
    <div class="organizer tasks">
      <div class="tasks-main">
        <header class="organizer-head">
          <h1>Tasks</h1>
          <span class="organizer-count">{open_count() === 1 ? "1 to do" : `${open_count()} to do`}</span>
        </header>
        <form class="task-add" onSubmit={add}>
          <input type="text" placeholder="Add a task and press Enter" aria-label="New task" value={quick()} onInput={(e) => setQuick(e.currentTarget.value)} />
        </form>
        <Show when={error() && !draft()}>
          <p class="panel-error" role="alert">
            {error()}
          </p>
        </Show>
        <ul class="task-list">
          <For each={tasks()} fallback={<li class="list-note">No tasks.</li>}>
            {(task) => {
              const day = () => dueDay(task);
              const late = () => !task.completed && day() !== undefined && dayNumber(day()!) < dayNumber(today());
              return (
                <li classList={{ done: task.completed, selected: draft()?.id === task.id && draft()?.account === task.account }}>
                  <input
                    type="checkbox"
                    checked={task.completed}
                    aria-label={`Done: ${task.summary}`}
                    onChange={(e) => tick(task, e.currentTarget.checked)}
                  />
                  <button class="task-title" onClick={() => open(task)}>
                    <span class="task-dot" style={{ background: task.color ?? "transparent" }} />
                    {task.summary || "(no title)"}
                  </button>
                  <Show when={day()}>
                    <span class="task-due" classList={{ late: late() }}>
                      {late() ? "Was due " : ""}
                      {formatShort(day()!)}
                    </span>
                  </Show>
                </li>
              );
            }}
          </For>
        </ul>
      </div>
      <aside class="organizer-side">
        <Show when={draft()} fallback={<p class="panel-hint flush organizer-empty">Select a task to see or change it.</p>}>
          {(d) => (
            <form class="panel" onSubmit={save}>
              <h1>Task</h1>
              <label>
                What
                <input type="text" value={d().summary} onInput={(e) => set("summary", e.currentTarget.value)} />
              </label>
              <label>
                Due
                <input type="date" value={d().due} onInput={(e) => set("due", e.currentTarget.value)} />
              </label>
              <label class="check">
                <input type="checkbox" checked={d().completed} onChange={(e) => set("completed", e.currentTarget.checked)} />
                Done
              </label>
              <label>
                Notes
                <textarea rows={6} value={d().description} onInput={(e) => set("description", e.currentTarget.value)} />
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
                <button type="button" onClick={() => setDraft(undefined)}>
                  Cancel
                </button>
                <button type="button" class="danger" onClick={remove}>
                  Delete
                </button>
              </div>
            </form>
          )}
        </Show>
      </aside>
    </div>
  );
}
