//! What a CalDAV or CardDAV server is asked.
//!
//! Finding a user's calendars and address books (RFC 4791, 6352, 6764),
//! listing what is in one, fetching, saving and deleting items. Everything
//! here is one request or a short fixed sequence; deciding what to sync is
//! `sync.rs`'s job.
//!
//! The user's name and password are only ever sent to the server the user
//! named: a redirect or a link to another server is refused, not followed.

use super::http::{self, HttpError, Request, Response, Url};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DavError {
    Http(HttpError),
    /// The name or password was refused.
    SignIn,
    /// The address is not there (any more).
    NotFound,
    /// The item changed on the server since it was last read here.
    Changed,
    /// The server refused, with this status and what it was doing.
    Refused { status: u16, doing: String },
    /// The server pointed to a different server.
    ElsewhereRedirect(String),
    /// The answer could not be understood.
    Unreadable(String),
}

impl std::fmt::Display for DavError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DavError::Http(e) => e.fmt(f),
            DavError::SignIn => write!(f, "the server refused the name or password"),
            DavError::NotFound => write!(f, "the server has nothing at that address"),
            DavError::Changed => write!(f, "it was changed on the server in the meantime"),
            DavError::Refused { status, doing } => write!(f, "the server refused ({status}) while {doing}"),
            DavError::ElsewhereRedirect(to) => {
                write!(f, "the server points to a different server ({to}); if that is right, enter that address instead")
            }
            DavError::Unreadable(e) => write!(f, "the server's answer could not be read: {e}"),
        }
    }
}

impl From<HttpError> for DavError {
    fn from(e: HttpError) -> Self {
        DavError::Http(e)
    }
}

pub type Result<T> = std::result::Result<T, DavError>;

/// What a collection holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A calendar. It may hold events, tasks, or both.
    Calendar { events: bool, tasks: bool },
    AddressBook,
}

/// A calendar, task list or address book on the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collection {
    /// Its path on the server, as the server writes it.
    pub href: String,
    pub name: String,
    pub kind: Kind,
    /// The colour the server holds for it (`#RRGGBB`), if any.
    pub color: Option<String>,
    /// Changes whenever anything in the collection changes, if the server
    /// offers it.
    pub ctag: Option<String>,
}

/// An item's address and version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemRef {
    pub href: String,
    pub etag: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub href: String,
    pub etag: String,
    /// The iCalendar or vCard text.
    pub data: String,
}

/// A connection to one DAV account. Cheap to make; holds no socket.
#[derive(Clone)]
pub struct Dav {
    base: Url,
    user: String,
    password: String,
}

const PROPFIND_HEAD: &str = r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:card="urn:ietf:params:xml:ns:carddav" xmlns:cs="http://calendarserver.org/ns/" xmlns:a="http://apple.com/ns/ical/"><d:prop>"#;
const PROPFIND_TAIL: &str = "</d:prop></d:propfind>";

impl Dav {
    pub fn new(base: Url, user: &str, password: &str) -> Dav {
        Dav { base, user: user.to_string(), password: password.to_string() }
    }

    pub fn base(&self) -> &Url {
        &self.base
    }

