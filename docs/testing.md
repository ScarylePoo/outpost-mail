# Building and testing

## What you need

| | Windows | Linux |
| --- | --- | --- |
| Rust (stable) | `rustup`, with the **MSVC** toolchain and the Visual Studio Build Tools | `rustup` |
| Node.js 22 or newer | yes | yes |
| Webview | WebView2 (part of Windows 11; on Windows 10 it comes with Edge) | WebKitGTK 4.1 |

Linux packages, Debian and Ubuntu:

```sh
sudo apt install build-essential pkg-config libwebkit2gtk-4.1-dev libssl-dev \
  libayatana-appindicator3-dev librsvg2-dev libxdo-dev
```

On Windows, if `rustup show` says the default host is `x86_64-pc-windows-gnu`,
either run `rustup default stable-msvc` or set
`RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-msvc` for the commands below.

`node_modules` holds platform-specific programs, so do not share one checkout
between Windows and WSL. Keep a second clone on the Linux side (a clone inside
the WSL home folder also builds far faster than one on a Windows drive).

## Commands

All from the repository root.

| To | Run |
| --- | --- |
| Install interface dependencies (once) | `npm install` |
| Run all automated tests | `cargo test` |
| Type-check the interface | `npm run typecheck` |
| Run the app while developing | `npm run dev` |
| Build the app (no installer) | `npm run build -- --no-bundle`, giving `target/release/outpost` |

Always build the app with that command, not with `cargo build`. A plain
`cargo build` of the shell produces a development binary that expects the dev
server, and shows "could not connect to localhost" when run on its own.
| Work on the interface in a browser, no Rust | `npm run ui`, then open http://localhost:1420 |

`cargo test` at the root tests everything except the shell, so it works
without any webview libraries installed. Add `--workspace` to compile the
shell too.

Every push runs the core tests, the type-check and a shell build on Windows
and Linux (`.github/workflows/ci.yml`).

## Sync tests (milestone 2)

The sync engine is tested against a real IMAP server. Each test starts its own
private Dovecot, as your own user, on a free local port, in a temporary
directory, and removes it afterwards. It never touches a system Dovecot or any
real mail. A second, separately written mail client makes changes "from
elsewhere", and a small proxy cuts the connection at chosen points.

They need Dovecot 2.4 (`sudo apt install dovecot-imapd`) and `openssl`, so
they run on Linux only. **Where Dovecot is missing (Windows) these tests print
`SKIPPED` and count as passed.** They are not skipped in CI: the Linux job
installs Dovecot and sets `OUTPOST_REQUIRE_DOVECOT=1`, which turns a missing
server into a failure.

What they cover: sign-in and its failures; folder names including non-ASCII;
TLS with an accepted certificate, and refusal of an untrusted one, a wrong
name and a different certificate; a first sync compared byte for byte with the
server; proof that syncing changed no flag on the server; changes made by
another client (new mail, deletions, flag changes) with and without fast
resync; a connection cut during sign-in, the folder list, headers and bodies,
then resumed with nothing duplicated; a folder the server renumbered; folders
created and deleted elsewhere; push, polling and reconnecting in a running
sync; and a rebuilt index not downloading anything again.

## Tests of changing and sending mail (milestone 4)

These run with the other automated tests (`cargo test`) and need Dovecot.
Sending is tested against a fake outgoing server built into the tests, which
keeps what it is given and delivers nothing; it can be told to refuse a
password or a recipient, to be busy, or to drop the connection after taking a
message. No test sends mail anywhere.

## Tests of conversations (milestone 5)

Part of `cargo test`. `store/thread.rs` covers how messages are joined
(reply headers in any arrival order, merging, placing by subject and when
not to, copies in two folders, one row per conversation through arrivals,
moves, deletions and a rebuild). `store/reduce.rs` covers what is hidden as
quoted text or signature and, as important, what is left alone. The
`conversations` test in `tests/wire_format.rs` goes through the core's
commands end to end.

To look at the conversation layout without any mail, run the interface in a
browser (`npm run dev -w ui`) and open `/?open=0&select=0`: the stub account
has a six-message conversation with a fork, an added person, an attachment
and a guessed link.

## Tests of sender verification (milestone 6)

Part of `cargo test`.

- `verify.rs`: reading result headers (comments, quoting, folding, forged
  and misleading ones), what verifies, what fails, what must never show a
  green check, and lookalike domains.
- `recheck.rs`: signs messages with a fresh key, then checks that a good
  signature is valid, that a changed body, a changed signed header or a
  different key does not match, and that a missing or withdrawn key is
  "could not re-check". The DNS answers are supplied by the test; nothing
  is looked up on the network.
