// The command-and-event interface, as the interface sees it.
//
// These types mirror crates/outpost-core/src/api.rs by hand. The JSON shape is
// pinned on the Rust side by crates/outpost-core/tests/wire_format.rs; change
// both together.

export type FolderId = string;

export type Command =
  | { type: "ping" }
  | { type: "listFolders" }
  | { type: "folderLayout"; folder: FolderId; conversations?: boolean }
  | { type: "listMessages"; folder: FolderId; offset: number; limit: number; conversations?: boolean }
  | { type: "openMessage"; folder: FolderId; id: number }
  | { type: "openThread"; folder: FolderId; id: number }
  | { type: "recheckSignature"; folder: FolderId; id: number }
  | { type: "listDavAccounts" }
  | { type: "addDavAccount"; url: string; username: string; password: string; remember: boolean }
  | { type: "provideDavPassword"; account: FolderId; password: string; remember: boolean }
  | { type: "removeDavAccount"; account: FolderId; password?: string | null }
  | { type: "setDavCollection"; account: FolderId; collection: number; enabled: boolean; color: string | null }
  | { type: "syncDav"; account: FolderId }
  | { type: "listEvents"; from: number; to: number }
  | { type: "listTasks" }
  | { type: "listContacts"; query: string; limit: number }
  | { type: "openDavItem"; account: FolderId; id: number }
  | { type: "saveEvent"; account: FolderId; collection: number; id: number | null; event: EventContent }
  | { type: "saveTask"; account: FolderId; collection: number; id: number | null; task: TaskContent }
  | { type: "saveContact"; account: FolderId; collection: number; id: number | null; contact: ContactContent }
  | { type: "deleteDavItem"; account: FolderId; id: number }
  | {
      type: "answerInvitation";
      folder: FolderId;
      id: number;
      answer: "accepted" | "tentative" | "declined" | "remove";
      account: FolderId;
      collection: number;
    }
  | { type: "getGeneral" }
  | { type: "setGeneral"; general: General; password?: string | null }
  | { type: "updateAccount"; account: FolderId; host: string; port: number; encrypted: boolean; signature: string; password?: string | null }
  | { type: "removeAccount"; account: FolderId; password?: string | null }
  | { type: "exportMessage"; folder: FolderId; id: number }
  | { type: "getProtection" }
  | { type: "setProtection"; protection: Protection; password?: string | null }
  | { type: "setProtectionPassword"; current?: string | null; new?: string | null }
  | { type: "trustMessage"; folder: FolderId; id: number; trusted: boolean; password?: string | null }
  | { type: "setCheckingServer"; account: FolderId; server: string; password?: string | null }
  | { type: "search"; scope: FolderId; text: string; limit: number }
  | { type: "changeFlags"; folder: FolderId; ids: number[]; seen: boolean | null; flagged: boolean | null }
  | { type: "moveMessages"; ids: number[]; to: FolderId }
  | { type: "archiveMessages"; folder: FolderId; ids: number[] }
  | { type: "deleteMessages"; folder: FolderId; ids: number[]; permanently: boolean }
  | { type: "createFolder"; parent: FolderId; name: string }
  | { type: "deleteFolder"; folder: FolderId }
  | { type: "saveAttachment"; folder: FolderId; id: number; index: number }
  | { type: "locateMessage"; folder: FolderId; id: number; conversations?: boolean }
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

/** Only `verified` may ever be shown with a green check. */
export type CheckState = "verified" | "unverified" | "notChecked" | "failed";

/** What is known about whether a message is from who it says. */
export interface SenderCheck {
  state: CheckState;
  /** The server whose results these are; empty if there were none to believe. */
  server: string;
  checks: {
    name: "SPF" | "DKIM" | "DMARC";
    outcome: "pass" | "fail" | "neutral";
    word: string;
    sentence: string;
    aligned: boolean;
  }[];
  /** Where a reply would go, when that is not the sender's address. */
  replyTo: string;
  replyElsewhere: boolean;
  ambiguousFrom: boolean;
  firstTime: boolean;
  /** The address earlier mail under this name came from. */
  nameSeenFrom: string;
  /** A domain the user hears from that the sender's resembles. */
  lookalikeOf: string;
}

/** A signature checked again on this computer. */
export interface SignatureLine {
  domain: string;
  outcome: "valid" | "mismatch" | "unknown";
  sentence: string;
  aligned: boolean;
}

/** A calendar-and-contacts account. */
export interface DavAccountInfo {
  id: FolderId;
  name: string;
  url: string;
  username: string;
  status: SyncStatus;
  collections: DavCollectionInfo[];
}

export interface DavCollectionInfo {
  id: number;
  name: string;
  events: boolean;
  tasks: boolean;
  contacts: boolean;
  color: string | null;
  enabled: boolean;
}

/** One occurrence of an event. */
export interface EventRow {
  account: FolderId;
  id: number;
  collection: number;
  color: string | null;
  summary: string;
  location: string;
  /**
   * Unix seconds. For an all-day event these are days, not moments:
   * midnight UTC of its first day and of the day after its last.
   */
  start: number;
  end: number;
  allDay: boolean;
  recurring: boolean;
}

export interface TaskRow {
  account: FolderId;
  id: number;
  collection: number;
  color: string | null;
  summary: string;
  due: number | null;
  dueAllDay: boolean;
  completed: boolean;
}