    /// One request to a path on this account's server, following redirects
    /// that stay on it.
    async fn request(&self, method: &str, href: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<(Url, Response)> {
        let mut url = self.at(href)?;
        for _ in 0..5 {
            let response = http::send(Request {
                method,
                url: &url,
                sign_in: Some((&self.user, &self.password)),
                headers,
                body,
            })
            .await?;
            if matches!(response.status, 301 | 302 | 307 | 308) {
                let Some(to) = response.header("location").and_then(|l| url.join(l)) else {
                    return Err(DavError::Unreadable("a redirect to nowhere".into()));
                };
                if !self.base.same_server(&to) {
                    return Err(DavError::ElsewhereRedirect(to.to_string()));
                }
                url = to;
                continue;
            }
            return Ok((url, response));
        }
        Err(DavError::Unreadable("too many redirects".into()))
    }

    /// The full address of a path the server gave, which must be on this
    /// account's server.
    fn at(&self, href: &str) -> Result<Url> {
        let url = self.base.join(href).ok_or_else(|| DavError::Unreadable(format!("a bad address: {href}")))?;
        if !self.base.same_server(&url) {
            return Err(DavError::ElsewhereRedirect(url.to_string()));
        }
        Ok(url)
    }

    async fn propfind(&self, href: &str, depth: &str, props: &str, doing: &str) -> Result<(Url, Vec<Entry>)> {
        let body = format!("{PROPFIND_HEAD}{props}{PROPFIND_TAIL}");
        let headers = [("Depth", depth), ("Content-Type", "application/xml; charset=utf-8")];
        let (url, response) = self.request("PROPFIND", href, &headers, body.as_bytes()).await?;
        check(&response, doing)?;
        Ok((url, entries(&response.text())?))
    }

    /// Finds the calendars, task lists and address books of this account.
    ///
    /// Starts at the address the user gave. If that says who the user is,
    /// their home collections are asked for and listed; otherwise the
    /// address itself is treated as the place the collections are in. An
    /// address with no path also tries the standard `/.well-known/` ones.
    pub async fn discover(&self) -> Result<Vec<Collection>> {
        let mut starts = vec![self.base.path.clone()];
        if self.base.path == "/" {
            starts.push("/.well-known/caldav".into());
            starts.push("/.well-known/carddav".into());
        }
        let mut homes: Vec<String> = Vec::new();
        let mut first_error = None;
        let mut reached = false;
        for start in &starts {
            let found = self
                .propfind(start, "0", "<d:current-user-principal/><d:resourcetype/>", "looking for your account")
                .await;
            let (url, found) = match found {
                Ok(found) => found,
                // A wrong password is the answer, wherever it was given.
                Err(DavError::SignIn) => return Err(DavError::SignIn),
                Err(e) => {
                    first_error.get_or_insert(e);
                    continue;
                }
            };
            reached = true;
            let principal = found.iter().find_map(|e| e.props.child("current-user-principal").and_then(|p| p.child_text("href")));
            let Some(principal) = principal else {
                // No principal: perhaps this address is itself the home.
                homes.push(url.path.clone());
                continue;
            };
            let (_, about) = self
                .propfind(&principal, "0", "<c:calendar-home-set/><card:addressbook-home-set/>", "asking where your calendars are")
                .await?;
            for entry in &about {
                for set in ["calendar-home-set", "addressbook-home-set"] {
                    if let Some(home) = entry.props.child(set) {
                        homes.extend(home.children.iter().filter(|c| c.name == "href").map(|c| c.text.trim().to_string()));
                    }
                }
            }
        }
        if !reached {
            return Err(first_error.unwrap_or(DavError::NotFound));
        }
        homes.sort();
        homes.dedup();

        let mut collections: Vec<Collection> = Vec::new();
        for home in &homes {
            let listed = self
                .propfind(
                    home,
                    "1",
                    "<d:resourcetype/><d:displayname/><a:calendar-color/><c:supported-calendar-component-set/><cs:getctag/>",
                    "listing your calendars and address books",
                )
                .await;
            let (_, listed) = match listed {
                Ok(listed) => listed,
                Err(DavError::NotFound) => continue,
                Err(e) => return Err(e),
            };
            for entry in listed {
                let Some(collection) = collection_of(&entry) else { continue };
                if !collections.iter().any(|c| same_path(&c.href, &collection.href)) {
                    collections.push(collection);
                }
            }
        }
        collections.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.href.cmp(&b.href)));
        Ok(collections)
    }

    /// One collection as it is now: for its change tag.
    pub async fn collection(&self, href: &str) -> Result<Option<Collection>> {
        let (_, found) = self
            .propfind(
                href,
                "0",
                "<d:resourcetype/><d:displayname/><a:calendar-color/><c:supported-calendar-component-set/><cs:getctag/>",
                "checking a collection",
            )
            .await?;
        Ok(found.iter().find_map(collection_of))
    }

    /// The address and version of everything in a collection.
    pub async fn list(&self, collection: &str) -> Result<Vec<ItemRef>> {
        let (_, found) = self.propfind(collection, "1", "<d:getetag/><d:resourcetype/>", "listing a collection").await?;
        Ok(found
            .into_iter()
            .filter(|e| !same_path(&e.href, collection))
            .filter(|e| e.props.child("resourcetype").is_none_or(|t| t.child("collection").is_none()))
            .filter_map(|e| {
                let etag = e.props.child_text("getetag")?;
                Some(ItemRef { href: e.href, etag })
            })
            .collect())
    }

    /// Fetches items of a collection by address, in one request.
    pub async fn fetch(&self, collection: &str, kind: Kind, hrefs: &[String]) -> Result<Vec<Item>> {
        if hrefs.is_empty() {
            return Ok(Vec::new());
        }
        let (report, ns, data) = match kind {
            Kind::Calendar { .. } => ("calendar-multiget", "urn:ietf:params:xml:ns:caldav", "calendar-data"),
            Kind::AddressBook => ("addressbook-multiget", "urn:ietf:params:xml:ns:carddav", "address-data"),
        };
        let mut body = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><x:{report} xmlns:d="DAV:" xmlns:x="{ns}"><d:prop><d:getetag/><x:{data}/></d:prop>"#
        );
        for href in hrefs {
            body.push_str(&format!("<d:href>{}</d:href>", escape(href)));
        }
        body.push_str(&format!("</x:{report}>"));
        let headers = [("Depth", "1"), ("Content-Type", "application/xml; charset=utf-8")];
        let (_, response) = self.request("REPORT", collection, &headers, body.as_bytes()).await?;
        check(&response, "fetching items")?;
        Ok(entries(&response.text())?
            .into_iter()
            .filter_map(|e| {
                let etag = e.props.child_text("getetag")?;
                let data = e.props.child_text(data)?;
                Some(Item { href: e.href, etag, data })
            })
            .collect())
    }

    /// Saves an item. With `known` (the version last read here) the save
    /// only happens if the server still has that version; without it, only
    /// if there is nothing at that address yet. Returns the new version if
    /// the server says what it is.
    pub async fn put(&self, href: &str, kind: Kind, data: &str, known: Option<&str>) -> Result<Option<String>> {
        let content_type = match kind {
            Kind::Calendar { .. } => "text/calendar; charset=utf-8",
            Kind::AddressBook => "text/vcard; charset=utf-8",
        };
        let condition = match known {
            Some(etag) => ("If-Match", etag),
            None => ("If-None-Match", "*"),
        };
        let (_, response) = self.request("PUT", href, &[("Content-Type", content_type), condition], data.as_bytes()).await?;
        check(&response, "saving")?;
        Ok(response.header("etag").map(str::to_string))
    }

    /// Deletes an item, if the server still has the version last read here.
    /// One that is already gone counts as deleted.
    pub async fn delete(&self, href: &str, known: Option<&str>) -> Result<()> {
        let headers: Vec<(&str, &str)> = known.map(|etag| ("If-Match", etag)).into_iter().collect();
        let (_, response) = self.request("DELETE", href, &headers, b"").await?;
        match check(&response, "deleting") {
            Err(DavError::NotFound) => Ok(()),
            other => other,
        }
    }

    /// Creates a calendar (for events, tasks, or both) or an address book.
    pub async fn create_collection(&self, href: &str, name: &str, kind: Kind) -> Result<()> {
        let (types, extra) = match kind {
            Kind::Calendar { events, tasks } => {
                let mut comps = String::new();
                for (wanted, comp) in [(events, "VEVENT"), (tasks, "VTODO")] {
                    if wanted {
                        comps.push_str(&format!(r#"<c:comp name="{comp}"/>"#));
                    }
                }
                ("<d:collection/><c:calendar/>", format!("<c:supported-calendar-component-set>{comps}</c:supported-calendar-component-set>"))
            }
            Kind::AddressBook => ("<d:collection/><card:addressbook/>", String::new()),
        };
        let body = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><d:mkcol xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:card="urn:ietf:params:xml:ns:carddav"><d:set><d:prop><d:resourcetype>{types}</d:resourcetype><d:displayname>{}</d:displayname>{extra}</d:prop></d:set></d:mkcol>"#,
            escape(name)
        );
        let (_, response) = self
            .request("MKCOL", href, &[("Content-Type", "application/xml; charset=utf-8")], body.as_bytes())
            .await?;
        check(&response, "creating a collection")
    }
}

