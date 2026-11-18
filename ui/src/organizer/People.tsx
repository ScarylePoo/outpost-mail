// People: every address book as one list, searched as you type.

import { createEffect, createSignal, For, on, Show } from "solid-js";
import { core, type ContactRow } from "../core";
import { davVersion, findPlace, placeKey, places } from "./store";

interface Draft {
  account: string;
  id: number | null;
  place: string;
  name: string;
  emails: string;
  phones: string;
  organisation: string;
  note: string;
}

function errorText(e: unknown): string {
  return String((e as { message?: string })?.message ?? e);
}

const lines = (text: string) =>
  text
    .split("\n")
    .map((l) => l.trim())
    .filter(Boolean);

export function People(props: { onWrite: (to: string) => void }) {
  const [contacts, setContacts] = createSignal<ContactRow[]>([]);
  const [query, setQuery] = createSignal("");
  const [draft, setDraft] = createSignal<Draft>();
  const [error, setError] = createSignal("");
  let request = 0;

  createEffect(
    on([query, davVersion], ([q]) => {
      const mine = ++request;
      core
        .send({ type: "listContacts", query: q, limit: 2000 })
        .then((r) => mine === request && setContacts(r.contacts))
        .catch((e) => mine === request && setError(errorText(e)));
    }),
  );

  function create() {
    const place = places("contacts")[0];
    if (!place) return setError("Add an account with an address book in Settings first.");
    setError("");
    setDraft({ account: place.account, id: null, place: placeKey(place), name: "", emails: "", phones: "", organisation: "", note: "" });
  }

  function open(contact: ContactRow) {
    setError("");
    core
      .send({ type: "openDavItem", account: contact.account, id: contact.id })
      .then((r) => {
        const c = r.item.contact;
        if (!c) return;
        setDraft({
          account: contact.account,
          id: contact.id,
          place: `${contact.account}#${r.item.collection}`,
          name: c.name,
          emails: c.emails.join("\n"),
          phones: c.phones.join("\n"),
          organisation: c.organisation,
          note: c.note,
        });
      })
      .catch((e) => setError(errorText(e)));
  }

  function save(e: Event) {
    e.preventDefault();
    const d = draft();
    if (!d) return;
    if (!d.name.trim()) return setError("A contact needs a name.");
    const place = findPlace(places("contacts"), d.place);
    core
      .send({
        type: "saveContact",
        account: place?.account ?? d.account,
        collection: place?.collection.id ?? Number(d.place.split("#")[1]),
        id: d.id,
        contact: { name: d.name, emails: lines(d.emails), phones: lines(d.phones), organisation: d.organisation, note: d.note },
      })
      .then(() => setDraft(undefined))
      .catch((e) => setError(errorText(e)));
  }

  function remove() {
    const d = draft();
    if (!d || d.id === null || !window.confirm(`Delete ${d.name}?`)) return;
    core
      .send({ type: "deleteDavItem", account: d.account, id: d.id })
      .then(() => setDraft(undefined))
      .catch((e) => setError(errorText(e)));
  }

  const set = <K extends keyof Draft>(key: K, value: Draft[K]) => setDraft({ ...draft()!, [key]: value });

  return (
    <div class="organizer people">
      <div class="people-main">
        <header class="organizer-head">
          <h1>People</h1>
          <input
            type="search"
            class="settings-search"
            placeholder="Search people"
            aria-label="Search people"
            value={query()}
            onInput={(e) => setQuery(e.currentTarget.value)}
          />
          <span class="organizer-gap" />
          <button class="primary" onClick={create}>
            New contact
          </button>
        </header>
        <Show when={error() && !draft()}>
          <p class="panel-error" role="alert">
            {error()}
          </p>
        </Show>
        <ul class="people-list">
          <For each={contacts()} fallback={<li class="list-note">{query() ? "Nobody matches." : "No contacts."}</li>}>
            {(contact) => (
              <li classList={{ selected: draft()?.id === contact.id && draft()?.account === contact.account }}>
                <button class="person" onClick={() => open(contact)}>
                  <strong>{contact.name || "(no name)"}</strong>
                  <span>{contact.emails.join(", ")}</span>
                </button>
                <Show when={contact.emails[0]}>
                  <button
                    class="inline"
                    title={`Write to ${contact.emails[0]}`}
                    onClick={() => props.onWrite(contact.name ? `${contact.name} <${contact.emails[0]}>` : contact.emails[0]!)}
                  >
                    Write
                  </button>
                </Show>
              </li>
            )}
          </For>
        </ul>
      </div>
      <aside class="organizer-side">
        <Show when={draft()} fallback={<p class="panel-hint flush organizer-empty">Select someone to see or change their details.</p>}>
          {(d) => (
            <form class="panel" onSubmit={save}>
              <h1>{d().id === null ? "New contact" : "Contact"}</h1>
              <label>
                Name
                <input type="text" value={d().name} onInput={(e) => set("name", e.currentTarget.value)} ref={(el) => setTimeout(() => el.focus())} />
              </label>
              <Show when={d().id === null && places("contacts").length > 1}>
                <label>
                  Address book
                  <select value={d().place} onChange={(e) => set("place", e.currentTarget.value)}>
                    <For each={places("contacts")}>{(p) => <option value={placeKey(p)}>{p.label}</option>}</For>
                  </select>
                </label>
              </Show>
              <label>
                Email addresses, one per line
                <textarea rows={3} spellcheck={false} value={d().emails} onInput={(e) => set("emails", e.currentTarget.value)} />
              </label>
              <label>
                Phone numbers, one per line
                <textarea rows={2} spellcheck={false} value={d().phones} onInput={(e) => set("phones", e.currentTarget.value)} />
              </label>
              <label>
                Organisation
                <input type="text" value={d().organisation} onInput={(e) => set("organisation", e.currentTarget.value)} />
              </label>
              <label>
                Notes
                <textarea rows={4} value={d().note} onInput={(e) => set("note", e.currentTarget.value)} />
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
