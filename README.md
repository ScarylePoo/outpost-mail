# Outpost Mail

A desktop mail client with the layout of classic Outlook, for ordinary IMAP
accounts, storing every message as its own file. Windows and Linux.

**Status: early development (milestone 0).** There is no mail support yet. The
app shows a synthetic mailbox to test the shell and the interface.

- Building and testing: [docs/testing.md](docs/testing.md)
- Decisions made during the build: [docs/decision-log.md](docs/decision-log.md)

## Layout

| Path | What |
| --- | --- |
| `crates/outpost-core` | The core: all mail work, behind one command-and-event interface |
| `crates/shell-tauri` | The Tauri shell adapter |
| `ui/` | The interface |
| `tools/` | One-off test scripts |
| `docs/` | Decision log, licences, test instructions |

## Licence

Outpost's own code is released into the public domain under
[the Unlicense](LICENSE). Libraries it uses keep their own licences; see
[docs/dependency-licenses.md](docs/dependency-licenses.md).
