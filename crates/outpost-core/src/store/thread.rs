//! Conversations: which messages belong together, and which replies to which.
//!
//! A message is linked by what it cites: its In-Reply-To and References
//! headers name the messages it descends from. Linking happens once, when a
//! message enters the index, and covers every folder of the account, so a
//! reply sitting in Sent takes its place between the messages it answers.
//!
//! Mail arrives in any order (a first sync fetches newest first), so each
//! new message is also checked against the messages already here that cite
//! it: they are re-attached under it, and their conversations merge.
//!
//! When a reply carries no such headers at all, as mail from some web forms
//! and old clients does, it is placed by subject: under the latest message
//! of the same subject, in the last 30 days, that shares a participant with
//! it. That link is a guess and is recorded as one, so the interface can
//! say so.

use super::index::Index;
use super::maildir::Flags;
use super::Result;
use rusqlite::{params, Connection, OptionalExtension};

const GUESS_WINDOW: i64 = 30 * 86_400;

/// The subject without its reply and forward prefixes, lower-cased, and
/// whether it had a reply prefix (in the handful of languages that mail
/// clients commonly use).
pub fn subject_key(subject: &str) -> (String, bool) {
    const REPLY: &[&str] = &["re", "aw", "sv", "vs", "antw", "odp", "ref", "rif", "res"];
    const FORWARD: &[&str] = &["fwd", "fw", "wg", "tr", "vl", "doorst", "enc", "rv", "i"];
    let mut rest = subject.trim();
    let mut is_reply = false;
    loop {
        let Some(colon) = rest.find(':').filter(|c| *c <= 8) else { break };
        // "Re[2]:" and "Re (3):" count as "Re:".
        let word: String = rest[..colon]
            .chars()
            .take_while(|c| c.is_alphabetic())
            .collect::<String>()
            .to_lowercase();
        let tail_ok = rest[word.len()..colon].chars().all(|c| c.is_ascii_digit() || " []()".contains(c));
        if !tail_ok || word.is_empty() {
            break;
        }
        if REPLY.contains(&word.as_str()) {
            is_reply = true;
        } else if !FORWARD.contains(&word.as_str()) {
            break;
        }
        rest = rest[colon + 1..].trim_start();
    }
    (rest.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase(), is_reply)
}

fn held(conn: &Connection, message_id: &str, not: i64) -> rusqlite::Result<Option<(i64, i64)>> {
    conn.prepare_cached(
        "SELECT id, thread_id FROM message
         WHERE message_id = ?1 AND id != ?2 AND thread_id IS NOT NULL ORDER BY id LIMIT 1",
    )?
    .query_row(params![message_id, not], |r| Ok((r.get(0)?, r.get(1)?)))
    .optional()
}

/// Whether two messages have someone in common: the sender of one among the
/// sender or recipients of the other.
fn share_participant(a_from: &str, a_to: &str, b_from: &str, b_to: &str) -> bool {
    let has = |list: &str, address: &str| !address.is_empty() && list.to_lowercase().contains(&address.to_lowercase());
    a_from.eq_ignore_ascii_case(b_from) && !a_from.is_empty() || has(b_to, a_from) || has(a_to, b_from)
}

