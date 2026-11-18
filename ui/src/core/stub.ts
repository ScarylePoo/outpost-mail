// Interface-side stub of the core, for working on the interface in a plain
// browser (`npm run ui`) with no shell and no Rust. It answers the same
// commands with made-up data after a short delay, so the list's loading paths
// are exercised. It is not a port of the Rust generator and the two will not
// produce the same rows.

import type { Command, CoreEvent, FolderInfo, MessageRow, Response, ResponseTo, Transport } from "./api";

const FOLDERS: FolderInfo[] = [
  { id: "stub", name: "Browser stub account", depth: 0, total: 0, unread: 0, role: null },
  { id: "stub/inbox", name: "Inbox", depth: 1, total: 100_000, unread: 12_000, role: "inbox" },
  { id: "stub/small", name: "Small folder", depth: 1, total: 40, unread: 4, role: null },
  { id: "stub/empty", name: "Empty folder", depth: 1, total: 0, unread: 0, role: null },
];

const NAMES = ["Alice Abbott", "Bram Bergström", "Chidi Castellanos", "Dana Dubois", "Elif Eriksen", "Farid Fontaine"];
const TOPICS = ["Quarterly numbers", "Roof repair quote", "Dinner on Saturday", "Server migration", "Invoice 4471"];
const SENTENCES = [
  "Thanks for getting back to me so quickly.",
  "I had a look at this over the weekend and I think we can make it work.",
  "Can you confirm the dates before Friday?",
  "Sounds good.",
  "I spoke to the supplier this morning and they can deliver on the 4th, but only before noon.",
  "The attached file has the figures you asked for, with last year's alongside for comparison.",
];

function hash(n: number): number {
  let x = (n + 0x9e3779b9) | 0;
  x = Math.imul(x ^ (x >>> 16), 0x85ebca6b);
  x = Math.imul(x ^ (x >>> 13), 0xc2b2ae35);
  return (x ^ (x >>> 16)) >>> 0;
}

function preview(i: number): string {
  const kind = hash(i * 7 + 1) % 4;
  const count = kind === 0 ? 1 : kind === 1 ? 2 : 4 + (hash(i * 7 + 2) % 3);
  const parts: string[] = [];
  for (let n = 0; n < count; n++) parts.push(SENTENCES[hash(i * 31 + n) % SENTENCES.length]!);
  return parts.join(" ");
}

function row(i: number): MessageRow {
  const name = NAMES[hash(i) % NAMES.length]!;
  const flags = hash(i * 13 + 5) % 100;
  return {
    id: i,
    index: i,
    senderName: name,
    senderAddress: name.toLowerCase().replace(/[^a-z ]/g, "").replace(" ", ".") + "@example.com",
    date: 1_794_916_800 - i * 700,
    subject: (hash(i * 3) % 3 === 0 ? "Re: " : "") + TOPICS[hash(i * 5) % TOPICS.length]!,
    preview: preview(i),
    unread: flags < 12,
    hasAttachment: flags >= 90,
  };
}

function answer(command: Command): Response {
  const folder = (id: string) => {
    const f = FOLDERS.find((f) => f.id === id);
    if (!f) throw { code: "unknownFolder", message: `no folder "${id}"` };
    return f;
  };
  switch (command.type) {
    case "ping":
      return { type: "pong", coreVersion: "stub" };
    case "listFolders":
      return { type: "folders", folders: FOLDERS };
    case "folderLayout": {
      const total = folder(command.folder).total;
      const previewLens = new Array<number>(total);
      for (let i = 0; i < total; i++) previewLens[i] = preview(i).length;
      return { type: "folderLayout", folder: command.folder, total, previewLens };
    }
    case "listMessages": {
      const total = folder(command.folder).total;
      const start = Math.min(command.offset, total);
      const end = Math.min(start + Math.min(command.limit, 1000), total);
      const rows: MessageRow[] = [];
      for (let i = start; i < end; i++) rows.push(row(i));
      return { type: "messages", folder: command.folder, offset: start, total, rows };
    }
    case "search":
      return { type: "searchResults", rows: [] };
    case "openMessage": {
      const r = row(command.id);
      // Every third message is HTML, one in nine is not downloaded yet, so
      // each state of the reading pane can be seen in a browser.
      const kind = command.id % 9 === 4 ? "pending" : command.id % 3 === 0 ? "html" : "text";
      return {
        type: "message",
        message: {
          id: command.id,
          subject: r.subject,
          senderName: r.senderName,
          senderAddress: r.senderAddress,
          to: "You <you@example.com>",
          cc: command.id % 2 ? "Someone Else <else@example.com>" : "",
          date: r.date,
          html:
            kind === "html"
              ? `<p>${r.preview}</p><p>See <a href="https://example.com/a/long/path?with=query">the document</a> for details.</p><table border="1" cellpadding="6"><tr><td>Item</td><td align="right">12.50</td></tr></table><blockquote>Earlier: ${r.preview}</blockquote>`
              : null,
          text: kind === "text" ? `${r.preview}\n\n> Earlier message quoted here.\n\n-- \n${r.senderName}` : null,
          attachments: r.hasAttachment ? [{ name: "report.pdf", mimeType: "application/pdf", size: 48_213 }] : [],
          remoteContentBlocked: kind === "html" && command.id % 2 === 0,
          downloaded: kind !== "pending",
        },
      };
    }
    case "startDemoSync":
      folder(command.folder);
      return { type: "accepted" };
  }
}

const listeners = new Set<(event: CoreEvent) => void>();

function demoSync(folder: string, total: number) {
  let step = 0;
  const timer = setInterval(() => {
    step++;
    const event: CoreEvent =
      step <= 20
        ? { type: "syncProgress", folder, done: Math.floor((total * step) / 20), total }
        : { type: "syncFinished", folder };
    listeners.forEach((l) => l(event));
    if (step > 20) clearInterval(timer);
  }, 100);
}

export const stubTransport: Transport = {
  name: "browser stub",
  send<C extends Command>(command: C) {
    return new Promise<ResponseTo<C>>((resolve, reject) => {
      // A few milliseconds, like a real process boundary.
      setTimeout(() => {
        try {
          const response = answer(command);
          if (command.type === "startDemoSync") {
            demoSync(command.folder, FOLDERS.find((f) => f.id === command.folder)?.total ?? 0);
          }
          resolve(response as ResponseTo<C>);
        } catch (e) {
          reject(e);
        }
      }, 3);
    });
  },
  onEvent(listener) {
    listeners.add(listener);
    return () => listeners.delete(listener);
  },
};
