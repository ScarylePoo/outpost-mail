// Every keyboard shortcut, in one table: the main window acts on it and the
// settings window lists it, so the two cannot disagree. Outlook's keys are
// used where Outlook has one.

export interface Shortcut {
  id: string;
  /** As shown to the user and as matched: modifiers, then the key. */
  keys: string;
  label: string;
  /** Works while the cursor is in a text field. */
  always?: boolean;
  /** Handled by the part of the window it belongs to, not by the main window. */
  elsewhere?: boolean;
}

export const SHORTCUTS: Shortcut[] = [
  { id: "new", keys: "Ctrl+N", label: "New message" },
  { id: "reply", keys: "Ctrl+R", label: "Reply" },
  { id: "replyAll", keys: "Ctrl+Shift+R", label: "Reply to all" },
  { id: "forward", keys: "Ctrl+F", label: "Forward" },
  { id: "send", keys: "Ctrl+Enter", label: "Send the message being written", elsewhere: true },
  { id: "delete", keys: "Delete", label: "Delete" },
  { id: "archive", keys: "Backspace", label: "Archive" },
  { id: "flag", keys: "Insert", label: "Flag or unflag" },
  { id: "read", keys: "Ctrl+Q", label: "Mark as read" },
  { id: "unread", keys: "Ctrl+U", label: "Mark as unread" },
  { id: "selectAll", keys: "Ctrl+A", label: "Select every message in the folder", elsewhere: true },
  { id: "up", keys: "Up / Down", label: "Previous or next message", elsewhere: true },
  { id: "extend", keys: "Shift+Up / Shift+Down", label: "Select a run of messages", elsewhere: true },
  { id: "mail", keys: "Ctrl+1", label: "Go to Mail", always: true },
  { id: "calendar", keys: "Ctrl+2", label: "Go to Calendar", always: true },
  { id: "people", keys: "Ctrl+3", label: "Go to People", always: true },
  { id: "tasks", keys: "Ctrl+4", label: "Go to Tasks", always: true },
  { id: "search", keys: "Ctrl+E", label: "Search", always: true },
  { id: "sync", keys: "F9", label: "Check for new mail now", always: true },
  { id: "print", keys: "Ctrl+P", label: "Print the open message", always: true },
  { id: "settings", keys: "Ctrl+,", label: "Settings", always: true },
  { id: "close", keys: "Escape", label: "Close settings or a panel", always: true },
];

/** Whether a key press is this shortcut. */
export function pressed(e: KeyboardEvent, keys: string): boolean {
  const parts = keys.split("+");
  const key = parts[parts.length - 1]!.toLowerCase();
  const want = (name: string) => parts.slice(0, -1).includes(name);
  if (e.ctrlKey !== want("Ctrl") || e.shiftKey !== want("Shift") || e.altKey !== want("Alt")) return false;
  return e.key.toLowerCase() === key;
}
