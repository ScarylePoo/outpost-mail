//! Calendar and contacts sync against a real server (Radicale), with a
//! second client changing things behind Outpost's back.

#[path = "common/dav.rs"]
mod dav;

use dav::{DavServer, DAV_PASSWORD, DAV_USER};
use outpost_core::dav::client::{Dav, DavError, Kind};
use outpost_core::dav::http::Url;
use outpost_core::dav::item::{edit_event, new_event, new_task, write_card, ContactEdit, EventEdit, TaskEdit};
use outpost_core::dav::store::DavStore;
use outpost_core::dav::sync::{sync, SyncError};

const CAL: Kind = Kind::Calendar { events: true, tasks: true };
const DAY: i64 = 86_400;
const NOV_20: i64 = 20_777 * DAY;

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("outpost-davsync-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn event(uid: &str, summary: &str, hour: i64) -> String {
    new_event(uid, &edit(summary, hour), 0)
}

fn edit(summary: &str, hour: i64) -> EventEdit {
    EventEdit { summary: summary.into(), start: NOV_20 + hour * 3600, end: NOV_20 + (hour + 1) * 3600, ..EventEdit::default() }
}

fn titles(store: &DavStore) -> Vec<String> {
    store.occurrences(NOV_20, NOV_20 + DAY).unwrap().into_iter().map(|o| o.summary).collect()
}

#[test]
fn an_account_is_mirrored_and_changes_go_both_ways() {
    let Some(server) = DavServer::start() else { return };
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let dav = Dav::new(Url::parse(&server.address()).unwrap(), DAV_USER, DAV_PASSWORD);
        // What is already on the server, put there by "another program".
        dav.create_collection("/ann/work/", "Work", CAL).await.unwrap();
        dav.create_collection("/ann/people/", "People", Kind::AddressBook).await.unwrap();
        dav.put("/ann/work/a.ics", CAL, &event("a", "Planning", 9), None).await.unwrap();
        dav.put("/ann/work/b.ics", CAL, &event("b", "Review", 11), None).await.unwrap();
        let card = write_card(None, "c1", &ContactEdit { name: "Dana Dubois".into(), emails: vec!["dana@example.com".into()], ..ContactEdit::default() }).unwrap();
        dav.put("/ann/people/c1.vcf", Kind::AddressBook, &card, None).await.unwrap();

        // First sync: everything arrives.
        let store = DavStore::open(temp("mirror"), "UTC").unwrap();
        let first = sync(&dav, &store).await.unwrap();
        assert_eq!((first.fetched, first.changed), (3, true));
        assert_eq!(titles(&store), ["Planning", "Review"]);
        assert_eq!(store.contacts("", 10).unwrap()[0].name, "Dana Dubois");
        let work = store.collections().unwrap().into_iter().find(|c| c.name == "Work").unwrap();

        // Nothing changed: the next pass fetches nothing.
        let quiet = sync(&dav, &store).await.unwrap();
        assert_eq!((quiet.fetched, quiet.removed, quiet.sent, quiet.changed), (0, 0, 0, false));

        // The other program changes one, deletes one, adds one.
        let listed = dav.list("/ann/work/").await.unwrap();
        let etag = |name: &str| listed.iter().find(|i| i.href.ends_with(name)).unwrap().etag.clone();
        dav.put("/ann/work/a.ics", CAL, &event("a", "Planning moved", 10), Some(&etag("a.ics"))).await.unwrap();
        dav.delete("/ann/work/b.ics", Some(&etag("b.ics"))).await.unwrap();
        dav.put("/ann/work/c.ics", CAL, &event("c", "Lunch", 12), None).await.unwrap();
        let pass = sync(&dav, &store).await.unwrap();
        assert_eq!((pass.fetched, pass.removed), (2, 1));
        assert_eq!(titles(&store), ["Planning moved", "Lunch"]);

        // The user makes an event, a task and a contact here. They show at
        // once, and after a sync they are on the server.
        let made = store.save_local(work.id, None, &event("mine", "Made in Outpost, with a comma", 15)).unwrap();
        store.save_local(work.id, None, &new_task("t1", &TaskEdit { summary: "Buy milk".into(), ..TaskEdit::default() }, 0)).unwrap();
        let people = store.collections().unwrap().into_iter().find(|c| c.name == "People").unwrap();
        let new_card = write_card(None, "c2", &ContactEdit { name: "Sam Okafor".into(), emails: vec!["sam@example.com".into()], ..ContactEdit::default() }).unwrap();
        store.save_local(people.id, None, &new_card).unwrap();
        assert_eq!(titles(&store).len(), 3);
        let pass = sync(&dav, &store).await.unwrap();
        assert_eq!((pass.sent, pass.notices.len()), (3, 0), "{:?}", pass.notices);
        assert_eq!(store.pending().unwrap(), []);
        assert_eq!(dav.list("/ann/work/").await.unwrap().len(), 4);
        assert_eq!(dav.list("/ann/people/").await.unwrap().len(), 2);
        // What the server holds is what was written, comma and all.
        let hrefs: Vec<String> = dav.list("/ann/work/").await.unwrap().into_iter().map(|i| i.href).collect();
        let on_server = dav.fetch("/ann/work/", CAL, &hrefs).await.unwrap();
        assert!(on_server.iter().any(|i| i.data.contains("SUMMARY:Made in Outpost\\, with a comma")), "{on_server:?}");
        // A sync straight after settles (the server may have rewritten the
        // files it was given) and then goes quiet, with nothing doubled.
        sync(&dav, &store).await.unwrap();
        assert!(!sync(&dav, &store).await.unwrap().changed);
        assert_eq!(titles(&store), ["Planning moved", "Lunch", "Made in Outpost, with a comma"]);
        assert_eq!(store.tasks().unwrap().len(), 1);

        // The user changes that event and deletes "Lunch".
        let text = store.data(made).unwrap();
        store.save_local(work.id, Some(made), &edit_event(&text, &edit("Renamed in Outpost", 16), 0).unwrap()).unwrap();
        let lunch = store.occurrences(NOV_20, NOV_20 + DAY).unwrap().into_iter().find(|o| o.summary == "Lunch").unwrap().id;
        store.delete_local(lunch).unwrap();
        let pass = sync(&dav, &store).await.unwrap();
        assert_eq!((pass.sent, pass.notices.len()), (2, 0), "{:?}", pass.notices);
        let hrefs: Vec<String> = dav.list("/ann/work/").await.unwrap().into_iter().map(|i| i.href).collect();
        let on_server = dav.fetch("/ann/work/", CAL, &hrefs).await.unwrap();
        assert_eq!(on_server.len(), 3);
        assert!(on_server.iter().any(|i| i.data.contains("SUMMARY:Renamed in Outpost")));
        assert!(!on_server.iter().any(|i| i.data.contains("Lunch")));
        sync(&dav, &store).await.unwrap();
        assert_eq!(titles(&store), ["Planning moved", "Renamed in Outpost"]);
    });
}

