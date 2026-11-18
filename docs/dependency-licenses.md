# Dependency licences

Outpost's own code is under the Unlicense. This lists the libraries it is
built with. Update it whenever a direct dependency is added or removed.

No direct dependency is copyleft. Checked on 18 November 2026 across all 611
packages in the build (`cargo metadata`): the only ones not under a
permissive licence are indirect and under the MPL-2.0, which is copyleft
file by file: `cssparser`, `cssparser-macros`, `selectors`, `dtoa-short`
(HTML and CSS parsing, through `ammonia` and Tauri) and `option-ext`. They
are used unmodified, which the MPL allows in a program under any licence;
their notices must ship with the app. (An earlier version of this page said
no copyleft library was in use at all. That was true only of the direct
dependencies.)

Before the first release, generate the full list including indirect
dependencies (`cargo about` or `cargo deny` for Rust, `license-checker` for
npm) and ship the notices with the app.

## Shipped in the app

| Library | Used for | Licence |
| --- | --- | --- |
| `serde`, `serde_json` | Encoding commands and events | MIT or Apache-2.0 |
| `tauri` | The shell | MIT or Apache-2.0 |
| `rusqlite`, with SQLite bundled | The index and full-text search | MIT (SQLite: public domain) |
| `mail-parser` | Reading message files | MIT or Apache-2.0 |
| `mail-builder` | Writing outgoing messages | MIT or Apache-2.0 |
| `ammonia` | Making HTML mail safe to display | MIT or Apache-2.0 |
| `keyring` | Passwords in the operating system's credential store | MIT or Apache-2.0 |
| `tauri-plugin-opener` | Opening links and showing saved files | MIT or Apache-2.0 |
| `imap-codec` | Parsing and writing the IMAP protocol | MIT or Apache-2.0 |
| `tokio` | Networking and timers | MIT |
| `tokio-rustls`, `rustls` | TLS | MIT, Apache-2.0 or ISC |
| `ring` (through `rustls`) | Cryptography for TLS | Apache-2.0 and ISC |
| `ring` (directly) | Hashing the settings password | Apache-2.0 and ISC |
| `mail-auth` | Checking DKIM signatures again locally | MIT or Apache-2.0 |
| `hickory-resolver` (through `mail-auth`) | Looking up signing keys in DNS | MIT or Apache-2.0 |
| `rsa`, `sha1`, `sha2` (through `mail-auth`) | Verifying signatures | MIT or Apache-2.0 |
| `ed25519-dalek` (through `mail-auth`) | Verifying signatures | BSD-3-Clause |
| `rustls-platform-verifier` | Checking certificates against the system's trust store | MIT or Apache-2.0 |
| `solid-js` | Interface framework | MIT |
| `@tauri-apps/api` | Interface side of the Tauri adapter | MIT or Apache-2.0 |
| Inter (`@fontsource-variable/inter`) | Typeface | SIL Open Font License 1.1 |

## Build tools only

| Tool | Licence |
| --- | --- |
| `tauri-build`, `@tauri-apps/cli` | MIT or Apache-2.0 |
| `vite`, `vite-plugin-solid` | MIT |
| `typescript` | Apache-2.0 |
| `rpassword` (in `outpost-sync` only) | Apache-2.0 |
