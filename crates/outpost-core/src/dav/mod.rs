//! Calendars, tasks and contacts over CalDAV and CardDAV (phase two).
//!
//! - `http`: the HTTP client underneath.
//! - `client`: what a DAV server is asked: where a user's calendars and
//!   address books are, what is in each, and changes to them.
//! - `item`: reading and changing calendar and contact files.
//! - `store`: the files and their index on this computer.
//! - `sync`: one pass that brings the two into line.

pub mod client;
pub mod http;
pub mod item;
pub mod store;
pub mod sync;