fn check(response: &Response, doing: &str) -> Result<()> {
    match response.status {
        200..=299 => Ok(()),
        401 | 403 => Err(DavError::SignIn),
        404 | 410 => Err(DavError::NotFound),
        409 | 412 => Err(DavError::Changed),
        status => Err(DavError::Refused { status, doing: doing.to_string() }),
    }
}

/// Whether two paths name the same place, however each was written.
pub fn same_path(a: &str, b: &str) -> bool {
    let tidy = |p: &str| {
        let p = p.split_once("://").map_or(p, |(_, rest)| rest.find('/').map_or("/", |i| &rest[i..]));
        unescape_path(p.trim_end_matches('/'))
    };
    tidy(a) == tidy(b)
}

fn unescape_path(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let Some(byte) = path.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn collection_of(entry: &Entry) -> Option<Collection> {
    let types = entry.props.child("resourcetype")?;
    let kind = if types.child("calendar").is_some() {
        // A calendar that does not say what it holds holds anything.
        let comps: Vec<&str> = entry
            .props
            .child("supported-calendar-component-set")
            .map(|set| set.children.iter().filter_map(|c| c.attribute("name")).collect())
            .unwrap_or_default();
        let has = |name: &str| comps.is_empty() || comps.iter().any(|c| c.eq_ignore_ascii_case(name));
        let (events, tasks) = (has("VEVENT"), has("VTODO"));
        if !events && !tasks {
            return None;
        }
        Kind::Calendar { events, tasks }
    } else if types.child("addressbook").is_some() {
        Kind::AddressBook
    } else {
        return None;
    };
    let name = entry.props.child_text("displayname").filter(|n| !n.trim().is_empty()).unwrap_or_else(|| {
        let path = unescape_path(entry.href.trim_end_matches('/'));
        path.rsplit('/').next().unwrap_or("").to_string()
    });
    // Apple's colours carry an alpha channel: #RRGGBBAA.
    let color = entry
        .props
        .child_text("calendar-color")
        .map(|c| c.trim().to_string())
        .filter(|c| c.starts_with('#') && c.len() >= 7 && c[1..7].chars().all(|d| d.is_ascii_hexdigit()))
        .map(|c| c[..7].to_ascii_lowercase());
    Some(Collection { href: entry.href.clone(), name: name.trim().to_string(), kind, color, ctag: entry.props.child_text("getctag") })
}

// ---- reading the server's XML ---------------------------------------------

/// A piece of XML: its name without any prefix, its attributes, the text
/// directly inside it, and what is inside it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Element {
    pub name: String,
    pub attributes: Vec<(String, String)>,
    pub text: String,
    pub children: Vec<Element>,
}