- `settings.rs`: defaults, a damaged file, and the password.
- `render.rs`: pictures allowed per sender, and links taken out.
- `tests/core_accounts.rs`, against Dovecot:
  - **the known-result set** (`KNOWN_RESULTS`): twelve messages whose right
    answer is known, each checked in the list and when opened. Add a case
    here whenever real mail shows something new.
  - what earlier mail says about a new sender;
  - a failed message end to end: no links, no pictures, no saving, until
    trusted with the password; pictures for one sender; a blocked sender's
    mail going to Junk here and on the server;
  - mail arriving while the connection is busy.

To see each state without any mail, run the interface in a browser
(`npm run dev -w ui`): `/?select=12` is a failed message, `/?select=9` a
verified one, `/?select=2` a lookalike, `/?panel=protection` the settings.

## Intermittent failures seen and not explained

Three, each seen once on 18 November 2026. None has been reproduced on
demand. They are listed so that the next sighting is recognised and its
output kept.

- **On CI (Linux), run 44303721708:** `an_account_is_added_synced_and_kept_up_to_date`
  in `tests/core_accounts.rs` timed out after 30 seconds "waiting for the
  pushed message": mail delivered while the app was idle was not announced.
  The runs before and after it passed. This is the one to take seriously,
  because it is the push path that real use depends on; if it is real, new
  mail would wait for the ten-minute poll. It is not known whether the test
  or the sync engine is at fault.
- Locally, under load: the next entry.
- Locally: `tests/dav_core.rs`, described under the calendar tests above.

**Seen once, 18 November 2026:** `first_sync_mirrors_the_account_and_changes_nothing`
in `tests/sync_engine.rs` failed in one full run on Linux while the machine
was also compiling the Windows build, and passed in the four runs after it.
The failure text was not captured. If it shows up again, capture it: a test
that fails only under load is usually a timing assumption in the test.

**Explained and fixed, 19 November 2026:** `an_invitation_is_shown_answered_filed_and_replied_to`
in `tests/invitations.rs` failed on CI (Linux, run 44486187126) with an
`unwrap` on `None` at the cancellation step. The test waited for the
cancellation's row to be listed and then read its invitation at once; a row
is listed when its headers are known, which can be a moment before its body
has been downloaded. The test now waits for the body. The app was not at
fault: the reading pane shows "not downloaded yet" in that moment.

## Tests of calendars, tasks and contacts (phase two)

These need **Radicale**, a small CalDAV and CardDAV server, the way the mail
tests need Dovecot. Each test starts its own on a free port with its own
storage and one user, and removes it afterwards. Without it they print
`SKIPPED` (CI sets `OUTPOST_REQUIRE_RADICALE=1`, which makes that a failure).

```sh
pip install --target ~/outpost-dav radicale
```

The tests look in the system Python and in `~/outpost-dav` (or wherever
`OUTPOST_RADICALE` points). Nothing is installed system-wide.

Part of `cargo test`:

- `dav/http.rs`, `dav/client.rs`: addresses, reading answers, reading the
  server's XML.
- `dav/item.rs`: events in every time form, repeats across a clock change
  and with exceptions, edits that keep what other programs wrote, tasks,
  contacts, invitations and replies.
- `dav/store.rs`: the lists, the user's choices, changes waiting to be sent.
- `tests/dav_client.rs`: the client against Radicale.
- `tests/dav_sync.rs`: two-way sync with a second client changing things at
  the same moment; an edit made in two places; an unreachable server.
- `tests/dav_core.rs`: an account end to end through the core's commands.
- `tests/invitations.rs`: needs Dovecot **and** Radicale: an invitation
  arrives by mail, is accepted into the calendar, the organiser is
  answered, it is cancelled and removed.

The app self-test also starts a Radicale if there is one: it adds the
account, makes an event and a task in the window, ticks the task off, and
prints what the calendar server ended up holding.

To see the views without a server, run the interface in a browser:
`/?view=calendar`, `/?view=tasks`, `/?view=people`, and `/?select=3` for a
message carrying an invitation.

**Seen once, 18 November 2026:** `tests/dav_core.rs` failed once in a run
where the test's own second client could not list a collection, and passed
in the 57 runs after it. The message was not captured.

## Tests of settings and accounts (milestone 8)

Part of `cargo test`: `settings.rs` (general settings kept and kept in
range, the password guarding them) and, against Dovecot in
`tests/core_accounts.rs`, new mail being announced once and only when it is
new and unread, a message saved as a file being the message byte for byte,
an account's server and signature being changed, and an account being
removed with its files and stored password while the server is untouched.

The app self-test opens Settings, lists its sections, searches it and
closes it with Escape.

**By eye, not automated** (nothing in the test run can see these):

