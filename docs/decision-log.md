# Decision log

Newest first. One entry per decision: what, why, and whether the owner has
confirmed it. The handoff doc holds the decisions made before the build
started.

Status words: **Decided** (owner agreed), **Proposal** (default in use, owner
has not reviewed), **Finding** (a test result).

## 2026-11-18 · Milestone 3 closed

The owner added his real account in the Windows app and reports it working.
He also asked that milestones no longer wait for his confirmation: work
continues until his input is actually needed. The by-eye scrolling check and
the Windows credential store were not separately reported on.

## 2026-11-18 · Milestone 3

**Proposals** unless marked otherwise.

### HTML mail is made safe twice

1. The core reduces it to formatting before it leaves (`render.rs`, using the
   `ammonia` library, MIT or Apache-2.0): no scripts, forms, frames, embedded
   objects, style sheets or event handlers, and only a fixed list of style
   properties that cannot fetch anything or cover the window.
2. The interface shows the result in a sandboxed frame that cannot run
   scripts, submit forms or open windows, under a content policy that allows
   no network access.

Either should be enough alone. Tested with fifteen kinds of hostile markup.

Costs to know about: style sheets in the message head are dropped, so some
newsletters lose their layout (inline styles survive). Remote pictures are
never loaded; the pane says when there were some. Pictures embedded in the
message by content ID (`cid:`) are not shown yet. The per-sender allow list
for remote pictures is milestone 6.

### Links do not navigate

Clicking a link in a message shows where it really goes, with a Copy button.
Opening it in the browser is not wired up yet (it needs a shell permission).
This is the handoff doc's "always show a link's real destination", applied to
every message for now rather than as a setting.

### Folder tree order, and folders kept inside the inbox

Inbox, Drafts, Sent, Archive, Junk, Trash, then everything else by name. A
folder's role comes from the server's special-use marker, otherwise from its
name (Sent Items, Deleted Items, Spam and so on). When every folder on the
server is a child of the inbox, as on the owner's host, they are shown beside
the inbox instead of under it. The folders on disk are not rearranged.

### Accounts are added in the app, and the app syncs them itself

Adding an account signs in first and saves nothing unless that works. The
server details go in `account.json` in the account's folder; the password
never does. Each account then syncs for as long as the app is open, and the
window follows along: sync status in the status bar, folder counts, and the
open list updating in place when mail arrives (keeping the same message at
the top of the view and the same one selected).

Mail lives in the per-user application data folder (`OUTPOST_DATA` overrides
it). An installation with no accounts shows the synthetic demo account until
one is added.

**Decided (owner agreed 18 Nov 2026):** passwords are kept in the operating
system's credential store (Windows Credential Manager, macOS keychain, the
Secret Service on Linux) through the `keyring` library (MIT or Apache-2.0).
Where there is no such store, as under WSL, the app asks for the password
each time it starts and keeps it in memory only. Passwords travel between the
interface and the core in a type that prints as `***`, so one cannot reach a
log through a debug message.

Not built yet: removing an account, editing its details, and accepting a
self-signed certificate from the setup screen. All belong with the settings
window (milestone 8) unless needed sooner.

### An out-of-date or empty index rebuilds itself

If the app opens an index written by an older version, or an empty one beside
mail files, it rebuilds it from the files in the background, showing progress
in the status bar, and then starts sync. Found the hard way: without this, an
upgrade showed an account with no folders and no explanation.

### The list scrolls folders of any size

Past 8 million pixels of content (about 85,000 rows) the scrollbar covers a
stand-in height and the list keeps the true position itself. Wheel, touch and
keys still move the content one for one; dragging the scrollbar covers the
whole folder proportionally. This removes the limit recorded under milestone
1, where the last rows of a 352,000-message folder could not be reached.
Checked in the app on a 162,790-message folder: both ends reachable, 2 slow
frames of 1,413.

### Search box

Above the list: results as you type, newest first, with a button that states
and switches the scope (this folder, or all folders of the account). Results
show the newest 200 matches. A result does not yet say which folder it is in,
and there is no search across accounts.

### Links, attachments and embedded pictures

- A link's destination is shown first; **Open** then hands it to the system's
  browser or mail program. Only `http`, `https` and `mailto` links can be
  opened, checked in the shell, so a message cannot open a file or start a
  program through an unusual address.