impl Element {
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn child_text(&self, name: &str) -> Option<String> {
        self.child(name).map(|c| c.text.clone())
    }

    pub fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
}

/// One `response` of a multi-status answer: an address, and the properties
/// the server could give for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub href: String,
    pub props: Element,
}

/// Reads a multi-status answer. Properties the server reported as missing
/// (a `propstat` with a status other than 200) are left out.
pub fn entries(xml: &str) -> Result<Vec<Entry>> {
    let root = parse_xml(xml)?;
    let mut out = Vec::new();
    for response in root.children.iter().filter(|c| c.name == "response") {
        let Some(href) = response.child_text("href") else { continue };
        let mut props = Element { name: "prop".into(), ..Element::default() };
        for propstat in response.children.iter().filter(|c| c.name == "propstat") {
            let ok = propstat.child_text("status").is_none_or(|s| s.split_whitespace().nth(1) == Some("200"));
            if !ok {
                continue;
            }
            if let Some(prop) = propstat.child("prop") {
                props.children.extend(prop.children.iter().cloned());
            }
        }
        out.push(Entry { href: href.trim().to_string(), props });
    }
    Ok(out)
}

fn parse_xml(xml: &str) -> Result<Element> {
    use quick_xml::events::Event;
    let bad = |e: String| DavError::Unreadable(e);
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut stack: Vec<Element> = vec![Element::default()];
    let local = |name: &str| name.rsplit(':').next().unwrap_or("").to_ascii_lowercase();
    let start = |e: &quick_xml::events::BytesStart| Element {
        name: local(e.name().as_ref()),
        attributes: e
            .attributes()
            .filter_map(|a| a.ok())
            .map(|a| (local(a.key.as_ref()), a.value.to_string()))
            .collect(),
        ..Element::default()
    };
    loop {
        match reader.read_event().map_err(|e| bad(e.to_string()))? {
            Event::Start(e) => stack.push(start(&e)),
            Event::Empty(e) => {
                let element = start(&e);
                stack.last_mut().expect("the root is never popped").children.push(element);
            }
            Event::End(_) => {
                if stack.len() < 2 {
                    return Err(bad("a closing tag with nothing open".into()));
                }
                let done = stack.pop().expect("checked");
                stack.last_mut().expect("checked").children.push(done);
            }
            Event::Text(t) => {
                stack.last_mut().expect("the root is never popped").text.push_str(&t);
            }
            Event::CData(t) => {
                stack.last_mut().expect("the root is never popped").text.push_str(&t);
            }
            Event::GeneralRef(r) => {
                let name: &str = &r;
                let c = match name {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    n => n
                        .strip_prefix("#x")
                        .and_then(|h| u32::from_str_radix(h, 16).ok())
                        .or_else(|| n.strip_prefix('#').and_then(|d| d.parse().ok()))
                        .and_then(char::from_u32),
                };
                if let Some(c) = c {
                    stack.last_mut().expect("the root is never popped").text.push(c);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if stack.len() != 1 {
        return Err(bad("the answer ends in the middle".into()));
    }
    let mut root = stack.pop().expect("checked");
    match root.children.len() {
        1 => Ok(root.children.remove(0)),
        _ => Err(bad("not one XML document".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = r##"<?xml version="1.0"?>
<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:CR="urn:ietf:params:xml:ns:carddav" xmlns:CS="http://calendarserver.org/ns/" xmlns:ICAL="http://apple.com/ns/ical/">
  <D:response><D:href>/ann/</D:href><D:propstat><D:prop><D:resourcetype><D:principal/><D:collection/></D:resourcetype></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>
  <D:response><D:href>/ann/work%20cal/</D:href>
    <D:propstat><D:prop>
      <D:resourcetype><D:collection/><C:calendar/></D:resourcetype>
      <D:displayname>Work &amp; travel</D:displayname>
      <ICAL:calendar-color>#FF2968FF</ICAL:calendar-color>
      <C:supported-calendar-component-set><C:comp name="VEVENT"/></C:supported-calendar-component-set>
      <CS:getctag>"tag-1"</CS:getctag>
    </D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>
  </D:response>
  <D:response><D:href>/ann/todo/</D:href>
    <D:propstat><D:prop><D:resourcetype><D:collection/><C:calendar/></D:resourcetype><C:supported-calendar-component-set><C:comp name="VTODO"/></C:supported-calendar-component-set></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>
    <D:propstat><D:prop><D:displayname/><ICAL:calendar-color/></D:prop><D:status>HTTP/1.1 404 Not Found</D:status></D:propstat>
  </D:response>
  <D:response><D:href>/ann/people/</D:href><D:propstat><D:prop><D:resourcetype><D:collection/><CR:addressbook/></D:resourcetype><D:displayname><![CDATA[People <all>]]></D:displayname></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>
  <D:response><D:href>/ann/journal/</D:href><D:propstat><D:prop><D:resourcetype><D:collection/><C:calendar/></D:resourcetype><C:supported-calendar-component-set><C:comp name="VJOURNAL"/></C:supported-calendar-component-set></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>
</D:multistatus>"##;

    #[test]
    fn a_listing_is_read_into_collections() {
        let found: Vec<Collection> = entries(LISTING).unwrap().iter().filter_map(collection_of).collect();
        assert_eq!(
            found,
            [
                Collection {
                    href: "/ann/work%20cal/".into(),
                    name: "Work & travel".into(),
                    kind: Kind::Calendar { events: true, tasks: false },
                    color: Some("#ff2968".into()),
                    ctag: Some("\"tag-1\"".into()),
                },
                // No name given: the last part of its address. A property
                // the server said it does not have is not an empty one.
                Collection {
                    href: "/ann/todo/".into(),
                    name: "todo".into(),
                    kind: Kind::Calendar { events: false, tasks: true },
                    color: None,
                    ctag: None,
                },
                Collection { href: "/ann/people/".into(), name: "People <all>".into(), kind: Kind::AddressBook, color: None, ctag: None },
            ],
            "the principal and the journal-only calendar are not collections Outpost shows"
        );
    }

    #[test]
    fn broken_answers_are_errors_not_empty_lists() {
        assert!(matches!(entries("<D:multistatus xmlns:D='DAV:'><D:response>"), Err(DavError::Unreadable(_))));
        assert!(matches!(entries("not xml at all </x>"), Err(DavError::Unreadable(_))));
        assert_eq!(entries("<multistatus xmlns='DAV:'/>").unwrap(), []);
    }

    #[test]
    fn paths_are_compared_however_they_are_written() {
        assert!(same_path("/ann/work%20cal/", "/ann/work cal"));
        assert!(same_path("https://dav.example.com/ann/x/", "/ann/x"));
        assert!(!same_path("/ann/x/", "/ann/xy/"));
        assert_eq!(unescape_path("/a%2Fb/%zz/100%"), "/a/b/%zz/100%");
    }
}