/// Places one message in its conversation. Safe to call again for a message
/// already placed.
pub(super) fn link(conn: &Connection, id: i64) -> rusqlite::Result<()> {
    let row = conn
        .prepare_cached(
            "SELECT message_id, in_reply_to, refs, subject, date, from_addr, recipients FROM message WHERE id = ?1",
        )?
        .query_row([id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
            ))
        })
        .optional()?;
    let Some((message_id, in_reply_to, refs, subject, date, from, recipients)) = row else {
        return Ok(());
    };
    let (key, is_reply) = subject_key(&subject);

    // Everything this message cites, oldest first, ending with its parent.
    let mut cited: Vec<&str> = refs.split_whitespace().filter(|r| *r != message_id).collect();
    if !in_reply_to.is_empty() && in_reply_to != message_id {
        cited.retain(|r| *r != in_reply_to);
        cited.push(&in_reply_to);
    }

    // Its parent is the nearest cited message that is here.
    let mut parent: Option<i64> = None;
    let mut thread: Option<i64> = None;
    for cite in cited.iter().rev() {
        if let Some((found, found_thread)) = held(conn, cite, id)? {
            parent = Some(found);
            thread = Some(found_thread);
            break;
        }
    }
    // None of them is here, but another message citing one of them may be:
    // the two share an ancestor we do not hold.
    if thread.is_none() {
        for cite in cited.iter().rev() {
            thread = conn
                .prepare_cached(
                    "SELECT m.thread_id FROM reference_to r JOIN message m ON m.id = r.message
                     WHERE r.ref = ?1 AND m.id != ?2 AND m.thread_id IS NOT NULL LIMIT 1",
                )?
                .query_row(params![cite, id], |r| r.get(0))
                .optional()?;
            if thread.is_some() {
                break;
            }
        }
    }
    // A reply that cites nothing: place it by subject, and say it is a guess.
    let mut guessed = false;
    if thread.is_none() && cited.is_empty() && is_reply && !key.is_empty() {
        let mut candidates = conn.prepare_cached(
            "SELECT id, thread_id, from_addr, recipients FROM message
             WHERE subject_key = ?1 AND date <= ?2 AND date >= ?3 AND id != ?4 AND thread_id IS NOT NULL
             ORDER BY date DESC, id DESC LIMIT 25",
        )?;
        let found = candidates
            .query_map(params![key, date, date - GUESS_WINDOW, id], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?))
            })?
            .filter_map(|c| c.ok())
            .find(|(_, _, c_from, c_to)| share_participant(&from, &recipients, c_from, c_to));
        if let Some((found, found_thread, _, _)) = found {
            parent = Some(found);
            thread = Some(found_thread);
            guessed = true;
        }
    }
    let thread = thread.unwrap_or(id);
    conn.prepare_cached(
        "UPDATE message SET thread_id = ?1, parent_id = ?2, link_guessed = ?3, subject_key = ?4 WHERE id = ?5",
    )?
    .execute(params![thread, parent, guessed, key, id])?;

    conn.prepare_cached("DELETE FROM reference_to WHERE message = ?1")?.execute([id])?;
    for cite in &cited {
        conn.prepare_cached("INSERT INTO reference_to (ref, message) VALUES (?1, ?2)")?
            .execute(params![cite, id])?;
    }

    // Messages already here that cite this one arrived before it. Bring
    // their conversations into this one, and put direct replies under it.
    if !message_id.is_empty() {
        let citing: Vec<(i64, i64, String, Option<i64>, bool)> = conn
            .prepare_cached(
                "SELECT m.id, m.thread_id, m.in_reply_to, m.parent_id, m.link_guessed
                 FROM reference_to r JOIN message m ON m.id = r.message
                 WHERE r.ref = ?1 AND m.id != ?2 AND m.thread_id IS NOT NULL",
            )?
            .query_map(params![message_id, id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        for (child, child_thread, child_reply_to, child_parent, child_guessed) in citing {
            if child_thread != thread {
                conn.prepare_cached("UPDATE message SET thread_id = ?1 WHERE thread_id = ?2")?
                    .execute(params![thread, child_thread])?;
            }
            if child_reply_to == message_id || child_parent.is_none() || child_guessed {
                conn.prepare_cached("UPDATE message SET parent_id = ?1, link_guessed = 0 WHERE id = ?2")?
                    .execute(params![id, child])?;
            }
        }
    }
    Ok(())
}

/// Places every message, oldest first. For after a rebuild.
pub(super) fn link_all(conn: &Connection, mut progress: impl FnMut(u64, u64)) -> rusqlite::Result<()> {
    conn.execute_batch("DELETE FROM reference_to; UPDATE message SET thread_id = NULL, parent_id = NULL, link_guessed = 0")?;
    let ids: Vec<i64> = conn
        .prepare("SELECT id FROM message ORDER BY date, id")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let total = ids.len() as u64;
    conn.execute_batch("BEGIN")?;
    for (n, id) in ids.iter().enumerate() {
        link(conn, *id)?;
        if (n + 1) % 20_000 == 0 {
            conn.execute_batch("COMMIT; BEGIN")?;
            progress(n as u64 + 1, total);
        }
    }
    conn.execute_batch("COMMIT")
}

/// One message of a conversation.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadRow {
    pub id: i64,
    pub folder_id: i64,
    /// The message this one replies to, if it is in the conversation.
    pub parent: Option<i64>,
    /// The link to `parent` was inferred from the subject, not from headers.
    pub guessed: bool,
    pub from_name: String,
    pub from_addr: String,
    pub recipients: String,
    pub date: i64,
    pub flags: Flags,
    pub has_attachment: bool,
    pub subject: String,
    pub preview: String,
    pub downloaded: bool,
}

