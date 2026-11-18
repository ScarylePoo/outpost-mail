//! The CalDAV and CardDAV client against a real server (Radicale).

#[path = "common/dav.rs"]
mod dav;

use dav::{DavServer, DAV_PASSWORD, DAV_USER};
use outpost_core::dav::client::{Dav, DavError, Kind};
use outpost_core::dav::http::Url;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
}

fn event(uid: &str, summary: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Outpost//Test//EN\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nDTSTAMP:20261118T100000Z\r\nDTSTART:20261120T090000Z\r\nDTEND:20261120T100000Z\r\nSUMMARY:{summary}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

const CALENDAR: Kind = Kind::Calendar { events: true, tasks: false };

#[test]
fn collections_are_found_and_items_saved_fetched_changed_and_deleted() {
    let Some(server) = DavServer::start() else { return };
    runtime().block_on(async {
        let base = Url::parse(&server.address()).unwrap();
        let dav = Dav::new(base.clone(), DAV_USER, DAV_PASSWORD);

        // A new account has nothing; a wrong password is said to be one.
        assert_eq!(dav.discover().await.unwrap(), []);
        let wrong = Dav::new(base.clone(), DAV_USER, "not it");
        assert_eq!(wrong.discover().await, Err(DavError::SignIn));

        // Collections of each kind are created and then found, from the
        // bare server address.
        dav.create_collection("/ann/work/", "Work & travel", CALENDAR).await.unwrap();
        dav.create_collection("/ann/todo/", "To do", Kind::Calendar { events: false, tasks: true }).await.unwrap();
        dav.create_collection("/ann/people/", "People", Kind::AddressBook).await.unwrap();
        let found = dav.discover().await.unwrap();
        let seen: Vec<(&str, &str, Kind)> = found.iter().map(|c| (c.name.as_str(), c.href.as_str(), c.kind)).collect();
        assert_eq!(
            seen,
            [
                ("People", "/ann/people/", Kind::AddressBook),
                ("To do", "/ann/todo/", Kind::Calendar { events: false, tasks: true }),
                ("Work & travel", "/ann/work/", CALENDAR),
            ]
        );
        let tag_before = dav.collection("/ann/work/").await.unwrap().unwrap().ctag;
        assert!(tag_before.is_some(), "this server offers change tags");

        // Save, list, fetch.
        dav.put("/ann/work/one.ics", CALENDAR, &event("one", "Planning"), None).await.unwrap();
        dav.put("/ann/work/two.ics", CALENDAR, &event("two", "Review"), None).await.unwrap();
        let listed = dav.list("/ann/work/").await.unwrap();
        let mut hrefs: Vec<String> = listed.iter().map(|i| i.href.clone()).collect();
        hrefs.sort();
        assert_eq!(hrefs, ["/ann/work/one.ics", "/ann/work/two.ics"]);
        assert_ne!(dav.collection("/ann/work/").await.unwrap().unwrap().ctag, tag_before, "the tag moved");
        let fetched = dav.fetch("/ann/work/", CALENDAR, &hrefs).await.unwrap();
        assert_eq!(fetched.len(), 2);
        let one = fetched.iter().find(|i| i.href.ends_with("one.ics")).unwrap();
        assert!(one.data.contains("SUMMARY:Planning"), "{}", one.data);
        assert_eq!(Some(&one.etag), listed.iter().find(|i| i.href == one.href).map(|i| &i.etag));

        // Changing an item is done against the version last read.
        dav.put("/ann/work/one.ics", CALENDAR, &event("one", "Planning, moved"), Some(&one.etag)).await.unwrap();
        // The version we held is now stale: neither saving nor deleting
        // with it goes through, so someone else's change is never lost.
        assert_eq!(
            dav.put("/ann/work/one.ics", CALENDAR, &event("one", "Late edit"), Some(&one.etag)).await,
            Err(DavError::Changed)
        );
        assert_eq!(dav.delete("/ann/work/one.ics", Some(&one.etag)).await, Err(DavError::Changed));
        let now = dav.fetch("/ann/work/", CALENDAR, &["/ann/work/one.ics".to_string()]).await.unwrap();
        assert!(now[0].data.contains("Planning, moved"));

        // Delete with the right version; deleting what is gone is fine.
        dav.delete("/ann/work/one.ics", Some(&now[0].etag)).await.unwrap();
        dav.delete("/ann/work/one.ics", None).await.unwrap();
        assert_eq!(dav.list("/ann/work/").await.unwrap().len(), 1);

        // A contact, in the address book.
        let card = "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:c1\r\nFN:Dana Dubois\r\nN:Dubois;Dana;;;\r\nEMAIL:dana@example.com\r\nEND:VCARD\r\n";
        dav.put("/ann/people/c1.vcf", Kind::AddressBook, card, None).await.unwrap();
        let cards = dav.fetch("/ann/people/", Kind::AddressBook, &["/ann/people/c1.vcf".to_string()]).await.unwrap();
        assert!(cards[0].data.contains("FN:Dana Dubois"), "{}", cards[0].data);

        // Nothing there.
        assert_eq!(dav.list("/ann/nowhere/").await, Err(DavError::NotFound));
    });
}
