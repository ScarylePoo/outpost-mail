# IMAP library evaluation

Milestone 0 · 17 November 2026 · **Proposal, awaiting the owner's confirmation**

## Recommendation

Build Outpost's own connection and sync layer on **`imap-codec`** (with its
companion `imap-types`), using `tokio` and `rustls` for the network. Do not
hand-write an IMAP parser, and do not adopt a full client library.

In plain terms: `imap-codec` turns bytes from the server into typed values and
typed commands into bytes, correctly, including the awkward parts. Everything
above that (which command to send when, pipelining, IDLE, reconnecting,
resync) is the sync engine, which the handoff doc says is where the effort
should go, and which no existing library does to the standard required.

The first task of milestone 2 is a one-day spike against the local Dovecot
server to confirm this before building on it. `async-imap` is the fallback.

## What was checked

Versions, release dates, licences and feature lists were read from crates.io
and repository activity from GitHub on 17 November 2026. `async-imap`'s public
API was read from its source. **No candidate has been compiled or run against
a server yet.**

| Library | Latest release | Activity | Licence | What it is |
| --- | --- | --- | --- | --- |
| `imap-codec` / `imap-types` | 2.0.0-alpha.10, 2 Nov 2026 (stable 1.0.0 is older) | Pushed 16 Nov 2026 | MIT or Apache-2.0 | Parser and serialiser only. No networking. |
| `imap-next` | 0.3.4, 18 Feb 2026 | Pushed Apr 2026 | MIT or Apache-2.0 | Thin connection state machine on `imap-codec`. |
| `async-imap` | 0.12.0, 30 Sep 2026 | Regular releases; used by Delta Chat | MIT or Apache-2.0 | Full async client. |
| `imap` (rust-imap) | 3.0.0-alpha.15, 8 Feb 2025; stable 2.4.1 | Alpha since 2021 | MIT or Apache-2.0 | Full blocking client. |
| `imap-client` (Pimalaya) | 0.3.1, 17 May 2026 | **Repository archived** | MIT | Full client on `imap-next`. |

## Against the sync requirements

| Requirement | `imap-codec` | `async-imap` |
| --- | --- | --- |
| CONDSTORE and QRESYNC | Typed support, behind the `ext_condstore_qresync` feature | `SELECT (CONDSTORE)` only. No QRESYNC, `CHANGEDSINCE` or `VANISHED` in the API; would need its raw-command escape hatch and our own parsing |
| IDLE | Yes | Yes |
| MOVE, UID EXPUNGE | Yes | Yes (`uid_mv`, `uid_expunge`) |
| SPECIAL-USE, LIST extensions, NAMESPACE, ID | Yes (`ext_list_extended`, `ext_namespace`, `ext_id`) | LIST and ID only |
| Fetching large messages in parts | Yes (typed partial fetch) | Yes (fetch items are free-form strings) |
| Tolerating misbehaving servers | A dozen named `quirk_*` switches, each for an observed server bug | Not a stated feature |
| Robustness of parsing | Fuzz-tested; the project's main focus | Parser is the separate `imap-proto` crate |
| Control over connections, pipelining, backoff | Entirely ours | The library's; one command at a time per session |

## Why not the others

- **`async-imap`** is maintained and proven, but it stops short of exactly the
  feature that makes resync fast on large mailboxes (QRESYNC), and its
  one-command-at-a-time session would have to be worked around for staged
  sync. Using it would mean writing a good part of the protocol layer anyway,
  on top of a parser we did not choose.
- **`imap-next`** would save some connection plumbing, but its last release is
  nine months old and it lags the codec's extension list (no
  `ext_list_extended`). Decide in the milestone 2 spike whether it earns its
  place or whether a small state machine of our own is simpler.
- **`imap`** is blocking and has been in alpha for five years.
- **`imap-client`** is archived.

## Risks

- **`imap-codec` 2.0 is an alpha series.** The API can change between
  releases. Pin an exact version and upgrade deliberately. The 1.0.0 stable
  release lacks most of the extension and quirk flags, so it is not a
  substitute.
- **More of our own code.** Connection handling, pipelining and IDLE are ours
  to write and test. That is the intended trade: it is also the code the fault
  tests in milestone 2 are aimed at.
- **Gmail's extensions** (`X-GM-EXT-1`) are not in the feature list. Version 1
  does not use them, and the codec can be extended or bypassed for them later.
