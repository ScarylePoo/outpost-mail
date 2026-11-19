# Outpost Mail

A desktop mail client with the layout of classic Outlook, for ordinary IMAP
accounts, storing every message as its own file. Windows and Linux.

**Status: nearing a first release (0.1.0, not yet published).** Milestones
0 to 6, 8 and most of 9 of the plan are built; 7, OAuth sign-in, is parked.
You can add an IMAP account (Gmail and similar with an app password), read
and search your mail, mark, move, archive and delete it, and write, reply to
and forward messages, online or offline. Mail is grouped into conversations:
one row each in the list, opening into a tree, and shown in the reading pane
without the quoted text. Each message shows whether your mail server
verified its sender, and a message that failed is held at arm's length.
Calendars, tasks and contacts sync with CalDAV and CardDAV servers, and
meeting invitations can be answered from the message. There are light, dark
and other colour schemes, and the fonts and sizes are yours to choose.
The store and search are tested at a million messages.

No telemetry: Outpost talks to your mail and calendar servers, to DNS when
checking a signature, and once a day to GitHub to see whether a newer
version is out (a setting).

Installers for Windows and Linux are built by the release workflow but none
has been published or installed on a real machine yet; see
[docs/release.md](docs/release.md). Changing and sending mail is tested
automatically only against a local test server.

- The full plan: [Outpost build handoff.md](Outpost%20build%20handoff.md)
- Building and testing: [docs/testing.md](docs/testing.md)
- Installers and releasing: [docs/release.md](docs/release.md)
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
