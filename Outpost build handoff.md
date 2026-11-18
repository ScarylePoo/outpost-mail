# Outpost: build handoff

Nov 17, 2026 · @Scary le Poo

## How to use this doc

This doc is the full brief for building Outpost, a desktop mail client. It is written so a new session can start cold, with no access to the planning conversation.

**If you are the building session:**

- Read the whole doc before writing code. Sections marked **Decided** are settled. Sections marked **Proposal** are defaults the owner has not reviewed yet: follow them, but say so when you first build on one.
- Work one milestone at a time, in the order given under Milestones. Each milestone must be usable and tested before the next begins.
- At the start of every milestone, read its **Model** line. If it differs from the model you are running on, stop and show the owner the switch notice printed there before doing anything else.
- Keep this doc current. When a decision changes or a proposal is confirmed, edit the relevant section and add a line to the decision log in the repository.
- Nothing here was tested. Every claim about a protocol, a host or a library is to be verified in the build, and the list under Open questions says which ones matter most.

**Working rules:**

- The owner is technical (IT and web development background) and will run builds and report back. Explain choices plainly and briefly.
- Never run destructive actions against a real mail account without asking. Use the local test server for anything that deletes, moves or expunges until the sync engine has passed its tests.
- Credentials never go into the repository, logs or this doc.
- Keep a `docs/` folder in the repository with the decision log, the dependency license list and the test instructions, so the project survives a change of session or model.

## What Outpost is

Outpost is a desktop mail client with the layout of classic Outlook, built for ordinary IMAP accounts and storing every message as its own file. It runs on Windows and Linux, with macOS as a nice-to-have.

It is for the owner first, then family and a few friends. The source goes on GitHub for anyone to use, with no plan to promote it. One intended use is on the owner's parents' computers, to help keep them clear of scam mail.

**Design principles, in priority order:**

1. **IMAP done very well.** Reliable sync, offline work and correct behaviour against real servers matter more than breadth of features.
2. **Fast at any size.** The design case is one person with 80 GB of mail. Opening a folder, scrolling and searching must not slow down as the mailbox grows.
3. **It has to feel good.** Typography, spacing, colour and responsiveness are core requirements, not polish.
4. **Outlook's front page without Outlook's back rooms.** Keep the three-pane layout. Drop the ribbon, nested dialogs and per-folder views.
5. **Legitimacy is visible.** Who a message is really from, and whether it passed its checks, is shown in the normal view. Nobody should have to read headers.
6. **Local and unsurprising.** Everything runs on the user's machine. The client adapts to the account and never changes account settings or mail the user did not ask it to touch.

## Decisions already made

These were settled with the owner on 17 November 2026 and are **Decided**. Change one only with the owner's agreement.

| Topic | Decision |
| --- | --- |
| Name | Outpost Mail for the repository, app identifier and product name; Outpost in the window title (confirmed 17 Nov 2026). An unrelated shared-inbox product is also called Outpost. |
| Platforms | Windows and Linux are required. macOS is a nice-to-have. |
| Version 1 scope | Mail only. Calendar, tasks and contacts are phase two. |
| Message storage | Maildir: one file per message, never one large database file of mail. |
| Maildir on Windows | Accept a non-standard separator before the flags in filenames, because Windows forbids the colon. |
| Index | A SQLite index beside the mail files, rebuildable from them, with full-text search. |
| Core | Written in Rust and run as its own process, separate from the interface. |
| Interface | Built with web technology. |
| Shell | Tauri first. Electron is the fallback if Tauri fails the scrolling test on Linux in milestone 0. |
| Gmail | Treated as a generic IMAP server with minimal special handling. Not a focus. |
| Sign-in | Passwords and app passwords, plus OAuth with a client ID the user registers. No built-in client ID. |
| Providers that must work | Hosted IMAP, personal Gmail, Google Workspace and Microsoft 365. |
| cPanel add-on | **Dropped from version 1** (17 Nov 2026). The milestone 0 test found the owner's host allows neither sign-in option. See cPanel add-on. |
| Protections | Each one is its own setting. There is no named protected mode. |
| Views | No per-folder views. A folder remembers its sort order and layout only. |
| Message list | Each row shows sender and time, subject, and up to three lines of preview. Taller tree rows are wanted. |
| Reading pane | Shows the selected branch by default, with whole thread and by date one click away. |
| Licence | The Unlicense for the project's own code. A copyleft library is allowed when clearly best, with the owner's approval. |
| Distribution | GitHub releases. No paid code signing. |
| Calendar servers | Phase two must allow several CalDAV and CardDAV servers, separate from mail accounts. |

