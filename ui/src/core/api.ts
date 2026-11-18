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
  | { type: "openThread"; folder: FolderId; id: number }
  | { type: "search"; scope: FolderId; text: string; limit: number }
  | { type: "changeFlags"; folder: FolderId; ids: number[]; seen: boolean | null; flagged: boolean | null }
  | { type: "moveMessages"; ids: number[]; to: FolderId }
  | { type: "archiveMessages"; folder: FolderId; ids: number[] }
  | { type: "deleteMessages"; folder: FolderId; ids: number[]; permanently: boolean }
  | { type: "createFolder"; parent: FolderId; name: string }
  | { type: "deleteFolder"; folder: FolderId }
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
      sending?: SendingSettings | null;
    }
  | { type: "setSending"; account: FolderId; sending: SendingSettings }
  | { type: "composeReply"; folder: FolderId; id: number; mode: "reply" | "replyAll" | "forward" }
  | { type: "editDraft"; folder: FolderId; id: number }
  | { type: "saveDraft"; account: FolderId; draft: DraftContent }
  | { type: "sendMessage"; account: FolderId; draft: DraftContent; delaySeconds: number }
  | { type: "cancelSend"; account: FolderId; id: number }
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

/** One message of a conversation, reduced to what its sender wrote. */
export interface ThreadMessage {
  id: number;
  folder: FolderId;
  folderName: string;
  parent: number | null;
  /** The link to `parent` was inferred from the subject. */
  guessed: boolean;
  senderName: string;
  senderAddress: string;
  recipients: string;
  date: number;
  unread: boolean;
  flagged: boolean;
  downloaded: boolean;
  text: string;
  quoted: string;
  quotedLines: number;
  signature: string;
  attachments: { name: string; mimeType: string; size: number }[];
  added: string[];
  dropped: string[];
}

export interface ThreadView {
  subject: string;
  messages: ThreadMessage[];
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
  /** How the account sends mail, once that has been set up. */
  sending: SendingSettings | null;
}

export interface SendingSettings {
  name: string;
  address: string;
  host: string;
  port: number;
  security: "tls" | "starttls" | "plaintext";
}

/** What the composer holds. */
export interface DraftContent {
  to: string;
  cc: string;
  bcc: string;
  subject: string;
  body: string;
  inReplyTo: string | null;
  references: string[];
  attachments: { name: string; mimeType: string; dataBase64: string }[];
  replyToMessage: number | null;
  forwardOf: number | null;
  replaces: number | null;
}

export type Response =
  | { type: "pong"; coreVersion: string }
  | { type: "folders"; folders: FolderInfo[] }
  | { type: "folderLayout"; folder: FolderId; total: number; previewLens: number[] }
  | { type: "messages"; folder: FolderId; offset: number; total: number; rows: MessageRow[] }
  | { type: "searchResults"; rows: MessageRow[] }
  | { type: "message"; message: MessageView }
  | { type: "thread"; thread: ThreadView }
  | { type: "attachmentSaved"; path: string; name: string }
  | { type: "draft"; draft: DraftContent }
  | { type: "draftSaved"; id: number }
  | { type: "sendQueued"; id: number }
  | { type: "sendCancelled"; cancelled: boolean }
  | { type: "folderCreated"; folder: FolderId }
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
  openThread: "thread";
  changeFlags: "accepted";
  moveMessages: "accepted";
  archiveMessages: "accepted";
  deleteMessages: "accepted";
  createFolder: "folderCreated";
  deleteFolder: "accepted";
  saveAttachment: "attachmentSaved";
  locateMessage: "located";
  listAccounts: "accounts";
  addAccount: "accepted";
  setSending: "accepted";
  composeReply: "draft";
  editDraft: "draft";
  saveDraft: "draftSaved";
  sendMessage: "sendQueued";
  cancelSend: "sendCancelled";
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