export interface ContactRow {
  account: FolderId;
  id: number;
  collection: number;
  name: string;
  emails: string[];
}

export interface EventContent {
  summary: string;
  location: string;
  description: string;
  start: number;
  end: number;
  allDay: boolean;
}

export interface TaskContent {
  summary: string;
  description: string;
  /** A day: midnight UTC. */
  due: number | null;
  completed: boolean;
}

export interface ContactContent {
  name: string;
  emails: string[];
  phones: string[];
  organisation: string;
  note: string;
}

/** One event, task or contact in full. Exactly one of the three is set. */
export interface DavItemView {
  id: number;
  collection: number;
  event: EventContent | null;
  recurring: boolean;
  task: TaskContent | null;
  contact: ContactContent | null;
}

/** How the app behaves in general. */
export interface General {
  notifyNewMail: boolean;
  startMinimised: boolean;
  closeToTray: boolean;
  /** Seconds a message waits, undoable, before it is sent. */
  undoSendSeconds: number;
  /** Folders open as conversations rather than single messages. */
  conversations: boolean;
  /** Lines of each message's text in the list: 0 to 3. */
  previewLines: number;
}

/** The protection settings: each its own switch. */
export interface Protection {
  disableFailed: boolean;
  blockRemoteImages: boolean;
  allowImagesFrom: string[];
  firstTimeBanner: boolean;
  lookalikeWarning: boolean;
  replyWarning: boolean;
  showLinkDestination: boolean;
  recheckSignatures: boolean;
  blockedSenders: string[];
}

export interface MessageRow {
  id: number;
  index: number;
  senderName: string;
  senderAddress: string;
  /** Who it was sent to, as display text. Shown in Sent and Drafts. */
  recipients: string;
  /** Unix time, seconds, UTC. */
  date: number;
  subject: string;
  preview: string;
  unread: boolean;
  flagged: boolean;
  hasAttachment: boolean;
  /** In a conversation list: how many messages, how many unread, and who. */
  threadCount: number;
  threadUnread: number;
  senders: string;
  /** The sender checks. A failed message gets a warning in the list. */
  check: CheckState;
  /**
   * Not sent by the core. Set by the list for a message picked inside an
   * open conversation: the folder that message is in, which need not be the
   * folder on show.
   */
  inFolder?: FolderId;
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
  senderCheck: SenderCheck;
  /** Failed its checks: links taken out, attachments off. */
  restricted: boolean;
  /** The user chose to trust it despite its failed checks. */
  trusted: boolean;
  /** Pictures on the internet were left in for this sender. */
  remoteImagesShown: boolean;
  /** The meeting invitation or cancellation the message carries. */
  invitation: InvitationView | null;
}

export interface InvitationView {
  kind: "request" | "cancel";
  event: EventContent;
  recurring: boolean;
  organizer: string;
  organizerName: string;
  /** The event is already in one of the user's calendars. */
  inCalendar: boolean;
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
  /** The user wrote this message. */
  mine: boolean;
  check: CheckState;
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
  port: number;
  encrypted: boolean;
  /** Text put at the end of messages written from this account. */
  signature: string;
  /** The server whose sender checks are believed; empty if none is known. */
  checkingServer: string;
  /** False while that is a guess the user has not confirmed. */
  checkingServerConfirmed: boolean;
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
  | { type: "signatureRechecked"; signatures: SignatureLine[]; verified: boolean }
  | { type: "davAccounts"; accounts: DavAccountInfo[] }
  | { type: "events"; events: EventRow[] }
  | { type: "tasks"; tasks: TaskRow[] }
  | { type: "contacts"; contacts: ContactRow[] }
  | { type: "davItem"; item: DavItemView }
  | { type: "davSaved"; id: number }
  | { type: "general"; general: General }
  | { type: "protection"; protection: Protection; passwordSet: boolean }
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
  | { type: "accountRemoved"; account: FolderId }
  | { type: "davAccountAdded"; account: FolderId }
  | { type: "davAccountSetupFailed"; error: string }
  | { type: "davChanged"; account: FolderId }
  | { type: "newMail"; account: FolderId; folder: FolderId; count: number; sender: string; subject: string }
  | { type: "accountSetupFailed"; error: string };

export interface CoreError {
  code: "badCommand" | "unknownFolder" | "unknownMessage" | "notPossible" | "wrongPassword" | "internal";
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
  recheckSignature: "signatureRechecked";
  listDavAccounts: "davAccounts";
  addDavAccount: "accepted";
  provideDavPassword: "accepted";
  removeDavAccount: "accepted";
  setDavCollection: "accepted";
  syncDav: "accepted";
  listEvents: "events";
  listTasks: "tasks";
  listContacts: "contacts";
  openDavItem: "davItem";
  saveEvent: "davSaved";
  saveTask: "davSaved";
  saveContact: "davSaved";
  deleteDavItem: "accepted";
  answerInvitation: "accepted";
  getGeneral: "general";
  setGeneral: "general";
  updateAccount: "accepted";
  removeAccount: "accepted";
  exportMessage: "attachmentSaved";
  getProtection: "protection";
  setProtection: "protection";
  setProtectionPassword: "protection";
  trustMessage: "accepted";
  setCheckingServer: "accepted";
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
  /** Shows a system notification. */
  notify(title: string, body: string): Promise<void>;
  /** Shows the unread count on the tray icon. */
  setUnread(count: number): Promise<void>;
}