- An attachment is saved to the Downloads folder, never opened directly. Its
  name is cleaned (no folders, no reserved or hidden names) and it never
  replaces an existing file. **Show in folder** reveals it in the file
  manager. Choosing where to save, and opening in place, are not built.
- Pictures carried inside the message (`cid:`) are shown; they need no
  network. Remote pictures are still never loaded.

### The inbox stays current during a long download

Still one connection per account, but while any other folder's bodies are
downloading, the download pauses every 30 seconds to bring the inbox up to
date. That covers the case a second connection was meant for (new mail stuck
behind a first sync of a big archive) with much less machinery. A second
connection can still be added if this proves too slow in practice.

### Testing the app as a whole

`tools/app-selftest.py` starts a throwaway Dovecot, runs the real app against
it, adds an account through the interface, delivers a message while the
window is open, and reports what the window showed. It is the only test that
covers window, core and sync together. It needs a display, so it is run by
hand and is not part of CI. Under WSL it can drive the Windows build too
(`python3 tools/app-selftest.py target/release/outpost.exe`); both pass as of
18 Nov 2026.

### Still to do in milestone 3

- The owner's judgement on the result: the milestone is done "when the owner
  can read his real mail in Outpost and scrolling meets the target".
- The Windows credential store has not been exercised. (The Windows app
  passes the end-to-end self-test against the Dovecot in WSL, but that test
  deliberately does not ask for the password to be remembered.)
- Adding a real account in the app, on either system, by the owner.
- The by-eye scrolling check on both systems, still open from milestone 0.
- Keyboard shortcuts beyond list navigation, and a menu bar (milestone 8).

## 2026-11-18 · Milestone 2 closed

### Finding: the owner's hosted account syncs, and nothing on the server changed

The owner ran `outpost-sync sync --once` against his hosted account: 17
folders and 1,655 messages, lists and bodies, in 7 seconds, with nothing left
to download. The before-and-after check found every message's flags on the
server unchanged. With the Dovecot tests passing, that meets the milestone's
"done when".

Seen on that server and worth remembering:

- Before sign-in it advertises only the basics (IDLE, ENABLE, LITERAL+).
  Whether it offers CONDSTORE and QRESYNC after sign-in was not recorded;
  the tool should print that.
- Every folder is a child of the inbox (`INBOX/Sent`, `INBOX/Trash`, …), as is
  usual on cPanel-style hosts. The folder tree in milestone 3 should show
  these beside the inbox, not nested under it, and must find Sent, Drafts,
  Trash and Junk there by special-use marker or by name.
- This account is small. The 80 GB design case has not been tried against a
  real server.

## 2026-11-18 · Milestone 2

**Proposals** unless marked otherwise.

### Finding: `imap-codec` does the job

The spike planned in the library evaluation passed. Against Dovecot 2.4.2,
sign-in, folder listing, read-only open with QRESYNC, header and body
fetching, and IDLE all worked through its typed commands and responses on the
first attempt. Its `Fragmentizer` does the framing, including literals. The
pin is `=2.0.0-alpha.10`. `imap-next` was not needed: the connection layer on
top is about 400 lines (`crates/outpost-core/src/imap/conn.rs`).

### Sync is read-only by construction in this milestone

Folders are opened with EXAMINE, never SELECT, and bodies are fetched with
PEEK. The session layer has no command that changes anything. Tests compare
the server's flags before and after a sync, and `outpost-sync --once` does the
same against a real account.

### How a folder is synced

1. Open it. If the server offers QRESYNC and the folder was fully listed
   before, the open itself reports what was removed or re-flagged since last
   time, and only UIDs at or above the last-seen UIDNEXT are looked at for new
   mail. Otherwise the complete UID-and-flags list is fetched and compared.
2. Remove what is gone, update flags, then fetch list data (nine header fields,
   size, date, flags) for unknown messages, newest first, 200 at a time.
3. Only after that, record the folder's new sync position.

Bodies are a separate step after every folder's list is complete, so all
folders can be browsed early. Small messages (up to 256 kB) are fetched
several per request, up to 2 MB a request; larger ones alone, in 512 kB
pieces. A message known only by its list data is a normal row in the index
with no file yet, and gets its preview and search text when the body lands.