impl Index {
    /// The whole conversation a message belongs to, oldest first, across
    /// every folder. A message that is in two folders (a copy the user kept)
    /// appears once.
    pub fn thread_of(&self, message: i64) -> Result<Vec<ThreadRow>> {
        let rows: Vec<(ThreadRow, String)> = self.read(|conn| {
            conn.prepare_cached(
                "SELECT m.id, m.folder_id, m.parent_id, m.link_guessed, m.from_name, m.from_addr,
                        m.recipients, m.date, m.flags, m.has_attachment, m.subject, m.preview,
                        m.base != '', m.message_id
                 FROM message m
                 WHERE m.id = ?1 OR m.thread_id = (SELECT thread_id FROM message WHERE id = ?1)
                 ORDER BY m.date, m.id",
            )?
            .query_map([message], |r| {
                Ok((
                    ThreadRow {
                        id: r.get(0)?,
                        folder_id: r.get(1)?,
                        parent: r.get(2)?,
                        guessed: r.get(3)?,
                        from_name: r.get(4)?,
                        from_addr: r.get(5)?,
                        recipients: r.get(6)?,
                        date: r.get(7)?,
                        flags: Flags(r.get(8)?),
                        has_attachment: r.get(9)?,
                        subject: r.get(10)?,
                        preview: r.get(11)?,
                        downloaded: r.get(12)?,
                    },
                    r.get::<_, String>(13)?,
                ))
            })?
            .collect()
        })?;
        // Copies of one message: keep the one asked for, else the first, and
        // point anything that replied to a dropped copy at the kept one.
        let mut kept: Vec<ThreadRow> = Vec::new();
        let mut seen: std::collections::HashMap<String, i64> = Default::default();
        let mut replaced: std::collections::HashMap<i64, i64> = Default::default();
        let asked_id = rows.iter().find(|(r, _)| r.id == message).map(|(_, m)| m.clone()).unwrap_or_default();
        for (row, message_id) in rows {
            if message_id.is_empty() {
                kept.push(row);
                continue;
            }
            if message_id == asked_id && row.id != message {
                replaced.insert(row.id, message);
                continue;
            }
            match seen.get(&message_id) {
                Some(first) => {
                    replaced.insert(row.id, *first);
                }
                None => {
                    seen.insert(message_id, row.id);
                    kept.push(row);
                }
            }
        }
        let present: std::collections::HashSet<i64> = kept.iter().map(|r| r.id).collect();
        for row in &mut kept {
            if let Some(parent) = row.parent {
                let parent = replaced.get(&parent).copied().unwrap_or(parent);
                row.parent = (present.contains(&parent) && parent != row.id).then_some(parent);
            }
        }
        Ok(kept)
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::TempDir;
    use super::super::Store;
    use super::*;

    struct Mail<'a> {
        id: &'a str,
        subject: &'a str,
        from: &'a str,
        to: &'a str,
        day: u32,
        reply_to: &'a str,
        refs: &'a str,
    }

    fn raw(m: &Mail) -> Vec<u8> {
        let mut text = format!(
            "From: {}\r\nTo: {}\r\nSubject: {}\r\nDate: {} Nov 2026 09:00:00 +0000\r\n",
            m.from, m.to, m.subject, m.day
        );
        if !m.id.is_empty() {
            text.push_str(&format!("Message-ID: <{}>\r\n", m.id));
        }
        if !m.reply_to.is_empty() {
            text.push_str(&format!("In-Reply-To: <{}>\r\n", m.reply_to));
        }
        if !m.refs.is_empty() {
            let refs: Vec<String> = m.refs.split(' ').map(|r| format!("<{r}>")).collect();
            text.push_str(&format!("References: {}\r\n", refs.join(" ")));
        }
        text.push_str("\r\nbody\r\n");
        text.into_bytes()
    }

    const A: &str = "alice@example.com";
    const B: &str = "bob@example.com";

    fn mail<'a>(id: &'a str, subject: &'a str, day: u32, reply_to: &'a str, refs: &'a str) -> Mail<'a> {
        Mail { id, subject, from: A, to: B, day, reply_to, refs }
    }

    fn store() -> (TempDir, Store, i64, i64) {
        let tmp = TempDir::new();
        let store = Store::open(tmp.path()).unwrap();
        let inbox = store.create_folder(&["Inbox"]).unwrap();
        let sent = store.create_folder(&["Sent"]).unwrap();
        (tmp, store, inbox, sent)
    }

