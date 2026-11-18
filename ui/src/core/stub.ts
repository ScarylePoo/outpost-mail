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
    flagged: flags >= 96,
    hasAttachment: flags >= 90,
  };
}

const listeners = new Set<(event: CoreEvent) => void>();

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
    case "search": {
      // Pretend: rows whose text contains the last word typed.
      const word = command.text.trim().split(/\s+/).pop()?.toLowerCase() ?? "";
      const rows: MessageRow[] = [];
      for (let i = 0; i < 3000 && rows.length < command.limit; i++) {
        const r = row(i);
        if (word && `${r.subject} ${r.preview} ${r.senderName}`.toLowerCase().includes(word)) rows.push(r);
      }
      return { type: "searchResults", rows };
    }
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
    case "saveAttachment":
      return { type: "attachmentSaved", path: "/downloads/report.pdf", name: "report.pdf" };
    case "locateMessage":
      return { type: "located", index: command.id };
    case "listAccounts":
      return { type: "accounts", accounts: [] };
    case "addAccount":
      // The browser stub has no server to sign in to.
      setTimeout(
        () => listeners.forEach((l) => l({ type: "accountSetupFailed", error: "The browser stub cannot add accounts." })),
        600,
      );
      return { type: "accepted" };
    case "composeReply":
    case "editDraft":
      return {
        type: "draft",
        draft: {
          to: "Alice Abbott <alice@example.com>",
          cc: "",
          bcc: "",
          subject: "Re: Roof repair quote",
          body: "\n\nOn Mon, 2 Nov 2026 at 09:15 UTC, Alice Abbott wrote:\n> Can you confirm the dates before Friday?\n",
          inReplyTo: null,
          references: [],
          attachments: [],
          replyToMessage: command.id,
          forwardOf: null,
          replaces: null,
        },
      };
    case "saveDraft":
      return { type: "draftSaved", id: 1 };
    case "sendMessage":
      return { type: "sendQueued", id: 1 };
    case "cancelSend":
      return { type: "sendCancelled", cancelled: true };
    case "setSending":
    case "changeFlags":
    case "moveMessages":
    case "archiveMessages":
    case "deleteMessages":
    case "providePassword":
    case "syncNow":
      return { type: "accepted" };
  }
}


export const stubTransport: Transport = {
  name: "browser stub",
  send<C extends Command>(command: C) {
    return new Promise<ResponseTo<C>>((resolve, reject) => {
      // A few milliseconds, like a real process boundary.
      setTimeout(() => {
        try {
          resolve(answer(command) as ResponseTo<C>);
        } catch (e) {
          reject(e);
        }
      }, 3);
    });
  },
  async openLink(url) {
    window.open(url, "_blank", "noopener");
  },
  async revealDownload() {},
  onEvent(listener) {
    listeners.add(listener);
    return () => listeners.delete(listener);
  },
};
