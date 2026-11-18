# Outpost Mail

A desktop mail client with the layout of classic Outlook, for ordinary IMAP
accounts, storing every message as its own file. Windows and Linux.

**Status: early development (milestone 2 of 9).** Read-only IMAP sync works
from the command line and is tested against Dovecot. The store, index and
search are tested at a million messages. The app can browse synced or
generated mail but cannot yet set up an account or send.

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
