//! IMAP: the protocol layer under the sync engine.
//!
//! - `conn`: one connection (socket, TLS, framing, commands and responses).
//! - `session`: what Outpost asks of a server, in Outpost's own terms.
//! - `utf7`: folder-name encoding.
//!
//! Parsing and serialising are done by `imap-codec`. None of its types leave
//! this module, so the sync engine does not depend on it.

pub mod conn;
pub mod session;
pub mod utf7;
mod write;

pub use conn::{ImapError, Security, Server};
pub use session::Session;
