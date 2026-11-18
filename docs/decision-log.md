# Decision log

Newest first. One entry per decision: what, why, and whether the owner has
confirmed it. The handoff doc holds the decisions made before the build
started.

Status words: **Decided** (owner agreed), **Proposal** (default in use, owner
has not reviewed), **Finding** (a test result).

## 2026-11-17 · Milestone 1

All of these are **Proposals** unless marked otherwise: they follow the
handoff doc's storage proposals and fill in details it left open.

### Account layout on disk

`<data>/accounts/<name>/mail/…` holds the Maildir tree and
`<data>/accounts/<name>/index.sqlite` the index. Folders nest as real
directories (the handoff doc's proposal). A folder name that would be invalid
on Windows, or would collide with `cur`, `new` or `tmp`, is percent-encoded on
disk and shown decoded.

Known gap: Windows and macOS treat `Work` and `work` as the same directory, so
two IMAP folders differing only in case would collide there. Not handled yet;
decide in milestone 2 when real folder lists arrive.

### File names

`<time>.M<micros>P<pid>Q<n>R<nonce>.outpost,U=<uid>,S=<size>` then the flag
section. The server UID is in the name (as mbsync and OfflineIMAP do) so a
rebuilt index knows which server message each file is without asking the
server. The Windows separator is `;` (the handoff doc's proposal); both `:`
and `;` are read on every system, so a mail folder can be copied between
Windows and Linux.

### Flags: the index leads, file names follow

Per the handoff doc's proposal. The index stores both the flags the user sees
and the flags currently in the file name; a background pass renames files
whose two values differ. A flag change that has not reached the file name is
lost if the index is rebuilt before it does. That window is short, and the
server holds the true flags once sync exists.

### Messages are ordered by a sort key, and search returns newest first without sorting

Each message gets a 62-bit key: its date in the high bits, a tie-breaker in
the low 23. The folder list is ordered by it, and it is the row number in the
search index. Search can therefore walk its matches newest-first and stop
after the first 50, instead of collecting every match and sorting.

Measured on a million messages (Linux): a word that appears in nearly every
message went from 164 ms typical (290 ms first run, against a 300 ms target)
to 0.1 ms. The cost: the index grew from 1.73 GB to 2.43 GB and a full rebuild
went from 126 s to 215 s, because large row numbers compress worse in the
search index. Chosen because search-as-you-type is used constantly and a
rebuild almost never.

Consequences to remember: sorting the list by anything other than date
(sender, subject, size) will need its own index or a sort, and search results
in any other order need the slower collect-and-sort path.

### Search

SQLite FTS5 over subject, participants and body, storing positions (so exact
phrases can be supported) but not a second copy of the text. Every word typed
must match; the last word matches as a prefix until a space follows it.
Accents and case are ignored. Body text beyond 256 kB per message is not
indexed. What the user types is always quoted, never interpreted as query
syntax; an explicit syntax for phrases and exclusions can be added in the
search box later.

### An index from an older version is discarded, not migrated

Until the first release the index format will keep changing, and everything
in it can be rebuilt, so opening an older index replaces it with an empty one.
An index from a newer version is refused untouched. Before sync state lives
in the index (milestone 2) this needs a real migration path, because sync
positions cannot be rebuilt from files.

### Previews strip quotes with simple rules for now

`>`-quoted lines, the "On … wrote:" line before them, and everything after a
signature separator or an "Original Message" divider are left out of the
preview. HTML quoting and repeated-signature detection are milestone 5.

### Dependencies added

`rusqlite` with bundled SQLite (MIT; SQLite itself is public domain) and
`mail-parser` (MIT or Apache-2.0).

### Finding: creating small files on Windows is about 40 times slower than on Linux

Generating the synthetic mailbox wrote about 22,700 messages a second on Linux
(ext4 under WSL2) and about 500 a second on Windows (NTFS, with Microsoft
Defender's real-time scanning on, which is the likely cause but was not
isolated). Reading them back for indexing was not slow. For Outpost this caps
how fast a first sync can store mail on Windows at a few hundred messages a
second, which is probably still faster than most servers deliver them. To
confirm in milestone 2, and to test with a Defender exclusion on the data
folder.

## 2026-11-17 · Milestone 0 closed

### Decided: the cPanel add-on is out of version 1

The owner ran `tools/cpanel-probe.py` against his host. Neither the mailbox
login on the webmail port nor a cPanel API token could reach the BoxTrapper or
spam-settings calls. The handoff doc's rule for that outcome is that the
add-on drops out of scope. The probe script stays in `tools/` for trying
another host. Not investigated: why the host refuses (the probe's output was
not captured here), so if this matters later, rerun it and keep the output.

### Decided: IMAP protocol layer on `imap-codec`

The owner agreed to the recommendation in `docs/imap-library-evaluation.md`.

### Decided: name

"Outpost Mail" for the repository, identifier and product name; "Outpost" in
the window title.

### Decided: the handoff doc is published in the repository

## 2026-11-17 · Milestone 0

### Decided (provisional): the shell is Tauri

The owner has no native Linux desktop for now; WSL is the only Linux
available. On that basis Tauri is adopted: the measured test passes on Windows
and, with `WEBKIT_DISABLE_DMABUF_RENDERER=1`, on WSLg (software-rendered). It
is provisional because the handoff doc's test called for the owner's Linux
machine and a by-eye check, and neither has happened. Repeat the shell test on
the first native Linux desktop that becomes available; the Electron fallback
stays open until then, so keep all mail logic out of `crates/shell-tauri`.

To decide later: whether the Linux build should set
`WEBKIT_DISABLE_DMABUF_RENDERER=1` itself. It helped on WSLg, but it is a
workaround that may cost performance on machines with working graphics, so it
is not set in code yet.

### Finding: Tauri passes the shell test on Windows

100,000 rows, release build, WebView2 146, 164 Hz display: 0 slow frames out
of 3,305 across all five scroll phases; worst frame 7.9 ms. Rows were briefly
without data only on instant jumps to a random position (one frame, about
6 ms). Full report in `docs/shell-test-results.md`. The by-eye half of the
test and the whole Linux test are still to do.

### Proposal: interface framework is SolidJS 1.9

Fine-grained updates and a small runtime, as the handoff doc asked for: the
whole interface bundle is 30 kB. It passed the Windows scroll test without any
hand-written DOM code in the list. Solid 2.0.0 was released days ago and its
Vite plugin does not support it yet, so the build uses 1.9; revisit when the
tooling catches up.

### Proposal: repository layout

- `crates/outpost-core`: the core library. No shell dependency.
- `crates/shell-tauri`: the Tauri adapter, about 60 lines.
- `ui/`: the interface (TypeScript, Vite). Talks to the core only through
  `ui/src/core`, which has one adapter per shell and a browser stub.
- `tools/`: one-off test scripts. `docs/`: this folder.

### Proposal: the core runs inside the shell's main process

The handoff doc says the core is "its own process, separate from the
interface". With Tauri the core runs in the application's native process and
the interface in the webview's separate process, which meets that. A third,
standalone core process is possible later (the core also speaks JSON text, see
`Core::handle_json`) but adds nothing yet.

### Proposal: the list lays out a folder from preview lengths

To give every row its true height before it is loaded, the core returns one
small number per message (preview length) for the whole folder, about 400 kB
of JSON for 100,000 messages. Line counts are estimated from that and the list
width. Simple and fast, but the estimate is by character count, so a row is
sometimes one line taller than its text. Revisit in milestone 3.

### Note: build environment

- Windows builds use the MSVC Rust toolchain (the machine's default is GNU).
- `npm audit` reports a regular-expression denial-of-service advisory in
  `picomatch`, pulled in by Vite. It is a build tool only and is not shipped
  in the app.
