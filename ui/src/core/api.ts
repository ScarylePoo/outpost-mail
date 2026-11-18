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
  | { type: "startDemoSync"; folder: FolderId };

export interface FolderInfo {
  id: FolderId;
  name: string;
  depth: number;
  total: number;
  unread: number;
}

export interface MessageRow {
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

export type Response =
  | { type: "pong"; coreVersion: string }
  | { type: "folders"; folders: FolderInfo[] }
  | { type: "folderLayout"; folder: FolderId; total: number; previewLens: number[] }
  | { type: "messages"; folder: FolderId; offset: number; total: number; rows: MessageRow[] }
  | { type: "accepted" };

export type CoreEvent =
  | { type: "syncProgress"; folder: FolderId; done: number; total: number }
  | { type: "syncFinished"; folder: FolderId };

export interface CoreError {
  code: "badCommand" | "unknownFolder" | "internal";
  message: string;
}

/** The response type that answers a given command type. */
type ResponseFor = {
  ping: "pong";
  listFolders: "folders";
  folderLayout: "folderLayout";
  listMessages: "messages";
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