Every step commits as it goes and the sync position is written last, so an
interrupted sync is resumed by running it again. There is no separate "resume"
code path to get wrong.

### When the server renumbers a folder, the local copy is replaced

A changed UIDVALIDITY means no local message can be matched to the server's.
The folder's local index entries and files are deleted and it is synced from
scratch. Safe while sync is read-only, since the local copy holds nothing the
server does not. **To revisit in milestone 4**, when unsent local changes can
exist: they must not be thrown away with it. Matching by Message-ID to avoid
downloading again is a possible later optimisation.

### UIDVALIDITY is also kept beside the mail files

Each synced folder has a small `outpost-uidvalidity` file. File names carry
UIDs, so after an index rebuild this file is what says whether those UIDs
still mean anything to the server. With it, a rebuilt index syncs without
downloading anything again (tested). Without it (imported mail, or the file
was lost) the mail is kept but detached from its UIDs, and the folder
downloads afresh, leaving duplicates. Accepted as a rare case; noted here so
it is not a surprise.

### A folder the server stops listing is deleted locally, with its mail

That is what "notice folder changes made by other clients" means for a
deletion. A rename looks like a deletion plus a new folder and downloads the
mail again; recognising renames is left for later. The deletion only happens
after a folder list the server completed successfully.

### One connection per account, for now

A pass syncs everything, then the same connection waits in IDLE on the inbox;
the other folders are checked every ten minutes (the timer the handoff doc
proposed for polling). New inbox mail therefore waits behind a long first
sync. A second connection dedicated to the inbox is the fix and belongs with
the main window in milestone 3, where the delay would be visible.

### Certificates are checked against the operating system's trust store

Through `rustls-platform-verifier`, so a private certificate authority the
user or their employer installed is honoured. An account can instead carry one
certificate the user chose to accept, which is then the only one trusted for
that server; the host name must still match. Checked by hand on Windows and
Linux: Gmail and Microsoft 365 connect; expired, wrong-name and self-signed
certificates are refused with a reason.

### Not done yet in milestone 2

- **STARTTLS** (encryption negotiated on port 143). Only TLS from the first
  byte (port 993) is implemented. Add if the owner's host or a tester needs it.
- **The app does not start sync itself.** Sync runs from `outpost-sync`. The
  app needs account setup and stored credentials first (milestones 3 and 7).
- **Gmail and Microsoft 365 are untested.** Both need app passwords or OAuth.
- **No reconnection on network change or wake from sleep** beyond noticing the
  dead connection (60 s timeout) and retrying with backoff.

### Dependencies added

`imap-codec` (MIT or Apache-2.0), `tokio` (MIT), `tokio-rustls` and `rustls`
with the `ring` crypto provider (MIT, Apache-2.0 or ISC; ring itself is
Apache-2.0 and ISC), `rustls-platform-verifier` (MIT or Apache-2.0), and
`rpassword` in the command-line tool (Apache-2.0).

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

### A rebuild writes messages into the index in list order

The rebuild loads rows into a scratch file as they are parsed, then copies
them into the index folder by folder, oldest to newest. The rows of any one
page of the message list therefore sit next to each other in the file, and
opening a folder cold reads a few adjacent pages instead of one scattered page
per row. Mail added later by sync arrives roughly in date order anyway.
Measured on a million messages with the file cache emptied: first open of a
163,000-message folder fell from 2.8 s to 0.22 s.

### Memory-mapped reads of the index are off

With them on, warm reads were no faster, and cold reads were five to ten
times slower (the same cold folder open: 0.22 s with, 0.045 s without). This
was measured under WSL2, where page faults are expensive, so the size of the
effect may differ on other systems, but there was no case where mapping
helped.

### Known issue for milestone 3: very large folders exceed the list's maximum height

The message list is one tall scrolling element. Browser engines cap an
element's height at about 33.5 million pixels, which is roughly 350,000 rows.
The synthetic Archive folder (352,194 messages) hit the cap exactly: its last
few thousand rows cannot be scrolled to. The list needs to map the scrollbar
onto a virtual height for folders this large. The core and index are not
affected.

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
