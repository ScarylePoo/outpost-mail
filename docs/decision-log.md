# Decision log

Newest first. One entry per decision: what, why, and whether the owner has
confirmed it. The handoff doc holds the decisions made before the build
started.

Status words: **Decided** (owner agreed), **Proposal** (default in use, owner
has not reviewed), **Finding** (a test result).

## 2026-11-18 · Milestone 9: hardening and release (in progress)

Started on the owner's instruction, on Fable 5.2 as the handoff doc asks.
**Proposals** unless marked otherwise. The milestone is **not closed**: its
bar includes a week of daily use by the owner and the owner accepting the
written-down gaps, and no release has been published.

### Found in the owner's first day of use (19 November 2026)

The owner reported five things after using the 0.1.0 build on their own
mail. Two were real faults that no test had covered.

- **Links in formatted messages never worked in the app.** The message is
  shown in a frame that may run no scripts, and browsers extend that to
  click handlers attached from outside the frame: they are silently never
  called. The handler that was meant to show a link's destination therefore
  never ran, and the app's own content policy stopped the frame from
  navigating, so a click did nothing at all. *Decided, to keep the frame
  locked:* the reading pane reads each link's position out of the frame and
  lays an invisible link of the same size over it, outside the frame. That
  one answers the click and shows the destination as before. The frame still
  runs nothing.
- **Addresses in plain text were not links.** Plain-text messages, and the
  reduced text of a conversation, showed `https://…` as plain text. They are
  now found (web addresses, `www.` names, email addresses) and behave like
  any other link: destination first, then Open. Not in a message that failed
  its sender checks while "disable failed messages" is on.
- **"Always load pictures from…" did not load them.** The frame's own
  content policy allowed the pictures, but a frame also inherits the policy
  of the window it sits in, and the window's allowed pictures only from the
  app itself. *Decided:* the window's policy now allows pictures over http
  and https. The window never shows mail outside the frame, and the frame's
  own policy and the core's sanitiser still decide per message, so nothing
  loads for a sender who has not been allowed. A picture address written
  without a scheme (`//host/picture.png`) is now fetched over https rather
  than dropped.
- **Decided (owner): read messages keep a firm sender line.** Unread: sender
  and subject bold. Read: sender semibold, subject regular. Before, a read
  row had no emphasis at all.
- **Decided (owner): the space between messages in the list is a setting**
  (Appearance, "Space between messages", six steps), and the normal value
  is a little roomier than before (10 px above and below, was 8).