    /// The conversation of `id` as (subject, parent's subject, guessed).
    fn shape(store: &Store, id: i64) -> Vec<(String, Option<String>, bool)> {
        let rows = store.index().thread_of(id).unwrap();
        rows.iter()
            .map(|r| {
                let parent = r.parent.map(|p| rows.iter().find(|x| x.id == p).unwrap().subject.clone());
                (r.subject.clone(), parent, r.guessed)
            })
            .collect()
    }

    fn s(text: &str) -> Option<String> {
        Some(text.to_string())
    }

    #[test]
    fn subjects_lose_their_prefixes() {
        assert_eq!(subject_key("Lunch on Friday?"), ("lunch on friday?".into(), false));
        assert_eq!(subject_key("Re: Lunch on Friday?"), ("lunch on friday?".into(), true));
        assert_eq!(subject_key("RE: Fwd: re[2]:  Lunch   on Friday?"), ("lunch on friday?".into(), true));
        assert_eq!(subject_key("AW: WG: Mittagessen"), ("mittagessen".into(), true));
        assert_eq!(subject_key("Fwd: Lunch"), ("lunch".into(), false), "a forward is not a reply");
        assert_eq!(subject_key("Reminder: lunch"), ("reminder: lunch".into(), false));
        assert_eq!(subject_key("Re: "), ("".into(), true));
        assert_eq!(subject_key(""), ("".into(), false));
    }

    #[test]
    fn replies_are_linked_whatever_order_they_arrive_in() {
        let chain = [
            mail("a", "one", 1, "", ""),
            mail("b", "two", 2, "a", "a"),
            mail("c", "three", 3, "b", "a b"),
            mail("d", "four, other branch", 4, "a", "a"),
        ];
        let expected = vec![
            ("one".to_string(), None, false),
            ("two".to_string(), s("one"), false),
            ("three".to_string(), s("two"), false),
            ("four, other branch".to_string(), s("one"), false),
        ];
        for order in [[0, 1, 2, 3], [3, 2, 1, 0], [2, 0, 3, 1], [1, 3, 0, 2]] {
            let (_tmp, store, inbox, sent) = store();
            let mut last = 0;
            for i in order {
                // The user's own reply lives in another folder.
                let folder = if i == 1 { sent } else { inbox };
                last = store.add_message(folder, &raw(&chain[i]), Flags::SEEN, None).unwrap();
            }
            assert_eq!(shape(&store, last), expected, "arrival order {order:?}");
        }
    }

    #[test]
    fn a_missing_message_does_not_break_the_conversation() {
        let (_tmp, store, inbox, _) = store();
        // "b" never arrives. "c" hangs from the nearest ancestor that did.
        store.add_message(inbox, &raw(&mail("a", "one", 1, "", "")), Flags::SEEN, None).unwrap();
        let c = store.add_message(inbox, &raw(&mail("c", "three", 3, "b", "a b")), Flags::SEEN, None).unwrap();
        assert_eq!(shape(&store, c), [("one".to_string(), None, false), ("three".to_string(), s("one"), false)]);

        // Two replies to a root that never arrives are still one conversation.
        let x = store.add_message(inbox, &raw(&mail("x", "reply one", 5, "lost", "lost")), Flags::SEEN, None).unwrap();
        store.add_message(inbox, &raw(&mail("y", "reply two", 6, "lost", "lost")), Flags::SEEN, None).unwrap();
        assert_eq!(shape(&store, x), [("reply one".to_string(), None, false), ("reply two".to_string(), None, false)]);
        // And if it turns up later, they move under it.
        store.add_message(inbox, &raw(&mail("lost", "found", 4, "", "")), Flags::SEEN, None).unwrap();
        assert_eq!(
            shape(&store, x),
            [("found".to_string(), None, false), ("reply one".to_string(), s("found"), false), ("reply two".to_string(), s("found"), false)]
        );
    }

