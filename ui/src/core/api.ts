// The command-and-event interface, as the interface sees it.
//
// These types mirror crates/outpost-core/src/api.rs by hand. The JSON shape is
// pinned on the Rust side by crates/outpost-core/tests/wire_format.rs; change
// both together.

export type FolderId = string;

export type Command =
  | { type: "ping" }
  | { type: "listFolders" }
  | { type: "folderLayout"; folder: FolderId }
  | { type: "listMessages"; folder: FolderId; offset: number; limit: number }
  | { type: "openMessage"; folder: FolderId; id: number }
  | { type: "search"; scope: FolderId; text: string; limit: number }
  | { type: "startDemoSync"; folder: FolderId };

export interface FolderInfo {
  id: FolderId;
  name: string;
  depth: number;
  total: number;
  unread: number;
  /** inbox, drafts, sent, archive, junk or trash, when known. */
  role: string | null;
}

export interface MessageRow {
  id: number;
  index: number;
  senderName: string;
  senderAddress: string;
  /** Unix time, seconds, UTC. */
  date: number;
  subject: string;
  preview: string;
  unread: boolean;
  hasAttachment: boolean;
}

/** A message as the reading pane shows it. */
export interface MessageView {
  id: number;
  subject: string;
  senderName: string;
  senderAddress: string;
  to: string;
  cc: string;
  date: number;
  /** Already made safe by the core. Still only ever shown in a sandboxed frame. */
  html: string | null;
  text: string | null;
  attachments: { name: string; mimeType: string; size: number }[];
  remoteContentBlocked: boolean;
  /** False while only the message's list data has been synced. */
  downloaded: boolean;
}

export type Response =
  | { type: "pong"; coreVersion: string }
  | { type: "folders"; folders: FolderInfo[] }
  | { type: "folderLayout"; folder: FolderId; total: number; previewLens: number[] }
  | { type: "messages"; folder: FolderId; offset: number; total: number; rows: MessageRow[] }
  | { type: "searchResults"; rows: MessageRow[] }
  | { type: "message"; message: MessageView }
  | { type: "accepted" };

export type CoreEvent =
  | { type: "syncProgress"; folder: FolderId; done: number; total: number }
  | { type: "syncFinished"; folder: FolderId };

export interface CoreError {
  code: "badCommand" | "unknownFolder" | "unknownMessage" | "internal";
  message: string;
}

/** The response type that answers a given command type. */
type ResponseFor = {
  ping: "pong";
  listFolders: "folders";
  folderLayout: "folderLayout";
  listMessages: "messages";
  search: "searchResults";
  openMessage: "message";
  startDemoSync: "accepted";
};

export type ResponseTo<C extends Command> = Extract<Response, { type: ResponseFor[C["type"]] }>;

/**
 * The only way the interface reaches the core. One implementation per shell
 * (see tauri.ts) plus a stub for running in a plain browser (see stub.ts).
 */
export interface Transport {
  readonly name: string;
  send<C extends Command>(command: C): Promise<ResponseTo<C>>;
  /** Returns a function that stops listening. */
  onEvent(listener: (event: CoreEvent) => void): () => void;
}
