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

## The app self-test (milestone 3)

```sh
python3 tools/app-selftest.py            # uses target/release/outpost
```

Needs Dovecot and a built app; run it on Linux. Under WSL, pass the path to
`outpost.exe` to test the Windows build the same way. It starts a throwaway Dovecot,
runs the real app against it, adds an account through the interface, delivers
one more message while the window is open, then works the window as a person
would: opens a message, flags it, deletes it, searches, replies and sends.
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
```

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