#[test]
fn a_change_made_in_two_places_is_never_silently_lost() {
    let Some(server) = DavServer::start() else { return };
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let dav = Dav::new(Url::parse(&server.address()).unwrap(), DAV_USER, DAV_PASSWORD);
        dav.create_collection("/ann/work/", "Work", CAL).await.unwrap();
        dav.put("/ann/work/a.ics", CAL, &event("a", "Planning", 9), None).await.unwrap();
        dav.put("/ann/work/b.ics", CAL, &event("b", "Review", 11), None).await.unwrap();
        let store = DavStore::open(temp("conflict"), "UTC").unwrap();
        sync(&dav, &store).await.unwrap();
        let work = store.collections().unwrap()[0].id;
        let find = |title: &str| store.occurrences(NOV_20, NOV_20 + DAY).unwrap().into_iter().find(|o| o.summary == title).unwrap().id;
        let (planning, review) = (find("Planning"), find("Review"));

        // Both sides change "Planning"; the user deletes "Review" while the
        // other program changes it.
        let listed = dav.list("/ann/work/").await.unwrap();
        let etag = |name: &str| listed.iter().find(|i| i.href.ends_with(name)).unwrap().etag.clone();
        dav.put("/ann/work/a.ics", CAL, &event("a", "Planning (their edit)", 9), Some(&etag("a.ics"))).await.unwrap();
        dav.put("/ann/work/b.ics", CAL, &event("b", "Review (their edit)", 11), Some(&etag("b.ics"))).await.unwrap();
        let text = store.data(planning).unwrap();
        store.save_local(work, Some(planning), &edit_event(&text, &edit("Planning (my edit)", 9), 0).unwrap()).unwrap();
        store.delete_local(review).unwrap();

        let pass = sync(&dav, &store).await.unwrap();
        // Their versions are kept, here and there, and the user is told
        // about each.
        assert_eq!(pass.notices.len(), 2, "{:?}", pass.notices);
        assert!(pass.notices[0].contains("Planning (my edit)") && pass.notices[0].contains("not saved"), "{:?}", pass.notices);
        assert!(pass.notices[1].contains("Review") && pass.notices[1].contains("not deleted"), "{:?}", pass.notices);
        assert_eq!(titles(&store), ["Planning (their edit)", "Review (their edit)"]);
        assert_eq!(store.pending().unwrap(), []);
        let hrefs: Vec<String> = dav.list("/ann/work/").await.unwrap().into_iter().map(|i| i.href).collect();
        let on_server = dav.fetch("/ann/work/", CAL, &hrefs).await.unwrap();
        assert_eq!(on_server.len(), 2);
        assert!(on_server.iter().all(|i| i.data.contains("their edit")));

        // An unreachable server loses nothing: the change waits.
        let text = store.data(planning).unwrap();
        store.save_local(work, Some(planning), &edit_event(&text, &edit("Edited while offline", 9), 0).unwrap()).unwrap();
        let nowhere = Dav::new(Url::parse("http://127.0.0.1:1/").unwrap(), DAV_USER, DAV_PASSWORD);
        assert!(matches!(sync(&nowhere, &store).await, Err(SyncError::Dav(DavError::Http(_)))));
        let wrong = Dav::new(Url::parse(&server.address()).unwrap(), DAV_USER, "not it");
        assert!(matches!(sync(&wrong, &store).await, Err(SyncError::Dav(DavError::SignIn))));
        assert_eq!(store.pending().unwrap().len(), 1);
        assert_eq!(titles(&store)[0], "Edited while offline");
        let pass = sync(&dav, &store).await.unwrap();
        assert_eq!((pass.sent, pass.notices.len()), (1, 0));

        // A collection switched off is neither read nor shown; switched
        // back on, it is read again.
        store.choose(work, false, None).unwrap();
        dav.put("/ann/work/z.ics", CAL, &event("z", "Added while off", 13), None).await.unwrap();
        sync(&dav, &store).await.unwrap();
        assert_eq!(titles(&store), Vec::<String>::new());
        store.choose(work, true, None).unwrap();
        sync(&dav, &store).await.unwrap();
        assert!(titles(&store).contains(&"Added while off".to_string()));

        // A collection deleted on the server goes from here too.
        runtime_delete(&dav, "/ann/work/").await;
        sync(&dav, &store).await.unwrap();
        assert_eq!(store.collections().unwrap(), []);
    });
}

async fn runtime_delete(dav: &Dav, href: &str) {
    dav.delete(href, None).await.unwrap();
}
