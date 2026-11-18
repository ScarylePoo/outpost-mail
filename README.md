# Outpost Mail

A desktop mail client with the layout of classic Outlook, for ordinary IMAP
accounts, storing every message as its own file. Windows and Linux.

**Status: early development (milestones 0 to 6 and 8 of 9 built; 7, OAuth
sign-in, is parked).** You can add an IMAP account,
read and search your mail, mark, move, archive and delete it, and write,
reply to and forward messages, online or offline. Mail is grouped into
conversations: one row each in the list, opening into a tree, and shown in
the reading pane without the quoted text. Each message shows whether your
mail server verified its sender, and a message that failed is held at arm's
length. Settings are one searchable page; there is a menu bar, a tray icon
and new-mail notifications. Changing and sending mail
has so far only been tested against a local test server, not real providers.
The store and search are tested at a million messages.

- The full plan: [Outpost build handoff.md](Outpost%20build%20handoff.md)
- Building and testing: [docs/testing.md](docs/testing.md)
- Decisions made during the build: [docs/decision-log.md](docs/decision-log.md)

## Layout

| Path | What |
| --- | --- |
| `crates/outpost-core` | The core: all mail work, behind one command-and-event interface |
| `crates/outpost-cli` | `outpost-sync`: run the sync engine from a terminal |
| `crates/outpost-bench` | Synthetic mailbox generator and performance tests |
| `crates/shell-tauri` | The Tauri shell adapter |
| `ui/` | The interface |
| `tools/` | One-off test scripts |
| `docs/` | Decision log, licences, test instructions |

## Licence

Outpost's own code is released into the public domain under
[the Unlicense](LICENSE). Libraries it uses keep their own licences; see
[docs/dependency-licenses.md](docs/dependency-licenses.md).
