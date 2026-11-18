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
  | { type: "changeFlags"; folder: FolderId; ids: number[]; seen: boolean | null; flagged: boolean | null }
  | { type: "moveMessages"; ids: number[]; to: FolderId }
  | { type: "archiveMessages"; folder: FolderId; ids: number[] }
  | { type: "deleteMessages"; folder: FolderId; ids: number[]; permanently: boolean }
  | { type: "saveAttachment"; folder: FolderId; id: number; index: number }
  | { type: "locateMessage"; folder: FolderId; id: number }
  | { type: "listAccounts" }
  | {
      type: "addAccount";
      host: string;
      port: number;
      encrypted: boolean;
      username: string;
      password: string;
      remember: boolean;
    }
  | { type: "providePassword"; account: FolderId; password: string; remember: boolean }
  | { type: "syncNow"; account: FolderId };

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
  flagged: boolean;
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

export type SyncState =
  | "notSyncing"
  | "indexing"
  | "needsPassword"
  | "connecting"
  | "syncing"
  | "upToDate"
  | "offline"
  | "signInFailed";

export interface SyncStatus {
  state: SyncState;
  detail: string;
}

export interface AccountInfo {
  id: FolderId;
  name: string;
  host: string;
  username: string;
  status: SyncStatus;
}

export type Response =
  | { type: "pong"; coreVersion: string }
  | { type: "folders"; folders: FolderInfo[] }
  | { type: "folderLayout"; folder: FolderId; total: number; previewLens: number[] }
  | { type: "messages"; folder: FolderId; offset: number; total: number; rows: MessageRow[] }
  | { type: "searchResults"; rows: MessageRow[] }
  | { type: "message"; message: MessageView }
  | { type: "attachmentSaved"; path: string; name: string }
  | { type: "located"; index: number | null }
  | { type: "accounts"; accounts: AccountInfo[] }
  | { type: "accepted" };

export type CoreEvent =
  | { type: "accountStatus"; account: FolderId; status: SyncStatus }
  | { type: "folderChanged"; folder: FolderId }
  | { type: "foldersChanged"; account: FolderId }
  | { type: "notice"; account: FolderId; text: string }
  | { type: "accountAdded"; account: FolderId }
  | { type: "accountSetupFailed"; error: string };

export interface CoreError {
  code: "badCommand" | "unknownFolder" | "unknownMessage" | "notPossible" | "internal";
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
  changeFlags: "accepted";
  moveMessages: "accepted";
  archiveMessages: "accepted";
  deleteMessages: "accepted";
  saveAttachment: "attachmentSaved";
  locateMessage: "located";
  listAccounts: "accounts";
  addAccount: "accepted";
  providePassword: "accepted";
  syncNow: "accepted";
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
  /** Opens a web or mail link outside the app. A shell service, not the core's. */
  openLink(url: string): Promise<void>;
  /** Shows a saved attachment in the system's file manager. */
  revealDownload(path: string): Promise<void>;
}
