//! Bringing one calendar-and-contacts account up to date, both ways.
//!
//! One pass: find the collections; send what the user changed here; then,
//! for each collection in use whose change tag has moved, compare the
//! server's list of versions with what is here and fetch, replace or remove
//! accordingly.
//!
//! Nobody's change is overwritten without having been seen. Every save and
//! delete names the version it was made against, and the server refuses it
//! if that is no longer the current one. When that happens the server's
//! version is kept, the local change is dropped, and the user is told.

use super::client::{Dav, DavError, Kind};
use super::store::{DavStore, StoreError};

#[derive(Debug)]
pub enum SyncError {
    Dav(DavError),
    Store(StoreError),
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncError::Dav(e) => e.fmt(f),
            SyncError::Store(e) => e.fmt(f),
        }
    }
}

impl From<DavError> for SyncError {
    fn from(e: DavError) -> Self {
        SyncError::Dav(e)
    }
}

impl From<StoreError> for SyncError {
    fn from(e: StoreError) -> Self {
        SyncError::Store(e)
    }
}

/// What a pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Something here looks different now.
    pub changed: bool,
    pub sent: usize,
    pub fetched: usize,
    pub removed: usize,
    /// Things the user should be told: a change that could not be kept.
    pub notices: Vec<String>,
}

/// A name no other item will have.
pub fn new_id() -> String {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 16];
    ring::rand::SystemRandom::new().fill(&mut bytes).expect("the system has a source of random numbers");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn named(summary: &str) -> String {
    match summary.trim() {
        "" => "An item".to_string(),
        s => format!("\"{s}\""),
    }
}

/// One pass. Stops at the first failure to reach the server (everything
/// still waiting to be sent stays waiting); a refusal of one item does not
/// stop the rest.
pub async fn sync(dav: &Dav, store: &DavStore) -> Result<Outcome, SyncError> {
    let mut outcome = Outcome::default();
    let found = dav.discover().await?;
    let before = store.collections()?;
    store.set_collections(&found)?;
    let collections = store.collections()?;
    outcome.changed |= before != collections;

    // What the user did here goes first.
    let mut touched: Vec<i64> = Vec::new();
    for pending in store.pending()? {
        let Some(collection) = collections.iter().find(|c| c.id == pending.collection) else { continue };
        let known = (!pending.etag.is_empty()).then_some(pending.etag.as_str());
        let result = if pending.delete {
            dav.delete(&pending.href, known).await.map(|_| None)
        } else {
            let data = store.data(pending.id)?;
            let href = if pending.href.is_empty() {
                let extension = if collection.kind == Kind::AddressBook { "vcf" } else { "ics" };
                format!("{}/{}.{extension}", collection.href.trim_end_matches('/'), new_id())
            } else {
                pending.href.clone()
            };
            // Something sent before but with no known version (the server
            // did not say) is saved against whatever is there.
            let known = if pending.href.is_empty() { None } else { known.or(Some("*")) };
            dav.put(&href, collection.kind, &data, known).await.map(|etag| Some((href, etag, data)))
        };
        match result {
            Ok(None) => {
                store.remove(&[pending.id])?;
                outcome.sent += 1;
            }
            Ok(Some((href, etag, data))) => {
                // With no version from the server, an empty one makes the
                // listing below fetch the item as the server now has it.
                store.sent(pending.id, &href, etag.as_deref().unwrap_or(""), &data)?;
                outcome.sent += 1;
            }
            Err(DavError::Changed) => {
                store.give_up(pending.id)?;
                outcome.notices.push(if pending.delete {
                    format!("{} was changed somewhere else, so it was not deleted.", named(&pending.summary))
                } else {
                    format!(
                        "{} was changed somewhere else at the same time. That version was kept and your change was not saved.",
                        named(&pending.summary)
                    )
                });
            }
            Err(DavError::NotFound) | Err(DavError::Refused { .. }) => {
                let why = match result {
                    Err(e) => e.to_string(),
                    Ok(_) => String::new(),
                };
                store.give_up(pending.id)?;
                outcome.notices.push(format!("{} could not be saved: {why}.", named(&pending.summary)));
            }
            // Could not reach the server, or not let in: try again later.
            Err(e) => return Err(e.into()),
        }
        touched.push(pending.collection);
        outcome.changed = true;
    }

    for collection in collections.iter().filter(|c| c.enabled) {
        let on_server = found.iter().find(|f| super::client::same_path(&f.href, &collection.href));
        let tag = on_server.and_then(|f| f.ctag.clone());
        let pushed = touched.contains(&collection.id);
        if !pushed && tag.is_some() && tag == collection.ctag {
            continue;
        }
        let listed = dav.list(&collection.href).await?;
        let known = store.known(collection.id)?;
        let wanted: Vec<String> = listed
            .iter()
            .filter(|remote| match known.iter().find(|k| k.href == remote.href) {
                Some(k) => k.dirty == 0 && k.etag != remote.etag,
                None => true,
            })
            .map(|remote| remote.href.clone())
            .collect();
        let gone: Vec<i64> = known
            .iter()
            .filter(|k| k.dirty == 0 && !k.href.is_empty() && !listed.iter().any(|remote| remote.href == k.href))
            .map(|k| k.id)
            .collect();
        for batch in wanted.chunks(50) {
            for item in dav.fetch(&collection.href, collection.kind, batch).await? {
                match store.put_remote(collection.id, &item.href, &item.etag, &item.data) {
                    Ok(()) => outcome.fetched += 1,
                    // Something that is not an event, task or contact (a
                    // journal entry, a broken file): left on the server,
                    // not shown here.
                    Err(StoreError::Unreadable) => {}
                    Err(e) => return Err(e.into()),
                }
            }
        }
        outcome.removed += gone.len();
        outcome.changed |= !wanted.is_empty() || !gone.is_empty();
        store.remove(&gone)?;
        // After sending into it, the tag we were given is out of date; with
        // none recorded, the next pass looks again, which is right.
        store.set_ctag(collection.id, if pushed { None } else { tag.as_deref() })?;
    }
    Ok(outcome)
}