## Architecture

Outpost is two parts: a Rust core that owns all mail work, and a web interface that only displays and sends commands. The interface never waits on the network or the disk.

&#91;embedded content: Outpost architecture: shell, core and what the core talks to\]

The interface sends commands down and receives events back. Only the core reaches the mail servers, the disk and the cPanel API.

**The core** holds the sync engine, the Maildir store, the index, message parsing, threading, quote and signature detection, sender checks, sending, and the cPanel client. It exposes one command-and-event interface:

- Commands are requests from the interface, such as list a page of a folder, open a message, or move these messages. Every command returns quickly, from the index where possible.
- Events are pushed by the core, such as new mail, sync progress, a flag changed elsewhere, or an error.

**The interface** is a web app inside the shell. It keeps no mail state of its own beyond what is on screen.

**The shell** is Tauri unless milestone 0 shows it cannot scroll a large list smoothly on Linux, in which case it is Electron. To keep that switch cheap:

- The command-and-event interface must not depend on either shell. Write it once in the core, with a thin adapter per shell.
- No mail logic goes in shell-specific code.

**Proposal:** structure the repository as a Rust workspace with a core library, a small adapter for the shell, and the interface as a separate web project. Choose the interface framework in milestone 0 by the same scrolling test, favouring one with fine-grained updates and a small runtime.

