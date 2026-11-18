# Decision log

Newest first. One entry per decision: what, why, and whether the owner has
confirmed it. The handoff doc holds the decisions made before the build
started.

Status words: **Decided** (owner agreed), **Proposal** (default in use, owner
has not reviewed), **Finding** (a test result).

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