- **Calendar entries were unreadable in the dark schemes.** An entry's
  background was its calendar's colour mixed with white, under the scheme's
  light text. *Rule from now on, asked for by the owner:* a colour that
  comes from outside the scheme (a calendar's colour) is never used with a
  fixed assumption about what is behind or on top of it. A tinted
  background is mixed with the scheme's own page colour, so the scheme's
  text stays readable on it; text on the full colour is black or white,
  whichever contrasts more with that colour (`ui/src/organizer/colors.ts`).

### Decided (owner, 18 November 2026): colour schemes, fonts and sizes

The owner asked for colour schemes and for the fonts and font sizes to be
adjustable. The handoff doc had a dark theme as "later"; this brings it
forward. What was built:

- **Six colour schemes**: Match the system (light or dark, following the
  desktop, and changing with it), Light, Dark, Paper (warm, low-glare),
  Slate (dark with a blue cast) and High contrast.
- **Seven accent colours**, each with a lighter shade for the dark schemes.
- **The window's font**: the built-in Inter, or any font on the computer.
  The list offers the well-known fonts this computer actually has; any other
  can be typed by name.
- **Size**, 80% to 200%. *This zooms the whole window* rather than changing
  only the letters. The message list works out every row's height from
  fixed measurements, and zooming keeps all of them in step; changing only
  the font size would have meant re-deriving each one. The owner asked for
  "font sizes"; if separate control of text and spacing is wanted, that is
  a later change.
- **Messages**: the font and text size for reading, and the font for
  writing. These are separate from the window's.

Every colour in the style sheet is now a named value, so a scheme is one
block that sets them. (Proposal) **Formatted (HTML) messages are always shown
on a light page**, whatever the scheme: mail sets its colours piecemeal, dark
text with no background being the commonest case, and recolouring it cannot
be done reliably. Plain-text messages and the conversation view follow the
scheme.

(Proposal) **Appearance is not guarded by the settings password.** The
password exists so that someone cannot be talked into switching protection
off; making the text larger weakens nothing, and the people the password
protects are the ones likeliest to want larger text.

A font's name ends up inside a style sheet, so the core strips everything
from it but letters, digits, spaces and `. - _`.

### Decided (owner, 18 November 2026): the settings page is split into sections

The owner found the one long page "a giant wall". Settings now has a list
of sections down the left (General, Appearance, Protection, each mail
account, each calendar account, Keyboard shortcuts, About and updates), and
shows one section at a time, its settings grouped on cards under small
headings. It covers the list and reading pane while open, for the width.

The handoff doc's bar was "one searchable window with nothing nested". Both
halves still hold: the search box looks through every section at once and
shows whatever matches, and no section opens another. What changed is that
the page is no longer all visible at once.

### One copy of the app at a time

Two copies syncing the same mail would each undo the other's work. Two
defences:

- The core takes a lock on `outpost.lock` in the data folder and refuses to
  start without it (`AlreadyRunning`). The operating system releases the
  lock if the app dies, so a crash never leaves the folder locked.
- The shell (`tauri-plugin-single-instance`) notices the second start
  first, and brings the running window forward instead. A copy pointed at
  its own data folder with `OUTPOST_DATA` is deliberately separate.

**Finding:** adding the lock showed that dropping the core did not stop its
background work: the sync tasks held the core themselves, so it was never
actually dropped, and in the tests an "old" core carried on syncing beside
its replacement. The app's handle on the core now owns the running of it:
when the last such handle goes, every sync is told to stop and the data
folder is released, whatever background work still holds.

### A damaged index is set aside and rebuilt

**Finding:** an index file that was not a readable database (overwritten,
cut short) stopped the whole app from starting. The index holds nothing the
mail files and the server cannot restore, so it is now renamed to
`index.sqlite.damaged`, an empty one takes its place, and the mail is read
back in, as after an upgrade. Not covered: damage that only shows in the
middle of a query, later. Checking the whole file at every start would take
minutes on a large mailbox.

Lost with a damaged index, as with an upgrade: changes made offline and not
yet sent to the server, and which messages were marked "I trust this".

If the core cannot start at all, the app now says why in a message box
instead of simply not appearing.

### A local log

`<data>/log/outpost.log`, one line per event, at most three files of 1 MB.
In it: starts and stops, each account's state when it changes (offline and
why, sign-in refused, up to date again), update checks, and where a crash
happened. Never in it: passwords, or the subject, sender or text of any
message. It does name accounts and servers. Settings > About has a button
that shows the file. The handoff doc's "sync status panel" is still the
status bar.

### Update check

As the handoff doc decided: check GitHub releases and notify; the user
installs by hand. Details in `docs/release.md`. (Proposal) It is **on by
default**, with a switch in Settings that says exactly what is sent. The
README's and About page's "talks only to your mail servers" now names
GitHub too.

### Installers and release

Built by a GitHub workflow from a version tag, onto a **draft** release that
a person publishes. Not code-signed, per the handoff doc. The Windows
installer is per-user. See `docs/release.md` for what each file does and
what has not been verified.

The version is now 0.1.0. **No tag has been pushed and nothing has been
released**: that waits for the owner's week of use.

### Default mail program

Built with the installers, as milestone 8 left it: the installers register
Outpost for email links, a button in Settings starts the system's way of
choosing it, and a `mailto:` address opens a new message. Only recipients,
subject and text are taken from the address.

### Licence notices ship with the app

`tools/license-notices.py` gathers the licence texts of all 475 packages
linked into the app (Windows and Linux together) into
`THIRD-PARTY-NOTICES.txt`, which the release build installs with the app and
attaches to the release. Still nothing copyleft except six indirect MPL-2.0
packages, used unmodified. `cargo about` was the intended tool; the script
does the same job with what a build already has.

### Performance at a million messages

See `docs/performance-results.md` for this milestone's run.

### Gaps, written down for the owner to accept or not

Milestone 9 is done "when the targets are met or the gaps are written down
and accepted by the owner". The gaps:

1. **No installer has been run.** Built, contents inspected, never
   installed. Windows registry entries, the uninstaller and default-app
   registration are untested.
2. **Not seen by anyone on a real desktop:** the tray icon, notifications,
   start minimised, close to tray, printing, the start-failure message box,
   bringing the window forward on a second start, and opening from a
   `mailto:` link in another program. The automated run cannot see these.
3. **A week of daily use** has not happened.
4. **OAuth sign-in** (milestone 7) is parked. Gmail works with an app
   password (the owner confirmed this on 18 November 2026 with a personal
   account); Microsoft 365 accounts that only allow OAuth cannot be added.
5. **Features the handoff doc lists for version 1 that are not built:**
   rich-text composing, drafts saved as you type, several identities per
   account, per-folder views and columns, Favorites and All inboxes, sync
   choices (headers only, skip large attachments, exclude a folder), import
   from Maildir and mbox, a sync status panel, editable shortcuts.