    #[test]
    fn a_reply_without_headers_is_placed_by_subject_and_marked_as_a_guess() {
        let (_tmp, store, inbox, _) = store();
        store.add_message(inbox, &raw(&mail("a", "Lunch on Friday?", 1, "", "")), Flags::SEEN, None).unwrap();
        let guess = Mail { id: "g", subject: "RE: Lunch on Friday?", from: B, to: A, day: 2, reply_to: "", refs: "" };
        let g = store.add_message(inbox, &raw(&guess), Flags::SEEN, None).unwrap();
        assert_eq!(
            shape(&store, g),
            [("Lunch on Friday?".to_string(), None, false), ("RE: Lunch on Friday?".to_string(), s("Lunch on Friday?"), true)]
        );

        // Not guessed: a new message that merely has the same subject; a
        // reply from strangers; a reply long after.
        let cases = [
            Mail { id: "n", subject: "Lunch on Friday?", from: B, to: A, day: 3, reply_to: "", refs: "" },
            Mail { id: "s", subject: "Re: Lunch on Friday?", from: "x@elsewhere.example", to: "y@elsewhere.example", day: 3, reply_to: "", refs: "" },
        ];
        for case in &cases {
            let id = store.add_message(inbox, &raw(case), Flags::SEEN, None).unwrap();
            assert_eq!(shape(&store, id).len(), 1, "{} should stand alone", case.id);
        }
        let late = b"From: bob@example.com\r\nTo: alice@example.com\r\nSubject: Re: Lunch on Friday?\r\nDate: 20 Jan 2027 09:00:00 +0000\r\nMessage-ID: <late>\r\n\r\nb\r\n";
        let id = store.add_message(inbox, late, Flags::SEEN, None).unwrap();
        assert_eq!(shape(&store, id).len(), 1, "two months later is a different conversation");

        // A real header beats the guess when the cited message arrives.
        let real = Mail { id: "p", subject: "something else entirely", from: A, to: B, day: 1, reply_to: "", refs: "" };
        let cites = Mail { id: "q", subject: "Re: Lunch on Friday?", from: B, to: A, day: 2, reply_to: "p", refs: "p" };
        let q = store.add_message(inbox, &raw(&cites), Flags::SEEN, None).unwrap();
        assert_eq!(shape(&store, q).len(), 1, "it cites a message, so nothing is guessed from its subject");
        store.add_message(inbox, &raw(&real), Flags::SEEN, None).unwrap();
        assert_eq!(shape(&store, q)[1], ("Re: Lunch on Friday?".to_string(), s("something else entirely"), false));
    }

    #[test]
    fn a_copy_in_another_folder_appears_once_and_deleting_leaves_no_trace() {
        let (_tmp, store, inbox, sent) = store();
        let a = store.add_message(inbox, &raw(&mail("a", "one", 1, "", "")), Flags::SEEN, None).unwrap();
        let copy = store.add_message(sent, &raw(&mail("a", "one", 1, "", "")), Flags::SEEN, None).unwrap();
        let b = store.add_message(inbox, &raw(&mail("b", "two", 2, "a", "a")), Flags::SEEN, None).unwrap();
        assert_eq!(shape(&store, b), [("one".to_string(), None, false), ("two".to_string(), s("one"), false)]);
        // Asked from the copy, the copy is the one shown.
        assert_eq!(store.index().thread_of(copy).unwrap()[0].id, copy);
        assert_eq!(store.index().thread_of(a).unwrap()[0].id, a);

        store.remove_messages(inbox, &[b]).unwrap();
        let left: i64 = store.index().read(|c| c.query_row("SELECT count(*) FROM reference_to", [], |r| r.get(0))).unwrap();
        assert_eq!(left, 0, "what a deleted message cited is forgotten with it");
        // Deleting a parent leaves its replies as the top of what remains.
        let c = store.add_message(inbox, &raw(&mail("c", "three", 3, "a", "a")), Flags::SEEN, None).unwrap();
        store.remove_messages(inbox, &[a]).unwrap();
        store.remove_messages(sent, &[copy]).unwrap();
        assert_eq!(shape(&store, c), [("three".to_string(), None, false)]);
    }

    #[test]
    fn a_rebuild_finds_the_same_conversations() {
        let (_tmp, store, inbox, sent) = store();
        let mut ids = Vec::new();
        for (i, m) in [
            mail("c", "three", 3, "b", "a b"),
            mail("a", "one", 1, "", ""),
            mail("z", "alone", 2, "", ""),
            mail("b", "two", 2, "a", "a"),
        ]
        .iter()
        .enumerate()
        {
            ids.push(store.add_message(if i == 3 { sent } else { inbox }, &raw(m), Flags::SEEN, None).unwrap());
        }
        let before = shape(&store, ids[0]);
        assert_eq!(before.len(), 3);
        store.rebuild(false, |_| {}).unwrap();
        let folder = store.index().folders().unwrap().into_iter().find(|f| f.path == "Inbox").unwrap();
        let three = store.index().page(folder.id, 0, 10).unwrap().into_iter().find(|r| r.subject == "three").unwrap();
        assert_eq!(shape(&store, three.id), before);
        let alone = store.index().page(folder.id, 0, 10).unwrap().into_iter().find(|r| r.subject == "alone").unwrap();
        assert_eq!(shape(&store, alone.id).len(), 1);
    }
}
