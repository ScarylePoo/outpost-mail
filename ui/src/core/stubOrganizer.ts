// Made-up calendars, tasks and contacts for the browser stub, so each view
// can be seen without a server. Everything here lives in memory.

import type {
  Command,
  ContactContent,
  DavAccountInfo,
  DavItemView,
  EventContent,
  EventRow,
  Response,
  TaskContent,
} from "./api";

const DAY = 86_400;
const ACCOUNT = "dav/stub";
const today = Math.floor(Date.now() / 1000 / DAY) * DAY;

const account: DavAccountInfo = {
  id: ACCOUNT,
  name: "stub",
  url: "https://dav.example.com/",
  username: "you",
  status: { state: "upToDate", detail: "" },
  collections: [
    { id: 1, name: "Work", events: true, tasks: false, contacts: false, color: "#1f5fbf", enabled: true },
    { id: 2, name: "Home", events: true, tasks: true, contacts: false, color: "#1a7f37", enabled: true },
    { id: 3, name: "People", events: false, tasks: false, contacts: true, color: null, enabled: true },
  ],
};

interface Stored {
  id: number;
  collection: number;
  event?: EventContent;
  /** Repeats every this many days. */
  every?: number;
  task?: TaskContent;
  contact?: ContactContent;
}

let nextId = 100;
const items: Stored[] = [
  { id: 1, collection: 1, every: 1, event: { summary: "Standup", location: "", description: "", start: today - 30 * DAY + 9 * 3600, end: today - 30 * DAY + 9.25 * 3600, allDay: false } },
  { id: 2, collection: 1, event: { summary: "Cutover window", location: "Server room", description: "Checklist attached to the thread.", start: today + 2 * DAY + 22 * 3600, end: today + 3 * DAY + 2 * 3600, allDay: false } },
  { id: 3, collection: 2, event: { summary: "Dinner with Dana", location: "Cafe Nord", description: "", start: today + 19 * 3600, end: today + 21 * 3600, allDay: false } },
  { id: 4, collection: 2, event: { summary: "Holiday", location: "", description: "", start: today + 8 * DAY, end: today + 11 * DAY, allDay: true } },
  { id: 5, collection: 1, event: { summary: "Quarterly review", location: "Room 4", description: "", start: today + 5 * DAY + 14 * 3600, end: today + 5 * DAY + 15.5 * 3600, allDay: false } },
  { id: 10, collection: 2, task: { summary: "Buy milk", description: "", due: today, completed: false } },
  { id: 11, collection: 2, task: { summary: "File taxes", description: "Forms are in the drawer.", due: today + 12 * DAY, completed: false } },
  { id: 12, collection: 2, task: { summary: "Book the roofer", description: "", due: null, completed: false } },
  { id: 13, collection: 2, task: { summary: "Renew passport", description: "", due: today - 20 * DAY, completed: true } },
  { id: 20, collection: 3, contact: { name: "Dana Dubois", emails: ["dana.dubois@example.com"], phones: ["+1 555 0100"], organisation: "Example Corp", note: "" } },
  { id: 21, collection: 3, contact: { name: "Sam Okafor", emails: ["sam@example.com", "sam.okafor@work.example"], phones: [], organisation: "", note: "Prefers calls after ten." } },
  { id: 22, collection: 3, contact: { name: "Priya Raman", emails: ["priya@example.com"], phones: ["+44 20 7946 0000"], organisation: "Finance", note: "" } },
];

const enabled = (collection: number) => account.collections.find((c) => c.id === collection)?.enabled ?? false;
const color = (collection: number) => account.collections.find((c) => c.id === collection)?.color ?? null;

/** Answers a calendar, task or contact command, or `undefined` for others. */
export function answerOrganizer(command: Command, changed: () => void): Response | undefined {
  switch (command.type) {
    case "listDavAccounts":
      return { type: "davAccounts", accounts: [account] };
    case "addDavAccount":
    case "provideDavPassword":
    case "removeDavAccount":
    case "syncDav":
    case "answerInvitation":
      return { type: "accepted" };
    case "setDavCollection": {
      const c = account.collections.find((c) => c.id === command.collection);
      if (c) {
        c.enabled = command.enabled;
        c.color = command.color ?? c.color;
      }
      changed();
      return { type: "accepted" };
    }
    case "listEvents": {
      const events: EventRow[] = [];
      for (const item of items) {
        const e = item.event;
        if (!e || !enabled(item.collection)) continue;
        const row = (shift: number): EventRow => ({
          account: ACCOUNT,
          id: item.id,
          collection: item.collection,
          color: color(item.collection),
          summary: e.summary,
          location: e.location,
          start: e.start + shift,
          end: e.end + shift,
          allDay: e.allDay,
          recurring: item.every !== undefined,
        });
        if (item.every === undefined) {
          if (e.start < command.to && e.end > command.from) events.push(row(0));
          continue;
        }
        const step = item.every * DAY;
        const first = Math.max(0, Math.ceil((command.from - e.end) / step));
        for (let n = first; e.start + n * step < command.to; n++) events.push(row(n * step));
      }
      events.sort((a, b) => a.start - b.start);
      return { type: "events", events };
    }
    case "listTasks":
      return {
        type: "tasks",
        tasks: items
          .filter((i) => i.task && enabled(i.collection))
          .map((i) => ({
            account: ACCOUNT,
            id: i.id,
            collection: i.collection,
            color: color(i.collection),
            summary: i.task!.summary,
            due: i.task!.due,
            dueAllDay: true,
            completed: i.task!.completed,
          }))
          .sort((a, b) => Number(a.completed) - Number(b.completed) || (a.due ?? Infinity) - (b.due ?? Infinity)),
      };
    case "listContacts": {
      const words = command.query.toLowerCase().split(/\s+/).filter(Boolean);
      return {
        type: "contacts",
        contacts: items
          .filter((i) => i.contact && enabled(i.collection))
          .filter((i) => {
            const text = `${i.contact!.name} ${i.contact!.emails.join(" ")} ${i.contact!.organisation}`.toLowerCase();
            return words.every((w) => text.includes(w));
          })
          .map((i) => ({ account: ACCOUNT, id: i.id, collection: i.collection, name: i.contact!.name, emails: i.contact!.emails }))
          .sort((a, b) => a.name.localeCompare(b.name))
          .slice(0, command.limit),
      };
    }
    case "openDavItem": {
      const item = items.find((i) => i.id === command.id);
      if (!item) throw { code: "unknownMessage", message: "it is no longer there" };
      const view: DavItemView = {
        id: item.id,
        collection: item.collection,
        event: item.event ?? null,
        recurring: item.every !== undefined,
        task: item.task ?? null,
        contact: item.contact ?? null,
      };
      return { type: "davItem", item: view };
    }
    case "saveEvent":
    case "saveTask":
    case "saveContact": {
      let item = items.find((i) => i.id === command.id);
      if (!item) {
        item = { id: nextId++, collection: command.collection };
        items.push(item);
      }
      if (command.type === "saveEvent") item.event = command.event;
      if (command.type === "saveTask") item.task = command.task;
      if (command.type === "saveContact") item.contact = command.contact;
      changed();
      return { type: "davSaved", id: item.id };
    }
    case "deleteDavItem": {
      const at = items.findIndex((i) => i.id === command.id);
      if (at >= 0) items.splice(at, 1);
      changed();
      return { type: "accepted" };
    }
    default:
      return undefined;
  }
}