6. **Changing and sending mail on real providers** has been used by the
   owner but not systematically tested; the automated tests run against a
   local Dovecot.
7. **Index damage found mid-query** is not recovered from automatically.
8. **macOS** is not built.

### Dependencies added

- `tauri-plugin-single-instance`, `tauri-plugin-dialog` (both MIT or
  Apache-2.0). The dialog plugin brings `rfd` (MIT) and `tauri-plugin-fs`.

## 2026-11-18 · Phase two: calendar, tasks and contacts

**Started early on the owner's instruction (18 November 2026).** The handoff
doc says phase two is not to begin until version 1 is in daily use; the owner
asked for it now. Version 1's own open items (milestones 7 and 9, and the
real-mail trials of 4 to 6) are unchanged by this.

**Proposals** unless marked otherwise. Tested against Radicale, a real
CalDAV and CardDAV server run privately by the tests. Not tried against
Nextcloud, Fastmail, iCloud, Google or the owner's own server.

### Shape

A calendar-and-contacts account is a server address plus a sign-in, in a
list of its own, separate from mail accounts (Decided in the handoff doc).
Outpost finds the calendars, task lists and address books on it; each can be
switched off and given a colour. Everything merges in the interface: one
calendar with every calendar overlaid in its colour, one task list, one
people list.

The bottom-left switcher (Mail, Calendar, People, Tasks; Ctrl+1 to 4) is now
shown. The core is the same core: the new commands and events go through the
same interface as mail, and the passwords through the same credential store.

### Stored as standard files, as mail is

Each event and task is its own `.ics` file and each contact its own `.vcf`,
exactly as the server gave it, in a folder per calendar or address book
under `dav/<account>/`, with a small SQLite index beside them.

### A file is changed one line at a time

Changing an event's title replaces the line that holds the title. Every
other line, including ones Outpost does not understand (attendees, alarms,
another program's private fields, a contact's photo), is kept exactly as it
was written. Reading for the index is by line too. One thing is left to a
library: working out when an event happens, time zones and repeats included
(`calcard`, by the authors of `mail-auth`). The handoff doc says not to
write repeat expansion by hand, and it was not.

- **Found and worked around:** `calcard` 0.3.14 treats a repeating event
  whose start is written in UTC as local time, shifting every occurrence by
  the computer's offset from UTC. Single events are not affected. Such
  events are expanded with UTC as the default zone, which gives the right
  moments; a test (`repeats_written_in_utc_stay_in_utc`) holds this in place
  and will show if a later version of the library changes. Worth reporting
  upstream.
- Text is escaped when written. An unescaped comma made the test server
  silently keep only the text before it.

### Its own small HTTP client

About 300 lines over the TLS stack already used for mail, for the reason
the SMTP client was written: certificates are checked exactly as for mail
and nothing else comes into the build. One request per connection, which is
slower than keeping connections open and simple to be sure of.

The user's name and password are sent only to the server the user named. A
redirect or a link to a different server is refused with a message saying
where it pointed, not followed. An address typed with `http://` is used
unencrypted, with a warning on the form; anything else is `https`.

### Sync, and never overwriting what has not been seen

One pass every five minutes, and at once when the user changes something:
find the collections; send what was changed here; then for each collection
whose change tag has moved, compare versions and fetch, replace or remove.