1. Settings, General: switch on "Keep running in the tray". Close the
   window. The tray icon should remain; clicking it should bring the window
   back, and its menu should have Quit.
2. Send yourself a message from somewhere else. A notification should
   appear, and the tray icon's tooltip should say how much is unread.
3. Open a message and press Ctrl+P. Only the message should be on the page.
4. Linux: the tray needs a desktop that shows tray icons, and
   `libayatana-appindicator3`.

## Tests of hardening, appearance and release (milestone 9)

Part of `cargo test`, and needing no server, so they run on Windows too:

- `tests/hardening.rs`: a second core on the same data folder is refused,
  until the last handle on the first is dropped; an index overwritten with
  rubbish, and one cut short, are each set aside and the mail read back in;
  a damaged settings file gives the protective defaults; with update
  checking off, nothing is asked of GitHub unprompted.
- `log.rs`: one event is one line, repeats are dropped, and the log never
  grows past three files of 1 MB.
- `updates.rs`: versions compare by number; a release's link is used only
  if it is under this project's releases; an answer is kept for a day.
- `settings.rs`: appearance is kept, kept in range, needs no password, and
  a font name cannot carry style-sheet syntax.

The app self-test now also chooses the Slate colour scheme and reports
that the window took it and the core kept it, sets the size to 125% and
reports that the window zoomed, and reports whether fonts were found on the
computer. While it runs, the app makes its daily update check: one request
to `api.github.com`, the only thing in the test that leaves the machine.

Since 19 November it also covers pictures and links, which had been broken
in the app without any test noticing. The test starts a small web server
with one picture on it and delivers a formatted message that uses it, with
a link, and a plain-text message with addresses written out. It reports
that the picture is absent at first, that it loads after "Always load
pictures from…" (and the web server confirms it was asked), that clicking
the link at its place on the screen shows its destination, and that the
addresses in the plain text became links. It also reports a row's height
before and after changing the spacing setting.

