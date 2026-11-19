The first release of Outpost Mail: a desktop mail program in the classic
three-pane layout, for ordinary IMAP mailboxes, on Windows and Linux.

## Installing

| System | File |
| --- | --- |
| Windows 10 or 11 | `Outpost-Mail-0.1.0-windows-x64-setup.exe` |
| Debian, Ubuntu, Mint | `outpost-mail_0.1.0_amd64.deb` |
| Fedora, openSUSE | `outpost-mail-0.1.0.x86_64.rpm` |
| Other Linux | `Outpost-Mail-0.1.0-x86_64.AppImage` (make it executable and run it) |

**The installers are not code-signed.** On Windows, SmartScreen will say
"Windows protected your PC" the first time: choose "More info", then "Run
anyway". To check that your download is the file that was built here, compare
its SHA-256 checksum with `SHA256SUMS.txt`.

The Windows installer needs no administrator rights and installs for you
only.

## What it does

- Any IMAP mailbox, several at once. Gmail and similar need an app password;
  signing in through the provider's own page (OAuth) is not built yet.
- Mail is kept on your computer as one standard file per message, readable
  by other programs, with fast search over all of it.
- Conversations as one row each, opening into a tree; reading without the
  quoted text.
- Shows whether your mail server verified each sender, and holds messages
  that failed at arm's length: links off, attachments off.
- Pictures from the internet are blocked until you allow a sender.
- Calendars, tasks and contacts from CalDAV and CardDAV servers; meeting
  invitations answered from the message.
- Light, dark and other colour schemes; your choice of fonts and sizes.
- No telemetry. Outpost talks to your mail and calendar servers, to DNS when
  checking a signature, and once a day to GitHub to see whether a newer
  version is out (which can be switched off).

## Known gaps

- No OAuth sign-in (Microsoft 365 and Gmail without an app password).
- Messages are written as plain text only.
- No rules, no import from other programs, no unified inbox yet.
- English only.

Licences of the libraries Outpost is built with are in
`THIRD-PARTY-NOTICES.txt`, which is also installed with the app.
