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
| Run the core's tests | `cargo test` |
| Type-check the interface | `npm run typecheck` |
| Run the app while developing | `npm run dev` |
| Build the app (no installer) | `npm run build -- --no-bundle`, giving `target/release/outpost` |

Always build the app with that command, not with `cargo build`. A plain
`cargo build` of the shell produces a development binary that expects the dev
server, and shows "could not connect to localhost" when run on its own.
| Work on the interface in a browser, no Rust | `npm run ui`, then open http://localhost:1420 |

`cargo test` at the root tests the core only, so it works without any webview
libraries installed. Add `--workspace` to compile the shell too.

Every push runs the core tests, the type-check and a shell build on Windows
and Linux (`.github/workflows/ci.yml`).

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
