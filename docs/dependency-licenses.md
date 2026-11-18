# Dependency licences

Outpost's own code is under the Unlicense. This lists the libraries it is
built with. Update it whenever a direct dependency is added or removed. No
copyleft library is in use.

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
| `ammonia` | Making HTML mail safe to display | MIT or Apache-2.0 |
| `imap-codec` | Parsing and writing the IMAP protocol | MIT or Apache-2.0 |
| `tokio` | Networking and timers | MIT |
| `tokio-rustls`, `rustls` | TLS | MIT, Apache-2.0 or ISC |
| `ring` (through `rustls`) | Cryptography for TLS | Apache-2.0 and ISC |
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