**Why Tauri is not certain:** on Linux it renders through the system's WebKitGTK engine. Developers report slower loading and inconsistent rendering there, and Tauri's own documentation covers graphics problems with some drivers ([Tauri: Linux graphics issues](https://v2.tauri.app/develop/debug/linux-graphics/), [developer write-up](https://blog-ximei-me.pages.dev/blog/en/2025/1008-tauri-issue)).

## Storage

Mail lives on disk as Maildir, one untouched file per message, with a SQLite index beside it that answers every question the interface asks.

**Mail files (Decided):**

- Each message is stored exactly as fetched from the server and is never modified afterwards.
- Each folder has the standard `cur`, `new` and `tmp` directories.
- On Linux and macOS, filenames use the standard `:2,` separator before the flag letters. On Windows they use a substitute character, because the colon is not allowed.
- Attachments stay inside the message file. They are extracted only when the user opens or saves one.

**Mail files (Proposal):**

- Use a semicolon as the Windows separator.
- Map the server's folder hierarchy to real nested directories, so the tree on disk reads like the tree in the app.
- Treat the index as the authority for flags while the app runs, and bring filenames into line in the background. Renaming a file on every read or unread click would be slow on large folders.
- Default data location: the per-user application data folder on each system, changeable in settings. Backing up or moving to another computer is copying that one folder.

**The index (Decided in outline, Proposal in detail):**

- SQLite in write-ahead mode. One database per account, plus a small one for settings.
- It holds, per message: folder, server identifiers, flags, the header fields the list needs, thread membership, the stripped preview text, the sender-check verdict, size, whether it has attachments, and the file's path.
- It holds, per folder: the sync state needed to resume and to detect a server-side reset.
- It holds the queue of actions waiting to be sent to the server.
- Full-text search uses SQLite's built-in FTS5 over subject, participants and body text.
- The index must be rebuildable from the mail files and the server. Rebuild is a user-visible command.

**Rules that keep it fast:**

- Startup never scans the mail directories.
- Folder lists, thread trees, counts and previews come from the index only. A message file is read only when that message is opened.
- The list asks for rows in pages and draws only what is on screen.

**Known risk:** Windows slows down with hundreds of thousands of small files in one directory, and antivirus scanning makes it worse. Test this in milestone 1 with the synthetic mailbox. Splitting a folder across subdirectories would fix it but breaks compatibility with other Maildir tools, so do that only if the test fails and the owner agrees.

## IMAP sync engine

The sync engine is the part of Outpost that most needs to be right, and it is where most of the build effort should go.

**Requirements:**

- **Driven by what the server advertises.** Use each extension when offered and fall back cleanly when not.
- **Encrypted connections only,** unless the user explicitly overrides it for one account.
- **Tracks messages by their server identifiers,** and handles the case where a server resets a folder's identifiers by resyncing that folder safely.
- **Push for new mail** using IDLE on the inbox and on any folders the user chooses. Other folders are polled.
- **Fast resync** using the CONDSTORE and QRESYNC extensions when offered. Otherwise compare identifier and flag lists.
- **Staged first sync:** the folder list, then headers newest first, then bodies newest first. It must be resumable after a crash or a dropped connection, and the mailbox must be usable while it runs.
- **Offline action queue.** Flag changes, moves, deletes and saved drafts are recorded locally, shown immediately, and replayed to the server in order. Replays must be safe to repeat.
- **Careful deletes.** Use the MOVE extension when offered. Otherwise copy, mark deleted and expunge only those specific messages. Never issue a folder-wide expunge, which can remove messages another client has marked.
- **Special folders** (Sent, Drafts, Trash, Junk, Archive) are found by the server's special-use markers, then by name, and the user can override them.
- **Folder management:** create, rename, delete and subscribe, and notice the same changes made by other clients.
- **Connection handling:** a small, capped number of connections per account, backoff on errors, and clean recovery when the network changes or the computer wakes.
- **Large messages** are fetched in parts so one big attachment cannot stall a folder.
- **Sync status is always visible.** A failed sync is never silent.

**Sending:**

- Send over SMTP submission with encryption.
- Save a copy to Sent, except on servers that file sent mail themselves.
- Drafts are saved locally and to the server's Drafts folder.

**Gmail and Google Workspace (Decided: minimal handling):**

- Both behave identically over IMAP.
- Detect Gmail by its advertised capability, not by hostname.
- Do not save a copy to Sent, because Gmail does it and a second copy duplicates the message.
- Delete means move to Trash.
- **Proposal:** skip the All Mail folder by default, so every message is not downloaded twice.
- Full label support through Gmail's own extensions is out of scope for version 1. Do not design anything that would prevent adding it later.

**Proposal:** in milestone 0, evaluate the existing Rust IMAP libraries against these requirements before deciding whether to build on one or write the protocol layer directly. Check that any candidate is actively maintained.

**Servers to test against:** a local Dovecot instance for automated tests, then the owner's hosted account, then Gmail, then Microsoft 365.

## Accounts and sign-in

Outpost supports two sign-in methods: a password, and OAuth with a client ID the user supplies. Between them they cover every provider the owner needs.

| Account type | How it signs in | Notes |
| --- | --- | --- |
| Hosted IMAP (the owner's main account) | Mailbox password | The simplest case and the first one to build. |
| Personal Gmail | App password, or OAuth with the user's own client ID | An app password needs 2-Step Verification. This is the owner's secondary account. |
| Google Workspace | OAuth with the user's own client ID, or an app password if the admin allows it | Password-only sign-in ended on 1 May 2025 ([Google admin help](https://support.google.com/a/answer/105694?hl=en-EN)). An admin can restrict or disable IMAP. |
| Microsoft 365 | OAuth with the user's own app registration, for both receiving and sending | Password-based sending is disabled by default for existing tenants from the end of December 2026 ([Microsoft message MC786329](https://mc.merill.net/message/MC786329?from=1&to=4)). |

**OAuth requirements:**

- One generic implementation: client ID, optional secret, authorisation address, token address and scopes, with presets for Google and Microsoft.
- Sign-in opens the system browser and returns to a local address on the user's machine.
- Tokens refresh automatically. A failed refresh shows a clear prompt to sign in again.
- No client ID ships with the app in version 1. The account setup screen links to short instructions for creating one with each provider.

**Credential storage (Proposal):** passwords and tokens go in the operating system's credential store on each platform, never in the index, settings files or logs.

**Account settings:** everything about an account is on one page: servers, sign-in, identity, signature, sync choices and the cPanel add-on.

**To verify during the build:**

- Whether a Google OAuth app left in testing status makes users sign in again about weekly.
- Whether a Workspace admin can mark the app as internal to avoid that and the test-user limit.
- What a Microsoft 365 tenant needs switched on for IMAP with OAuth, and whether an admin must approve the app.

## Interface

The interface is classic Outlook's main window with a conversation tree in the message list and a reading pane that hides everything a message merely carries along. The owner approved [the mockup](https://claude.ai/artifact/T1Q1uEWQCz1FFVJE59tQx7) as the direction. It was not render-checked, so treat it as intent and tune the details in a running build.

**Main window (Decided):**

- Three panes: folder tree, message list, reading pane.
- A plain menu bar and one fixed toolbar. No ribbon.
- The folder tree has Favorites at the top, then each account with its folders. A Held mail folder appears on accounts with the cPanel add-on.
- A switcher at the bottom left for Mail, Calendar, People and Tasks. **Proposal:** show only Mail until phase two.

**Message list (Decided):**

- Each row shows sender and time, then subject, then up to three lines of preview. Short messages use fewer lines.
- The preview is only what that person wrote: quoted history and signatures are stripped.
- An open conversation is drawn as a tree with connector lines, so forks are visible. Indentation is capped at about five levels, after which replies continue flat with a marker.
- Markers on rows: recipients added or dropped on the message where it happened, attachments only on the message that introduced them, a divider for gaps in time such as 3 days later, and unread dots with a count on the collapsed thread.
- A link the client inferred, where reply headers were missing, is drawn dashed and labelled as placed by subject.
- A warning label appears on a row only when a sender check failed. Verified mail shows nothing extra in the list.

**Reading pane (Decided):**

- Shows the conversation as a stack of messages, each reduced to its new text.
- Three modes, one click each: this branch (default), whole thread, by date.
- Quoted text and signatures are collapsed behind small links that say how much is hidden. A show original link is always available.
- Every message shows the sender's display name, actual address and check result. See Sender verification.

**Threading:**

- Build the tree from each message's reply headers, across all folders, so the user's own replies in Sent appear in place.
- When headers are missing or broken, fall back to matching subject and participants within a time window, and mark the link as guessed.

**Hiding quotes and signatures:**

- Quoted text: detect the plain-text quote prefix, the markup the major clients wrap quotes in, and the header block some clients insert above a quoted message.
- Signatures: detect the standard separator, then detect by repetition. A block that ends several messages from the same sender is that sender's signature. This also catches disclaimers.
- Detection only ever collapses. It never deletes or alters the stored message.

**Views (Decided):** a small set of named layouts that apply everywhere. Sent and Drafts show recipients in place of the sender automatically. Reset this folder and apply to all folders are one click each.

**Settings (Decided):**

1. One settings window, searchable, never more than one level deep.
2. Nothing modal unless it is a real yes or no question.
3. Rules are edited on a single screen.
4. Columns, sorting and grouping are changed directly on the list header.
5. Search is instant and always states its scope.

**Visual design:**

- Inter typeface. Interface text at 13 px, message bodies at 14 px with generous line spacing and a capped line width.
- Neutral grey surfaces, white panes and one blue accent (#1F5FBF), with green, amber and red reserved for sender-check results.
- Meaning never rests on colour alone: check results also differ by icon and wording.
- **Proposal:** a dark theme and adjustable density follow once the light theme is right.

**Feel:** message opens, list scrolling and actions such as delete or flag must respond immediately, with the server catching up in the background. Every common action has a keyboard shortcut, matching Outlook's where one exists.

## Sender verification

Every opened message shows who it claims to be from, the address it actually came from, and whether it passed its checks, without the user opening headers.

**Where the verdict comes from:**

- The user's mail server records the results of SPF, DKIM and DMARC in a header on each message. Outpost reads that.
- Only trust the result header added by the user's own server. A sender can put a forged one in the message, so each account needs to know which server name to believe. **Proposal:** detect it from the account's newest messages and let the user confirm it.
- Outpost re-checks the DKIM signature itself where it can, since the signature travels with the message. Old messages may fail a re-check only because the sender has since rotated keys, so report that as could not re-check, not as a failure.
- SPF cannot be re-checked by a mail client. Say plainly that it is the server's result.

**States:**

| State | When | What the user sees |
| --- | --- | --- |
| Verified sender | The server recorded passes that align with the sender's domain | A green check beside the address. The selected message also lists each result. |
| Not checked | The server recorded nothing usable | A neutral label. Never a green check. |
| Failed | A check failed | A red banner at the top of the message and a warning label on its row in the list. |

**Extra warnings Outpost adds itself:**

- The display name matches earlier mail that came from a different address.
- Replies would go to a different domain than the sender's.
- The sender's domain closely resembles one the user corresponds with.
- This is the first message from this sender.

**The failed-message view (Decided, as mocked up):**

- A banner stating that the message failed its sender checks and that links and remote images are off.
- Name shown, actually sent from, and replies would go to, with mismatches highlighted.
- Each check on its own line with pass or fail and one plain sentence.
- A line saying which server did the checking and whether the signature was re-checked locally.
- Actions: delete, move to spam, block sender, and a low-key option to trust the message and turn links on.
- The body is shown dimmed, with each link disabled and its real destination written out.

**Protection settings (Decided: each is individual):**

- Turn off links and attachments on messages that fail a check.
- Block remote images, with a per-sender allow list.
- Show the first-time sender banner.
- Show the lookalike domain warning.
- Show the reply address warning.
- Always show a link's real destination.
- Require a password to change settings or to override a warning.

The owner intends to switch most of these on for his parents' installations.

## cPanel add-on

> **Out of scope as of 17 November 2026.** The owner ran `tools/cpanel-probe.py` against his host: neither the mailbox login nor an API token could reach the BoxTrapper or spam-settings calls. By the rule under First task below, the add-on drops out of version 1. The rest of this section is kept as the design in case a host that allows it turns up. Ignore mentions of Held mail and the cPanel add-on elsewhere in this doc.

On accounts hosted with cPanel, Outpost can show the BoxTrapper queue as a folder and manage SpamAssassin settings, so the user never needs webmail for either. It is optional per account and invisible on other hosts.

The owner uses BoxTrapper on his main account and has SpamAssassin switched off, so BoxTrapper comes first.

**BoxTrapper held mail:**

- A Held mail folder under the account, with a count.
- The list comes from the queue listing call. The preview comes from the call that returns the first 200 lines of a queued message, so attachments are not visible until delivery.
- Actions: deliver, deliver and always allow the sender, delete, block the sender, ignore the sender.
- The queue has no push mechanism. Poll it on a timer (**Proposal:** every 10 minutes, adjustable) and notify when something new is held.
- Reference: [BoxTrapper functions in the cPanel API](<https://api.docs.cpanel.net/specifications/cpanel.openapi/spam-prevention-(boxtrapper)>).

**SpamAssassin settings:**

- On or off, move spam to a spam folder, the score threshold, auto-delete and its threshold, and the allow and block lists.
- Right-click a message to always allow or always block its sender.
- These settings apply to the whole hosting account, not one mailbox. The screen must say so before saving.
- The host can lock them. Read whether they are changeable and grey out what is not.
- The API offers no way to train the filter.
- Reference: [spam management functions in the cPanel API](https://api.docs.cpanel.net/specifications/cpanel.openapi/spam-management.md).

**Signing in to the API:**

- Option one: the mailbox's own login against the webmail port. Per the documentation, calls made this way apply to the signed-in mailbox. This needs no extra credential and is preferred for BoxTrapper.
- Option two: a cPanel API token. It covers the whole hosting account, so store it in the credential store and treat it with care. SpamAssassin settings probably need this.

**First task:** in milestone 0, test both options against the owner's host. If neither works, the add-on is not viable there and drops out of scope.

## Proposed defaults

None of the features below were discussed in detail. Each row is a **Proposal** for the owner to confirm or change.

| Feature | Proposal |
| --- | --- |
| Composer | Rich text by default with a plain-text copy included, and a plain-text-only option. Inline images, drag-and-drop attachments, one signature per identity. |
| Drafts | Saved locally as you type and to the server's Drafts folder at intervals. |
| Undo send | A short delay before sending, 10 seconds by default, with an undo button. |
| Send later | Left out of version 1, because it only works while the app is open. |
| Sync policy | Download everything by default. Per account: headers only, or skip attachments above a chosen size. Per folder: exclude from sync. |
| Search | Local and instant, with the scope shown as this folder, this account or all mail. Falls back to asking the server for mail that is not downloaded. |
| Rules | None in version 1 beyond the cPanel add-on. Simple client-side rules on one screen come later, then server-side rules where the host supports them. |
| Identities | Several sending identities per account, each with its own name, address and signature. |
| Unified inbox | An All inboxes entry in Favorites. |
| Address suggestions | Collected from mail the user has sent and received, until contacts sync exists. |
| Encryption and signing | PGP and S/MIME are out of version 1. |
| Import | Existing Maildir and mbox in version 1. Thunderbird profiles later. Outlook data files are out of scope. |
| System integration | New mail notifications, a tray icon with an unread count, registering as the default mail app, and an option to start minimised. |
| HTML mail | Rendered in a sandbox with scripts off, remote content blocked by default and the markup sanitised. |
| Updates | Check GitHub releases and notify. The user installs the update by hand in version 1. |
| Printing and export | Print a message or a conversation. Export a message as a standard mail file. |
| Logging | A local rotating log and a sync status panel. No telemetry of any kind. |
| Language | English only in version 1, with text kept separate so translation is possible later. |

## Performance targets and test plan

Outpost must stay fast with a million messages and 80 GB of mail, and the test suite has to prove it on every platform. The numbers below are a **Proposal**: targets to validate, not measurements.

| Measure | Target |
| --- | --- |
| Cold start to a usable window | Under 2 seconds |
| Open a folder of 200,000 messages | First rows in under 100 ms |
| Scrolling the message list | Smooth, with no blank rows |
| Open a downloaded message | Under 100 ms |
| Search across 1,000,000 messages | First results in under 300 ms |
| Flag, move or delete | Visible at once, synced in the background |

**Test assets:**

- **A synthetic mailbox generator** that can produce a million messages with realistic threads, sizes and attachments. All performance tests run against it.
- **A local Dovecot server** in a container for sync tests, including faults: dropped connections mid-download, a folder reset, and another client changing mail at the same time.
- **A set of real, messy conversations** for threading and for quote and signature detection. The owner supplies these and they stay out of the repository.
- **A set of messages with known check results,** passing, failing and missing, for sender verification.

**The shell test (milestone 0):**

- Build a 100,000-row message list with three-line previews in Tauri, fed by the core.
- Run it on the owner's Linux machine and on Windows.
- Tauri passes if scrolling is smooth, rows never blank, and there are no rendering faults.
- If it fails on Linux, repeat in Electron and adopt Electron.

**Automated runs:** every change runs the unit and sync tests on Windows and Linux. Performance tests run on demand and before each release.

**Real accounts:** read-only testing against the owner's hosted account starts in milestone 2. Anything that changes or deletes mail runs against the test server first.

## Milestones

There are ten milestones for version 1, built in order. Each names the model to use, and two of them ask for a switch.

**About the model lines.** They reflect the models available in November 2026: Fable 5.2 is the most capable and is recommended for anything architectural, protocol-level or security-sensitive. Opus 5.5 is suitable for routine interface work. If a more capable model exists when you read this, prefer it. If the owner's plan limits make Fable 5.2 impractical, Opus 5.5 can do any milestone, with more reliance on the tests.

1. **Milestone 0: Foundations and the three tests.** Model: Fable 5.2.
   - Repository, Rust workspace, automated test runs on Windows and Linux, the `docs/` folder.
   - The command-and-event interface between core and interface, with a stub on each side.
   - The shell test, the cPanel sign-in test, and the IMAP library evaluation.
   - Done when: the shell and interface framework are chosen and recorded, and the owner knows whether the cPanel add-on is viable.
2. **Milestone 1: Store and index.** Model: Fable 5.2.
   - Maildir reading and writing with the Windows filename rule, the index schema, full-text search, index rebuild.
   - The synthetic mailbox generator and the performance tests.
   - Done when: the million-message mailbox meets the folder-open and search targets on both platforms.
3. **Milestone 2: Sync engine, read-only.** Model: Fable 5.2.
   - Connect, list folders, staged first sync, push for new mail, fast resync, recovery from faults.
   - Done when: the sync tests pass against the test server, and the owner's hosted account syncs fully without changing anything on the server.
4. **Milestone 3: Main window, read-only.** Model: Fable 5.2.
   - Folder tree, a flat message list with three-line previews, the reading pane with safe HTML rendering, sync status.
   - Done when: the owner can read his real mail in Outpost and scrolling meets the target.
5. **Milestone 4: Changing and sending mail.** Model: Fable 5.2.
   - The offline action queue, flags, move, delete, archive, folder management.
   - Sending, a basic composer, drafts, reply and forward.
   - Done when: the owner can use Outpost for a full day on his main account, including offline, with nothing lost or duplicated.
6. **Milestone 5: Conversations.** Model: Fable 5.2.
   - Threading across folders, the tree in the list, quote and signature hiding, the three reading modes, recipient and gap markers.
   - Done when: the owner's set of messy conversations displays correctly, and every guess is labelled as one.
7. **Milestone 6: Sender verification.** Model: Fable 5.2.
   - Reading the server's results, local signature re-checks, the extra warnings, the failed-message view, the individual protection settings.
   - Done when: the known-result message set shows the right state every time, and nothing unverified shows a green check.
8. **Milestone 7: OAuth sign-in.** Model: Fable 5.2.
   - The generic OAuth flow, presets for Google and Microsoft, token refresh, credential storage.
   - Done when: personal Gmail, a Workspace account and a Microsoft 365 account each sync and send.
9. **Milestone 8: Settings, shortcuts and system integration.** Model: Opus 5.5.

   > **Switch notice.** This milestone is routine interface work: the settings window, account pages, toolbar and menu wiring, keyboard shortcuts, notifications and the tray icon. Opus 5.5 is suitable and spreads usage. Tell the owner: "Milestone 8 is a good point to switch to Opus 5.5. Stay on Fable 5.2 if you prefer."
   - The cPanel add-on screens are not part of this milestone: milestone 0 found the add-on is not viable on the owner's host.
   - Done when: every setting in this doc exists, on one searchable window with nothing nested.
10. **Milestone 9: Hardening and release.** Model: Fable 5.2.

    > **Switch notice.** This milestone returns to work where mistakes are costly: performance at full scale, fault recovery, installers and the update check. Tell the owner: "Milestone 9 should run on Fable 5.2. Please switch back before continuing."
    - Full performance run at a million messages, a week of daily use by the owner, installers for Windows and Linux, the first GitHub release.
    - Done when: the targets are met or the gaps are written down and accepted by the owner.

A macOS build can follow milestone 9 if the owner wants it.

## Phase two: calendar, tasks and contacts

Phase two adds calendar and tasks over CalDAV and contacts over CardDAV, after version 1 is in daily use. It is not to be started before then, but version 1 must not block it.

**Decided:**

- The user can define several servers. Server accounts for calendar and contacts are a separate list from mail accounts.
- Each server account is an address plus a sign-in. Outpost discovers the calendars, task lists and address books on it, and the user chooses which to sync.
- Everything merges in the interface: one calendar view with every calendar overlaid and colour-coded, one task list, one contact list.

**Proposal:**

- Store each event or task as its own standard calendar file and each contact as its own standard contact file, in a folder per calendar or address book, with the same index approach as mail.
- Use an established library to expand repeating events. Do not write that logic by hand.
- Handle meeting invitations that arrive by email: show them in the message, and accepting one writes to the calendar and sends the reply.
- Google and iCloud are nice-to-have. Self-hosted servers come first.

**What version 1 must leave room for:** the account model, the credential store, the index design and the bottom-left switcher should all be able to take these on without rework.

## Licensing

Outpost's own code is released under the Unlicense, and the owner wants the project as permissive as it can practically be.

- Prefer libraries under permissive licences such as MIT, Apache 2.0 and BSD. Keep their licence notices in every release.
- A copyleft library may be used when it is clearly the best tool. Ask the owner first. If one is adopted, the built app as a whole is distributed under that library's licence, while the project's own source files stay under the Unlicense.
- Keep a dependency licence list in `docs/` and update it whenever a library is added or removed.
- This is the practical convention, not legal advice.

## Open questions

These are unresolved. The first group needs the owner. The second is for the build to find out.

**For the owner:**

- [ ] Confirm or change each row under Proposed defaults.
- [x] Outpost or Outpost Mail for the repository and app identifier? Outpost Mail; the window title is Outpost.
- [x] Which Linux distribution and desktop is the main test machine? None yet: WSL2 (Ubuntu 26.04) is the only Linux available as of 17 Nov 2026.
- [ ] Is there a Microsoft 365 account and a Workspace account available for milestone 7?
- [ ] Supply a set of real conversations for the threading tests, kept outside the repository.

**To find out during the build:**

- [ ] Does Tauri pass the shell test on Linux? (Windows: the measured half passed on 17 Nov 2026, see `docs/shell-test-results.md`. Linux: only WSL is available; the measured half passed there too. Tauri adopted provisionally on 17 Nov 2026, to be re-tested on a native Linux desktop. The by-eye half is still to do.)
- [x] Does the owner's host allow cPanel API calls with a mailbox login, a token, or neither? Neither (17 Nov 2026). The add-on is out of scope.
- [ ] Does the owner's mail server record sender-check results on incoming mail, and under what server name?
- [ ] How do Windows and common antivirus tools behave with several hundred thousand files in one folder? (Partly answered 17 Nov 2026: holding and listing 352,000 files in one folder is fine; creating small files is about 60 times slower than on Linux, probably Defender. See `docs/performance-results.md`. Still to test: a Defender exclusion, and other antivirus tools.)
- [x] Is there a maintained Rust IMAP library that meets the sync requirements? Decided 17 Nov 2026: build Outpost's own sync layer on `imap-codec`, see `docs/imap-library-evaluation.md`. To be confirmed by a spike against Dovecot at the start of milestone 2.)
- [ ] How large is the search index for an 80 GB mailbox, and how long does the first build take? (Synthetic answer 17 Nov 2026: 2.4 GB of index and a 4-minute rebuild for a million messages of nearly pure text, 6.9 GB. Real mail, with attachments, is still to be measured.)
- [ ] The Google and Microsoft sign-in details listed under Accounts and sign-in.

**Provenance.** This doc was written from a planning conversation. The statements about Google, Microsoft, cPanel and Tauri come from the linked pages as read on 17 November 2026. Everything else is design intent that has not been built or tested.