It also shift-clicks and Ctrl-clicks onto unread messages (the selection
must grow and nothing may be marked read), changes a message's subject with
F2 (the list, the reader and search must follow, and the server's copy must
keep the sender's subject), and adds a second account and sends a new
message from it. A unit test in `store/mod.rs` covers the changed subject
surviving an index rebuild.

Nothing tests the update check against a live newer release, because there
is no release. Nothing runs an installer. See `docs/release.md`.

**By eye, not automated:**

1. Settings > Appearance: try each colour scheme. Look at the message list,
   an open plain-text message, an open formatted message (which stays on a
   light page), the calendar, and a message that failed its checks.
2. Change the font and the size; the message list's rows should not clip or
   leave gaps.
3. Start Outpost a second time while it is running: the first window should
   come forward and no second one appear.
4. After installing with an installer: Settings > General > "Make Outpost
   the default", then click an email address on a web page. A new message
   to that address should open in Outpost.

## The app self-test (milestone 3)

```sh
python3 tools/app-selftest.py            # uses target/release/outpost
```

Needs Dovecot and a built app; run it on Linux. Under WSL, pass the path to
`outpost.exe` to test the Windows build the same way. It starts a throwaway Dovecot,
runs the real app against it, adds an account through the interface, delivers
one more message while the window is open, then works the window as a person
would: opens a message, flags it, deletes it, searches, replies and sends.
It then opens the conversation its reply made and reports the tree and the
reading pane. One of its messages is delivered as a forgery: expect one row
marked as failed, the failed-message view on it with no green check, and
"Verified sender" on a genuine one.
It prints what the window showed and then what the mail server and the (fake)
outgoing server ended up with. Expect the deleted message in Trash with its
flag, the reply in Sent, the original marked answered, and one message handed
to the outgoing server. It opens a window for about half a minute.

## Using the app with a real account (milestone 3)

Start the app, press **Add account**, and enter the address, password and
server. It signs in to check them, then syncs and stays in sync while open.
It is still read-only: nothing it does changes mail on the server.

Under WSL start it with `WEBKIT_DISABLE_DMABUF_RENDERER=1`, and expect to be
asked for the password at each start, because WSL has no credential store.

## Syncing a real account from the command line (milestone 2)

Until the app has account setup, `outpost-sync` runs the sync engine from a
terminal. Build it with `cargo build --release -p outpost-cli`.

**It is read-only.** It opens folders in read-only mode and fetches without
marking anything read. It cannot change, move or delete mail on the server.

First check the address and certificate, which needs no password:

```sh
target/release/outpost-sync check --host mail.example.com
target/release/outpost-sync check-smtp --host mail.example.com   # the outgoing server
```

`check-smtp` tries port 465 (TLS) and 587 (STARTTLS) and says which work. It
signs in to nothing and sends nothing.

Then a single pass, with a check that nothing changed on the server:

```sh
target/release/outpost-sync sync --once --data DATA --account home \
    --host mail.example.com --user you@example.com
```

It asks for the password at a hidden prompt, uses it for that run and stores
it nowhere. (`OUTPOST_PASSWORD` in the environment is read instead, for
scripts.) Before and after the pass it records every message's flags on the
server and reports any difference. Mail goes to `DATA/accounts/home`.

A pass that is interrupted (Ctrl-C, a dropped connection, a closed laptop) is
continued by running the same command again. Without `--once` it stays
running: push for the inbox, a check of the other folders every ten minutes,
and automatic reconnection.

To browse the result in the app: `OUTPOST_DATA=DATA target/release/outpost`.

Other options: `--port`, `--plaintext` (no encryption; only for a local test
server), `--no-verify` (skip the before/after check, which reads every flag
twice), `--all-mail` (include Gmail's All Mail folder).

## Performance tests (milestone 1)

`outpost-bench` writes a synthetic mailbox of real message files, indexes it
and times the operations the handoff doc sets targets for. Build it once:

```sh
cargo build --release -p outpost-bench
```

Then, with `DIR` an empty directory on the disk you want to test:

```sh
target/release/outpost-bench all --data DIR --messages 1000000
```

That runs three steps, which can also be run one at a time:

| Step | What it does |
| --- | --- |
| `generate --data DIR --messages N` | Writes N messages as a Maildir tree under `DIR/mail`: conversations with reply headers and quoted history, signatures, HTML parts, attachments, 12 years of dates, nine folders. Same `--seed`, same mailbox. |
| `rebuild --data DIR` | Empties `DIR/index.sqlite` and rebuilds it from the mail files, timing each phase. |
| `measure --data DIR` | Times folder opens, deep pages, message reads and ten kinds of search, and marks each against its target. |

A million messages is about 7 GB of mail plus the index. Generating it takes
under a minute on Linux and well over half an hour on Windows, where creating
small files is far slower.

"First" in the report is the first run in that process; "typical" is the
median of 15 more. The tool cannot empty the operating system's file cache, so
for a truly cold number, reboot and run `measure` alone. On Linux the index
can be dropped from the cache without a reboot:

```sh
python3 -c "import os,sys; fd=os.open(sys.argv[1],0); os.fsync(fd); os.posix_fadvise(fd,0,0,os.POSIX_FADV_DONTNEED)" DIR/index.sqlite
```

To browse a generated mailbox in the app, put it where an account would be and
point the app at the data directory:

```sh
target/release/outpost-bench all --data DATA/accounts/synthetic --messages 1000000
OUTPOST_DATA=DATA target/release/outpost
```

Results are recorded in `docs/performance-results.md`.

## The shell test (milestone 0)

This decides whether the shell stays Tauri. It has two halves, and both must
be done on Windows and on the main Linux machine.

**Use a release build.** Debug builds are slower and prove nothing.

### 1. The measured half

Start the app and press **Run scroll test**. For 20 seconds the list scrolls
itself through five patterns, then a report appears in the reading pane. Press
**Copy** and paste it into `docs/shell-test-results.md`.

To run it unattended, set `OUTPOST_SHELL_TEST` to a file path. The app runs
the test on startup, writes the report there and exits:

```sh
OUTPOST_SHELL_TEST=/tmp/shell-test.txt target/release/outpost
```

Reading the report:

- **slow** counts frames that took more than 1.5 display intervals. A handful
  is fine. More than about 1% in the first three phases is a fail.
- **blank** counts frames in which a visible row had no data yet. It should be
  0 in the first three phases. A few single frames in "random jumps" are
  expected: that phase teleports to places that cannot be loaded in advance.

### 2. The by-eye half

The page cannot see what the engine paints, so this needs a person. In the
Archive folder (200,000 rows):

1. Scroll with the wheel or touchpad, slowly and then as fast as you can.
2. Drag the scrollbar thumb top to bottom and back.
3. Hold Page Down, then End, then Home.
4. Resize the window and drag the edge of the list while it is scrolled deep.

It fails if you see white or empty areas where rows should be, text that
smears or tears, rows drawn in the wrong place, or scrolling that stutters.
Note what you saw, and the graphics card and desktop (X11 or Wayland), under
the pasted report.

### Verdict

Tauri passes if both halves are clean on both systems. If Linux fails, the
handoff doc's rule applies: repeat in Electron and adopt Electron.

## The cPanel sign-in test (milestone 0)

```sh
python3 tools/cpanel-probe.py
```

It asks for the server, the mailbox and its password at a hidden prompt, makes
three read-only calls, and prints whether each sign-in option works. It stores
nothing and its output contains no secrets or message contents.