Every save and delete names the version it was made against, and the server
refuses it if that is no longer current. When that happens **the server's
version is kept, the local change is dropped, and the user is told** in the
status bar ("... was changed somewhere else at the same time. That version
was kept and your change was not saved."). This loses the user's edit rather
than someone else's, and says so. Merging the two, or keeping both, is not
attempted.

What the user does shows at once and is sent in the background; with the
server unreachable it waits and nothing is lost (tested).

### What can be edited

- **Events:** title, start and end, all day, place, notes; created and
  deleted. Times are written in UTC.
- **Repeating events are shown and expanded correctly but their rule cannot
  be made or changed here.** Editing a repeating event changes the whole
  series and the editor says so; one occurrence cannot be changed alone.
  Outpost cannot create a repeating event.
- **Tasks:** title, due day, notes, done; a due *time* set elsewhere is kept
  unless the day is changed.
- **Contacts:** name, email addresses, phone numbers, organisation, note.
  Labels such as "work" on an address that was already there are kept.
- Reminders and alarms are kept in the file and not shown or sounded.

### Meeting invitations

A message carrying a calendar file shows it as a card: what, when, where,
who organised it, with Accept, Maybe and Decline. Answering puts the event
in the chosen calendar (replacing an earlier copy of the same event, so
changing one's mind does not double it) and mails the answer to the
organiser as a standard calendar reply, from the account the invitation
arrived in. Declining removes it from the calendar and still tells the
organiser. A cancellation is shown as one, with "Remove from my calendar",
which mails nothing.

- The calendar is checked before anything is mailed: an answer that has been
  sent cannot be taken back.
- An invitation on a message that failed its sender checks cannot be
  answered.
- Not done: sending invitations, updated invitations replacing the stored
  event by themselves (the user answers again), counter-proposals, and
  showing other attendees' answers.

### Not done yet in phase two

- **Tried only against Radicale.** Other servers differ in discovery and in
  what they accept; the first real server is the test that matters.
- Sign-in is a password (or an app password). No OAuth, so Google needs an
  app password and may not work at all for calendars.
- Month view only: no week or day view, no dragging.
- Creating or editing repeat rules; changing one occurrence; time zones
  other than UTC on events made here.
- Reminders.
- Address suggestions in the composer from contacts. People has a Write
  button; the composer's To field does not yet look contacts up.
- Creating, renaming and deleting calendars from Outpost.
- Contact photos, postal addresses, birthdays (kept in the file, not shown).
- Each request opens its own connection; a large first sync is slower than
  it need be.
- The index is rebuilt by syncing, not from the files alone.

### Dependencies added

- `calcard` 0.3 (Apache-2.0 or MIT), with `chrono` and `chrono-tz`.
- `quick-xml` (MIT), `httparse` (MIT or Apache-2.0), `iana-time-zone` (MIT
  or Apache-2.0).
- For tests only: Radicale (GPL-3.0), a separate program the tests start.
  It is not linked into or shipped with Outpost.

## 2026-11-18 · Milestone 8: settings, shortcuts and system integration

**Order (Decided, owner, 18 November 2026):** milestone 7 (OAuth) is parked
until there are client IDs to test with; milestone 8 was built next, on
Fable 5.2 rather than switching models; phase two follows.

**Proposals** unless marked otherwise.

### One settings page, searchable, one level deep

Settings opens in the reading area (Ctrl+comma, the toolbar, the File menu).
Everything is on one scrolling page under plain headings; the search box
narrows the page as you type and nothing opens a second level. Each account
is a heading on the same page: incoming server, password, sending and
identity, signature, whose sender checks are believed, and Remove. The
protection panel from milestone 6 is now a section of it. The menus' "Protection
settings", "Account settings" and "Keyboard shortcuts" open the page already
narrowed to that topic.

A switch that needs the settings password shows its old state until the
change has really been made.

### The settings password guards the whole page

With a password set, the core refuses to change any general or protection
setting, to change an account's server, or to remove an account without it.
Naming the server whose sender checks are believed is guarded too. Two things
are not: setting up sending and entering a mailbox password, which are
needed to keep mail working.

### Removing an account

Deletes the account's settings, stored password and downloaded mail from
this computer, after a yes-or-no question. Nothing is changed on the server.
The directory is marked first and deleted second, so if Windows will not
delete open files there and then, the rest goes at the next start and the
account never reappears. Mail that is only browsed (no server details) cannot
be removed this way.

### Shortcuts come from one table

`ui/src/shortcuts.ts` lists every shortcut. The main window acts on the
table and the settings page prints it, so the two cannot disagree. Outlook's
keys are used where it has one (Ctrl+N, Ctrl+R, Ctrl+Shift+R, Ctrl+F, Ctrl+Q,
Ctrl+U, Insert, Delete, Backspace, F9, Ctrl+E, Ctrl+P, Ctrl+Enter).
Shortcuts are not yet editable.

### The menu bar is drawn in the window

A plain File / View / Message / Tools / Help bar, drawn by the interface
rather than by the operating system: it looks the same on Windows and Linux
and every entry calls the same function as its toolbar button and shortcut.

### Tray icon and notifications

- The tray icon's tooltip (and, where the desktop shows it, the text beside
  the icon) carries the unread count of the inboxes. A click opens the
  window; the menu has Open and Quit. Drawing the number onto the icon
  itself is not done.
- "Keep running in the tray when the window is closed" and "Start
  minimised" are off by default. If the desktop has no tray, the window is
  never hidden: there would be no way to get it back.
- A notification is shown for unread mail that arrives in an inbox while
  the app is running. Mail that came while it was closed is not announced
  one by one; the tray count covers it. Mail a blocked sender's rule moved
  to junk is not announced.

### Smaller things

- **Signature**, one per account, put under what the user will write and
  above anything quoted. A reopened draft is left as saved.
- **Undo-send delay** is a setting: off, or 5 to 60 seconds.
- **Lines of preview** in the list: none to three.
- **Sent, Drafts and Outbox show who a message is to** in place of the
  sender (Decided in the handoff doc).
- **Save message as a file** writes the message to Downloads as `.eml`,
  byte for byte as received. **Print** prints the open message or
  conversation.

### Every setting the handoff doc names, and where it stands

"Done when: every setting in this doc exists." It does not, yet, and the gap
is features, not switches: a setting is only on the page if something obeys
it. Dead switches were not added.

| Setting in the handoff doc | State |
| --- | --- |
| The seven protection settings | Done (milestone 6), now on the settings page |
| Which server's checks to believe, per account | Done |
| Account: servers, sign-in, identity, signature | Done. One identity per account, not several |
| Account: sync choices (headers only, skip large attachments, exclude a folder) | **Not built.** The sync engine downloads everything |
| Account: OAuth sign-in | Parked with milestone 7 |
| Account: cPanel add-on | Dropped in milestone 0 |
| Undo send delay | Done |
| New mail notifications | Done |
| Tray icon with unread count | Done as tooltip and label, not drawn on the icon |
| Start minimised | Done |
| Register as the default mail app | **Not built.** Needs the installers (milestone 9) |
| Check for updates | **Not built.** Belongs with the first release (milestone 9) |
| Composer: rich text, or plain text only | **Not built.** The composer is plain text only |
| Drafts saved as you type | **Not built.** Drafts are saved when asked |
| Several identities per account | **Not built** |
| Views: named layouts, reset this folder, apply to all | Partly. Conversations or messages, and preview lines, apply everywhere. No per-folder layouts, columns, sorting or grouping |
| Favorites and All inboxes | **Not built** |
| Rules | Out of version 1 per the handoff doc |
| Search scope | Done in the list (this folder, or all of the account) |
| Logging and a sync status panel | **Not built.** Status is the status bar |
| Import (Maildir, mbox) | **Not built** |
| Printing and export | Done, basic |
| Dark theme, density | Later, per the handoff doc |
| Language | English only, per the handoff doc |

### Not done yet in milestone 8

- The rows marked "Not built" above.
- Tray, notifications, start-minimised and close-to-tray are built and
  compile on both systems, and the app starts with them, but nobody has
  looked at them on a real desktop: the automated run cannot see a tray or a
  notification. Printing likewise.
- No native (operating-system) menu; no editable shortcuts.

### Dependencies added

- `tauri-plugin-notification` (MIT or Apache-2.0) and Tauri's `tray-icon`
  feature. On Linux the tray needs `libayatana-appindicator3` installed.

## 2026-11-18 · Milestone 6: sender verification

**Proposals** unless marked otherwise. Built and tested against made-up mail
with known answers. It has not been run on real mail from the owner's
provider: which header that server writes, and under what name, is the
first thing to confirm (see "Not done yet").

### Only the topmost result header from the account's own server is believed

The receiving server records SPF, DKIM and DMARC results in an
`Authentication-Results` header. Anyone can put such a header in a message
they send. Servers add headers at the top, so nothing a sender wrote can be
above what the receiving server added. Outpost therefore reads one header
only: the topmost one carrying the name of the account's own server. Lower
headers under the same name are ignored even when they look genuine.

- Cost: a server that spreads its results over several headers (one per
  check, as some Postfix setups do) shows fewer results. It can show less
  trust than is due, never more.
- **Known limit.** If the account's server adds no result header at all to
  some message, and does not strip forged ones bearing its name (the
  standard says it must), a forged header would be the topmost and would be
  believed. Whether the owner's server strips them needs checking on real
  mail. The local signature re-check is an independent second opinion for
  exactly this case.

### Which server to believe is worked out, then confirmed

The name is taken from the newest 500 messages: the server that wrote the
topmost result header on at least three of them and on at least 60% of
those that have one. It is used at once, marked as a guess, and shown in
the Protection panel for the user to confirm or correct. Once confirmed it
is never replaced by a guess. With no name, every message is "not checked".

Each message's verdict is stored with the name of the server that wrote it,
so changing the believed server changes every list at once without reading
the mail again.

### What counts as verified, and as failed

- **Verified** (the only state with a green check): a DMARC pass for the
  From domain; or a DKIM pass by the From domain, its parent or its child;
  or an SPF pass for such a domain. A pass for any other domain is shown for
  what it is and verifies nothing.
- **Failed** (red banner, warning in the list): a DMARC failure, whatever
  else passed; or an SPF hard failure with nothing vouching for the message.
- **Not verified** (neutral): checks were recorded but none ties the message
  to its From domain. A broken DKIM signature or an SPF soft failure alone
  lands here, not in Failed: mailing lists and forwarding cause both.
- **Not checked** (neutral): nothing usable recorded.
- A message with two From lines, or two addresses in one, is never
  verified: the server may have checked one and the reader be shown another.

"Parent or child" is by name only. Two different names under one parent
(`a.example.com`, `b.example.com`) are not treated as related, because
without the public-suffix list that cannot be told from `a.co.uk` and
`b.co.uk`. This errs toward not verified. The handoff doc's table has three
states; "not verified" is a second neutral one, added so that "your server
looked and found nothing tying this to the sender" is not worded the same
as "nobody looked". **Decided** (owner, 18 November 2026: "probably ok").

### Signatures are checked again locally, and "could not" is not "failed"

`mail-auth` (Stalwart's library) verifies DKIM signatures, with keys fetched
through the computer's own DNS settings and kept for up to an hour. Three
outcomes: valid; does not match the message as stored (the key is there and
the message is not what was signed); could not re-check (key withdrawn or
gone, signature expired, lookup failed), which is said to be normal for old
mail and never shown as a failure.

- **Decided (owner, 18 November 2026): a signature Outpost verifies itself
  earns the green check.** When the server did not verify a message, a
  valid signature made by the From domain (or its parent or child) shows
  "Verified sender", and the details say it was Outpost's own check. It
  never overrides a failure the server recorded, and never applies to a
  message naming more than one sender. This extends the handoff doc's
  definition of "verified". The mark appears when the message is opened; the
  list and the conversation tree show only what the server recorded.
- It therefore runs whenever a message the server did not verify is opened
  (and for the details of any message). It asks DNS about the signing domain, which tells
  the DNS server roughly what following a link in the message would. It has
  its own switch.
- Pure-Rust cryptography was chosen over the library's default (which
  needs a C toolchain and assembler on Windows).

### Outpost's own warnings

All worked out from the mail already here, when a message is opened:

- **First message from this sender**: no earlier message is from, or to,
  the address. Mail to someone counts, so a reply from a person the user
  wrote to first is not flagged.
- **A familiar name on a new address**: the address is new, and earlier mail
  under the same display name came from another address, which is shown.
- **Lookalike domain**: the sender's domain has next to no history here and
  resembles (one or two characters out, characters that pass for each
  other, or the real name with something attached) a domain with real
  history: three messages at least and more than three times the
  newcomer's. Short names are compared exactly only, since `abc.com` and
  `abd.com` are simply different companies.
- **Replies go elsewhere**: the reply address is at a different domain.

### A failed message is held at arm's length

With that protection on (the default), a failed message's links are removed
in the core, not just disabled in the window: each becomes its text followed
by where it pointed. Remote pictures are never loaded for it, and the core
refuses to save its attachments. The view follows the mock-up: banner, name
shown / actually sent from / replies would go to with mismatches marked,
each check on a line with a sentence, which server checked and what the
local re-check found, then Delete, Move to spam, Block sender and a low-key
"I trust this message". Trusting is per message, is remembered, can be
withdrawn, and leaves the banner in place.

### Protection settings, each its own switch

Kept in `settings.json` in the data folder; a damaged file reads as the
defaults, which are all on. Seven from the handoff doc plus one added
(signature re-checks). Pictures can be allowed per sender from the message
itself; blocked senders' mail is moved from the inbox to the junk folder as
it arrives (through the normal action queue, so it happens on the server
too).

The password is stored as a salted PBKDF2 hash (210,000 rounds). With one
set, the core refuses to change a setting or to trust a failed message
without it; withdrawing trust needs none. **It is a guard against someone
at the keyboard, not against someone who can edit files on the computer**,
who could delete it from the settings file. That fits the stated purpose
(parents being talked into switching protection off) and is said in the
panel.

### Fixed along the way: new mail missed while the connection was busy

Found by the blocked-sender test. The server only pushes changes that
happen while the client is waiting. Mail arriving while the connection was
replaying the user's actions was announced in the middle of that work and
then not seen until the next poll, up to ten minutes later; so after
deleting a message, the next new one could be late. Before waiting, the
inbox's position is now compared with where the last sync of it ended, and
the inbox is synced first if they differ. A regression test delivers mail
in the middle of a burst of changes; it failed every time before the fix.

### Not done yet in milestone 6

- **Not run on real mail.** Needs the owner: which result header their
  server writes, under what name, whether it is always present, and whether
  forged ones are stripped. The known-result set is made up from the
  standard's formats and common servers' output.
- ARC results (forwarded mail vouched for by the forwarder) are ignored.
- Results spread over several headers by one server are read from the top
  one only (see above).
- Internationalised domains are compared as written; mixed-script
  lookalikes are not detected.
- The warning label in the list is for the message that stands for a
  conversation; a failed message deeper in a conversation is marked in the
  tree and in the reading pane.
- The Protection panel is a plain panel; milestone 8 moves it into the
  settings window.

### Dependencies added

- `mail-auth` 0.15 (Apache-2.0 or MIT) with `hickory-resolver` for DNS and
  the RustCrypto `rsa`, `ed25519-dalek`, `sha1`, `sha2` crates. Note for the
  release review: `rsa` has an open advisory (RUSTSEC-2023-0071, a timing
  side channel in private-key operations); Outpost only verifies with public
  keys, which it does not affect.
- `ring` used directly (already in the build through rustls) for the
  password hash.

## 2026-11-18 · Milestone 5: conversations

**Proposals** unless marked otherwise. Built and tested on made-up mail and
the throwaway Dovecot. It has not yet been tried on the owner's messiest real
conversations, which is what the handoff doc asks for before this milestone
closes.

### How messages are put into conversations

A message joins the conversation of anything it names in `In-Reply-To` or
`References`, and of anything that names it, whichever arrives first. Two
conversations that turn out to be one are merged. Copies of a message in
several folders (Inbox and Sent, say) are one message of the conversation.

A reply that names nothing (some phones and ticket systems) is **placed by
subject**: under the newest message with the same subject, less `Re:`-style
prefixes, from the last 30 days, that shares a participant with it. Such a
link is recorded as a guess and shown as one (dashed connector, "Placed by
subject" chip), never passed off as certain. A message that is not a reply
(no `Re:`) is never placed by subject, so two unrelated "Hello" messages stay
apart. A forward starts a new conversation.

### Conversations are recorded by sort key, not row number

A conversation, and each message's parent, is named by a message's sort key,
which is fixed when the message is first seen. That lets a rebuild work
conversations out in memory before rows get their final positions, so rows
are written once, complete and in list order. (Threading a million messages
one at a time was far too slow; in memory it is seconds.)

### The list shows one row per conversation, served by the index

In each folder the newest message of each conversation is marked as its
head, and the conversation list is simply the folder's heads: the same kind
of index scan as the message list, so it is as fast at any size. Triggers
keep the mark right when a message arrives, is linked, moves or goes.

A row shows who wrote in the conversation (the user as "You"), how many
messages it has in every folder, and how many are unread. The row is where
the conversation's newest message *in that folder* is: a reply the user sent
does not move the conversation in the inbox, a reply received does.

Opening a row shows the tree: each reply under what it answers, connector
lines, nesting drawn five deep and flat beyond, a divider where two or more
days passed, and chips for people added or dropped and for attachments.
Every line has a fixed height, so the list still never measures anything.
The list header switches between Conversations and Messages; the choice is
remembered on this computer.

### The reading pane shows a conversation reduced

Each message is shown without what it quotes and without its signature, with
"Show quoted text (N lines)", "Show signature" and "Show original" to bring
each back. Three modes: **This branch** (the selected message and what it
answers, up to the start), **Whole thread** (the tree order) and **By date**.
Each message says whom it replies to and who was added or dropped.

Reduction is conservative: when the rules are unsure, text stays visible.
Quoted history is found by `>` marks, by "On … wrote:" lines, by the header
block Outlook puts above a quote, and by comparing with the text of the
message being answered (which catches quotes with no marks at all). In a
reply, a dated "On … wrote:" line starts the history even when nothing after
it is marked, as in mail written as HTML only.

### A rebuild compacts the index file

Measured: after a rebuild the index's pages were scattered through the file
and a first, cold read of a 350,000-message folder's layout took 360 to
430 ms against a 100 ms target. Rewriting the file at the end of the rebuild
(`VACUUM`) brings it to 35 to 65 ms, for about 20 seconds more at a million
messages. Results in `docs/performance-results.md`.

### Not done yet in milestone 5

- Not tried on real, messy conversations (needs the owner's mail).
- Keyboard movement inside an open tree; arrow keys move between rows only.
- Actions on a conversation row (delete, move, mark) apply to the message
  shown for it, not to the whole conversation.
- Search results are still single messages.
- An index from the previous version is discarded and rebuilt from the mail
  files on first start (as before; proper migrations are still owed before
  release).

## 2026-11-18 · Milestone 4 (in progress)

**Proposals** unless marked otherwise. From this milestone the app changes
mail on the server. Everything here was developed and tested against the
throwaway Dovecot and a fake outgoing server, never a real account.

### What the user does is applied here first and replayed to the server

Marking, moving, deleting and saving take effect in the local store at once
and are recorded in a queue in the same transaction. The sync engine replays
the queue in order, at once when online, later when not. A message moved or
created locally has no server UID until its action has been replayed; actions
queued behind it refer to it by its row and get the UID filled in when the
server reports it. This is what lets a message be moved three times and
re-flagged in between while offline, and arrive as one copy in the right
place (tested).

Every replay is safe to repeat. Flags are idempotent, moving or expunging a
UID that is gone does nothing, and before a message is uploaded again the
folder is searched for its Message-ID. Sync recognises a message it put in a
folder itself, by Message-ID, instead of downloading it as new.

Sync and the queue do not fight: the queue is replayed before anything is
read back, and sync leaves alone any message that has a change on the way.

### An action the server refuses is undone and reported, not retried

A refused move puts the message back where it was. The user sees one line in
the status bar. The actions queued behind it still run.

### Deleting

Delete moves to the account's trash folder. Deleting from the trash removes
the message for good, after a yes-or-no question. Removal is always by UID:
this client has no folder-wide EXPUNGE, so it cannot remove messages another
client had marked for deletion (tested). On a server that cannot expunge by
UID, the message is marked deleted and left; and any message marked deleted,
by anyone, is treated as gone until the mark is removed.

### Sending

- **Own SMTP client** (`smtp.rs`, about 250 lines) rather than a library, so
  that certificates are checked exactly as for IMAP and the outcomes are kept
  apart. TLS from the start (port 465) and STARTTLS (port 587) are supported;
  a server that will not encrypt is never used when encryption was asked for.
- **Three outcomes, three behaviours.** Not reachable: the message waits in
  the outbox and is retried after a growing pause, without holding up other
  actions. Refused (wrong password, refused recipient): it goes back to
  Drafts with a notice. Connection lost after the message was handed over and
  before the server answered: it may have gone, so it is **never retried
  automatically**; it stays in the outbox and the user is told to check.
- **Undo send:** a sent message waits 10 seconds in the outbox (the handoff
  doc's proposal), during which Undo makes it a draft again.
- **Blind copies** are in the copy kept in Sent and removed from what is
  transmitted.
- **The copy in Sent** is uploaded by Outpost, except on Gmail (recognised by
  its capability), which files sent mail itself.
- **The outbox** is a folder that exists only on this computer, shown in the
  tree while it holds something.
- **One password.** The outgoing server is signed in to with the mailbox
  password. A separate one is not supported yet.
- Replying marks the original answered; forwarding marks it forwarded and
  carries its attachments.

### The composer is plain text

The handoff doc proposes rich text by default. The milestone asks for a basic
composer, and this one is plain text: To, Cc, Bcc, subject, body, file
attachments, save as draft. Rich text, inline pictures, signatures, several
identities, drag-and-drop and saving as you type are not built.

### Shortcuts so far

Outlook's where they exist: Ctrl+N new, Ctrl+R reply, Ctrl+Shift+R reply all,
Ctrl+F forward, Del delete, Backspace archive, Ctrl+Q read, Ctrl+U unread,
Insert flag. Opening a message marks it read.

### Several messages at once

Ctrl-click, Shift-click, Shift with the arrow keys, and Ctrl+A select several
messages; delete, archive, move, read/unread and flag then apply to all of
them (up to 5,000 at a time). Reply and forward act on the current one. A
selection of several does not survive the list refreshing, which every action
causes; the row now in the same place is selected instead, so Delete can be
pressed repeatedly.

### Folders

A folder can be created (the `+` beside the account name) and an ordinary,
empty-of-subfolders folder deleted (a button in the list header, after a
yes-or-no question). Both go through the queue, so they work offline, and
mail can be filed into a new folder before the server has heard of it. On
servers that keep every folder inside the inbox, a new top-level folder is
created inside the inbox, where it will appear beside the others. Not built:
renaming or moving a folder, creating one inside another from the interface,
and subscription management.

### Found while testing the window end to end

Two interface bugs that no core test could see, both fixed: clearing a search
left the toolbar acting on nothing although a message looked selected; and the
reading pane reloaded the open message on every list refresh, which silently
marked it read again a moment after the user marked it unread.

### Finding: the outgoing path works against real servers as far as the greeting

`outpost-sync check-smtp` connects, secures the connection and reads the
server's capabilities without signing in. Against a public provider and the
owner's host, both TLS on port 465 and the STARTTLS upgrade on 587 work with
certificates accepted. Signing in and sending for real are still untested.

### Still to do in milestone 4

- **The owner's "full day on his main account, including offline".** This is
  the milestone's "done when", and the first time any of this meets a real
  server: sending for real, and moving, deleting and flagging real mail.
- Renaming folders; saving drafts as you type; rich text; drag-and-drop.
- **Upgrades lose the queue.** The index is still discarded and rebuilt when
  its format changes, and the queue lives in it. Mail is never lost (unsent
  messages stay in the outbox as files) but queued changes would be. A real
  migration path is needed before any release.

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
