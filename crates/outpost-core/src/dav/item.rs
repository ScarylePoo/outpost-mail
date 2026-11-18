//! Calendar files (iCalendar) and contact files (vCard): what the index
//! and the interface need out of them, and the changes Outpost makes.
//!
//! A file is never rewritten wholesale. Changing an event's title replaces
//! the one line that holds the title and leaves every other line, including
//! ones Outpost does not understand, exactly as another program wrote it.
//! Reading is likewise by line, except for one thing that is hard and is
//! left to a library: working out when an event happens, time zones and
//! repeats included (`calcard`).

use calcard::common::timezone::Tz;
use calcard::icalendar::dates::TimeOrDelta;
use calcard::icalendar::ICalendar;
use std::str::FromStr;

/// An event, as the index holds it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Event {
    pub uid: String,
    pub summary: String,
    pub location: String,
    pub description: String,
    /// Unix seconds. For an all-day event: midnight UTC of its first day.
    pub start: i64,
    /// Unix seconds, exclusive. For an all-day event: midnight UTC of the
    /// day after its last.
    pub end: i64,
    pub all_day: bool,
    /// The event repeats; `start` and `end` are its first occurrence.
    pub recurring: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Task {
    pub uid: String,
    pub summary: String,
    pub description: String,
    /// Unix seconds; for a date without a time, midnight UTC of that day.
    pub due: Option<i64>,
    pub due_all_day: bool,
    pub completed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Contact {
    pub uid: String,
    pub name: String,
    pub emails: Vec<String>,
    pub phones: Vec<String>,
    pub organisation: String,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Event(Event),
    Task(Task),
    Contact(Contact),
}

// ---- lines ------------------------------------------------------------------

/// One content line: `NAME;params:value`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Line {
    /// Upper case, without any group prefix (`item1.EMAIL` is `EMAIL`).
    name: String,
    /// Everything between the name and the colon, including the leading
    /// semicolon. Upper-cased for matching.
    params: String,
    value: String,
}

/// The content lines of a file, with folded lines joined.
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(rest) = raw.strip_prefix(' ').or_else(|| raw.strip_prefix('\t')) {
            if let Some(last) = lines.last_mut() {
                last.push_str(rest);
                continue;
            }
        }
        if !raw.is_empty() {
            lines.push(raw.to_string());
        }
    }
    lines
}

fn parse_line(line: &str) -> Option<Line> {
    // The name ends at the first `;` or `:`; the value starts at the first
    // `:` that is not inside a quoted parameter.
    let mut quoted = false;
    let mut name_end = None;
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ';' if !quoted && name_end.is_none() => name_end = Some(i),
            ':' if !quoted => {
                let name_end = name_end.unwrap_or(i);
                let name = line[..name_end].rsplit('.').next().unwrap_or("").to_ascii_uppercase();
                return Some(Line { name, params: line[name_end..i].to_ascii_uppercase(), value: line[i + 1..].to_string() });
            }
            _ => {}
        }
    }
    None
}

/// Folds a line at 75 bytes, as the formats ask, never inside a character.
fn fold(line: &str) -> String {
    let mut out = String::with_capacity(line.len() + 8);
    let mut width = 0;
    for c in line.chars() {
        if width + c.len_utf8() > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(c);
        width += c.len_utf8();
    }
    out.push_str("\r\n");
    out
}

/// Text as it is written inside a value: backslash, comma, semicolon and
/// line break escaped. (An unescaped comma makes some servers drop the rest
/// of the text.)
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.trim().chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ',' => out.push_str("\\,"),
            ';' => out.push_str("\\;"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') | Some('N') => out.push('\n'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// Splits a value at unescaped occurrences of `at` (`;` in a name, `,` in a
/// list).
fn split_unescaped(value: &str, at: char) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            parts.last_mut().unwrap().push(c);
            if let Some(next) = chars.next() {
                parts.last_mut().unwrap().push(next);
            }
        } else if c == at {
            parts.push(String::new());
        } else {
            parts.last_mut().unwrap().push(c);
        }
    }
    parts
}

/// The lines of the first `kind` component that is not an override of one
/// occurrence, as (index into `lines`, parsed). Lines of components nested
/// inside it (alarms) are left out.
fn component(lines: &[String], kind: &str) -> Option<(usize, usize, Vec<(usize, Line)>)> {
    let begin = format!("BEGIN:{kind}");
    let end = format!("END:{kind}");
    let mut i = 0;
    while i < lines.len() {
        if !lines[i].eq_ignore_ascii_case(&begin) {
            i += 1;
            continue;
        }
        let start = i;
        let mut depth = 0;
        let mut own = Vec::new();
        let mut finish = None;
        for (j, line) in lines.iter().enumerate().skip(start + 1) {
            if line.eq_ignore_ascii_case(&end) && depth == 0 {
                finish = Some(j);
                break;
            }
            let upper = line.to_ascii_uppercase();
            if upper.starts_with("BEGIN:") {
                depth += 1;
            } else if upper.starts_with("END:") {
                depth -= 1;
            } else if depth == 0 {
                if let Some(parsed) = parse_line(line) {
                    own.push((j, parsed));
                }
            }
        }
        let finish = finish?;
        if !own.iter().any(|(_, l)| l.name == "RECURRENCE-ID") {
            return Some((start, finish, own));
        }
        i = finish + 1;
    }
    None
}

fn value_of(lines: &[(usize, Line)], name: &str) -> Option<String> {
    lines.iter().find(|(_, l)| l.name == name).map(|(_, l)| unescape(&l.value))
}

// ---- reading ------------------------------------------------------------------

fn zone(name: &str) -> Tz {
    Tz::from_str(name).ok().filter(|tz| !tz.is_floating()).unwrap_or(Tz::Tz(chrono_tz_utc()))
}

fn chrono_tz_utc() -> chrono_tz::Tz {
    chrono_tz::Tz::UTC
}

/// Days since 1 January 1970 of a date, by the usual civil-calendar sum.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// (year, month, day) of a day number.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// A date or date-time value as (unix seconds, is a date without a time).
/// A time without a zone is taken to be in `tz`, the user's own.
fn moment(line: &Line, tz: &str) -> Option<(i64, bool)> {
    let value = line.value.trim();
    let digits = |from: usize, to: usize| value.get(from..to).and_then(|d| d.parse::<i64>().ok());
    let (year, month, day) = (digits(0, 4)?, digits(4, 6)?, digits(6, 8)?);
    let midnight = days_from_civil(year, month, day) * 86_400;
    if value.len() == 8 || line.params.contains("VALUE=DATE") && !line.params.contains("DATE-TIME") {
        return Some((midnight, true));
    }
    if value.as_bytes().get(8) != Some(&b'T') {
        return None;
    }
    let seconds = digits(9, 11)? * 3600 + digits(11, 13)? * 60 + digits(13, 15).unwrap_or(0);
    let local = midnight + seconds;
    if value.ends_with('Z') || value.ends_with('z') {
        return Some((local, false));
    }
    // Local time somewhere: named on the line, or the user's own zone.
    let named = line.params.split(';').find_map(|p| p.strip_prefix("TZID=")).map(|z| z.trim_matches('"').to_string());
    let tz = match named {
        // The parameters were upper-cased for matching; zone names are not
        // case-sensitive to look up.
        Some(name) => zone_ignoring_case(&name).unwrap_or_else(|| zone(tz)),
        None => zone(tz),
    };
    use chrono::TimeZone;
    let naive = chrono::DateTime::from_timestamp(local, 0)?.naive_utc();
    let resolved = tz.from_local_datetime(&naive).earliest()?;
    Some((resolved.timestamp(), false))
}

fn zone_ignoring_case(name: &str) -> Option<Tz> {
    if let Ok(tz) = Tz::from_str(name) {
        return Some(tz);
    }
    chrono_tz::TZ_VARIANTS.iter().find(|tz| tz.name().eq_ignore_ascii_case(name)).map(|tz| Tz::Tz(*tz))
}

/// When the events of a calendar file happen between `from` and `to` (unix
/// seconds), as (start, end, all-day), repeats expanded. `tz` is the user's
/// time zone, for times written without one.
///
/// All-day occurrences are reported as midnight UTC of their days, like
/// [`Event::start`], whatever the zone.
pub fn occurrences(text: &str, tz: &str, from: i64, to: i64) -> Vec<(i64, i64, bool)> {
    let Ok(calendar) = ICalendar::parse(text) else { return Vec::new() };
    let lines = unfold(text);
    let start_line = component(&lines, "VEVENT").and_then(|(_, _, own)| own.into_iter().find(|(_, l)| l.name == "DTSTART"));
    let all_day = start_line.as_ref().is_some_and(|(_, l)| l.value.trim().len() == 8);
    // The library applies the default zone to a repeating event whose start
    // is written in UTC, as if it were a time without a zone (seen with
    // calcard 0.3.14; a single event is not affected). Such an event is
    // therefore expanded with UTC as the default, which gives the right
    // moments. `repeats_written_in_utc_stay_in_utc` holds this in place.
    let in_utc = start_line.as_ref().is_some_and(|(_, l)| l.value.trim().to_ascii_uppercase().ends_with('Z'));
    let default_zone = if in_utc { zone("UTC") } else { zone(tz) };
    // Enough for a daily event over fifty years.
    let expanded = calendar.expand_dates(default_zone, 20_000);
    let mut out: Vec<(i64, i64, bool)> = expanded
        .events
        .iter()
        .filter(|e| {
            calendar
                .component_by_id(e.comp_id)
                .is_some_and(|c| c.component_type == calcard::icalendar::ICalendarComponentType::VEvent)
        })
        .map(|e| {
            let end = match &e.end {
                TimeOrDelta::Time(t) => *t,
                TimeOrDelta::Delta(d) => e.start + *d,
            };
            if all_day {
                let day = |t: &chrono::DateTime<Tz>| {
                    use chrono::Datelike;
                    let d = t.naive_local().date();
                    days_from_civil(d.year() as i64, d.month() as i64, d.day() as i64) * 86_400
                };
                let start = day(&e.start);
                (start, day(&end).max(start + 86_400), true)
            } else {
                (e.start.timestamp(), end.timestamp().max(e.start.timestamp()), false)
            }
        })
        .filter(|(start, end, _)| *start < to && (*end > from || (*end == *start && *start >= from)))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Reads a calendar file: its event, or failing that its task.
pub fn read_calendar(text: &str, tz: &str) -> Option<Item> {
    let lines = unfold(text);
    if let Some((_, _, own)) = component(&lines, "VEVENT") {
        let start_line = own.iter().find(|(_, l)| l.name == "DTSTART").map(|(_, l)| l)?;
        let (start, all_day) = moment(start_line, tz)?;
        let recurring = own.iter().any(|(_, l)| l.name == "RRULE" || l.name == "RDATE");
        // The first occurrence, from the library when it can say (it knows
        // durations and odd cases), from the lines otherwise.
        let first = occurrences(text, tz, i64::MIN / 2, i64::MAX / 2).into_iter().next();
        let end = match first {
            Some((s, e, _)) if s == start => e,
            _ => own
                .iter()
                .find(|(_, l)| l.name == "DTEND")
                .and_then(|(_, l)| moment(l, tz))
                .map(|(end, _)| end)
                .unwrap_or(if all_day { start + 86_400 } else { start }),
        };
        return Some(Item::Event(Event {
            uid: value_of(&own, "UID").unwrap_or_default(),
            summary: value_of(&own, "SUMMARY").unwrap_or_default(),
            location: value_of(&own, "LOCATION").unwrap_or_default(),
            description: value_of(&own, "DESCRIPTION").unwrap_or_default(),
            start,
            end: end.max(start),
            all_day,
            recurring,
        }));
    }
    let (_, _, own) = component(&lines, "VTODO")?;
    let due = own.iter().find(|(_, l)| l.name == "DUE").and_then(|(_, l)| moment(l, tz));
    let status = value_of(&own, "STATUS").unwrap_or_default();
    Some(Item::Task(Task {
        uid: value_of(&own, "UID").unwrap_or_default(),
        summary: value_of(&own, "SUMMARY").unwrap_or_default(),
        description: value_of(&own, "DESCRIPTION").unwrap_or_default(),
        due: due.map(|d| d.0),
        due_all_day: due.is_some_and(|d| d.1),
        completed: status.eq_ignore_ascii_case("COMPLETED") || own.iter().any(|(_, l)| l.name == "COMPLETED"),
    }))
}

/// Reads a contact file.
pub fn read_card(text: &str) -> Option<Contact> {
    let lines = unfold(text);
    let (_, _, own) = component(&lines, "VCARD")?;
    let all = |name: &str| -> Vec<String> {
        own.iter().filter(|(_, l)| l.name == name).map(|(_, l)| unescape(&l.value).trim().to_string()).filter(|v| !v.is_empty()).collect()
    };
    let mut name = value_of(&own, "FN").unwrap_or_default().trim().to_string();
    if name.is_empty() {
        // No display name: put one together from the name's parts
        // (family; given; ...).
        if let Some((_, n)) = own.iter().find(|(_, l)| l.name == "N") {
            let parts: Vec<String> = split_unescaped(&n.value, ';').iter().map(|p| unescape(p).trim().to_string()).collect();
            let given = parts.get(1).cloned().unwrap_or_default();
            let family = parts.first().cloned().unwrap_or_default();
            name = format!("{given} {family}").trim().to_string();
        }
    }
    let organisation = own
        .iter()
        .find(|(_, l)| l.name == "ORG")
        .map(|(_, l)| split_unescaped(&l.value, ';').iter().map(|p| unescape(p).trim().to_string()).filter(|p| !p.is_empty()).collect::<Vec<_>>().join(", "))
        .unwrap_or_default();
    Some(Contact {
        uid: value_of(&own, "UID").unwrap_or_default(),
        name,
        emails: all("EMAIL"),
        phones: all("TEL"),
        organisation,
        note: value_of(&own, "NOTE").unwrap_or_default(),
    })
}

// ---- writing ------------------------------------------------------------------

fn stamp(unix: i64) -> String {
    let (y, m, d) = civil_from_days(unix.div_euclid(86_400));
    let s = unix.rem_euclid(86_400);
    format!("{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z", s / 3600, s % 3600 / 60, s % 60)
}

fn date(unix: i64) -> String {
    let (y, m, d) = civil_from_days(unix.div_euclid(86_400));
    format!("{y:04}{m:02}{d:02}")
}

/// Replaces properties of the first `kind` component. Each change is a
/// property name and its new line (`None` removes it). Everything else in
/// the file is kept as it was, in order.
fn change(text: &str, kind: &str, changes: &[(&str, Option<String>)]) -> Option<String> {
    let lines = unfold(text);
    let (_, finish, own) = component(&lines, kind)?;
    let dropped: Vec<usize> = own.iter().filter(|(_, l)| changes.iter().any(|(name, _)| l.name == *name)).map(|(i, _)| *i).collect();
    let mut out = String::with_capacity(text.len() + 128);
    for (i, line) in lines.iter().enumerate() {
        if i == finish {
            for (_, new) in changes {
                if let Some(new) = new {
                    out.push_str(&fold(new));
                }
            }
        }
        if !dropped.contains(&i) {
            out.push_str(&fold(line));
        }
    }
    Some(out)
}

/// What the user can set on an event. Times are unix seconds; for an
/// all-day event, midnights UTC as in [`Event`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EventEdit {
    pub summary: String,
    pub location: String,
    pub description: String,
    pub start: i64,
    pub end: i64,
    pub all_day: bool,
}

fn optional(name: &str, text: &str) -> Option<String> {
    (!text.trim().is_empty()).then(|| format!("{name}:{}", escape(text)))
}

fn event_changes(edit: &EventEdit, now: i64) -> Vec<(&'static str, Option<String>)> {
    let (start, end) = if edit.all_day {
        let end = edit.end.max(edit.start + 86_400);
        (format!("DTSTART;VALUE=DATE:{}", date(edit.start)), format!("DTEND;VALUE=DATE:{}", date(end)))
    } else {
        (format!("DTSTART:{}", stamp(edit.start)), format!("DTEND:{}", stamp(edit.end.max(edit.start))))
    };
    vec![
        ("DTSTAMP", Some(format!("DTSTAMP:{}", stamp(now)))),
        ("LAST-MODIFIED", Some(format!("LAST-MODIFIED:{}", stamp(now)))),
        ("DTSTART", Some(start)),
        ("DTEND", Some(end)),
        // A duration would contradict the new end.
        ("DURATION", None),
        ("SUMMARY", Some(format!("SUMMARY:{}", escape(&edit.summary)))),
        ("LOCATION", optional("LOCATION", &edit.location)),
        ("DESCRIPTION", optional("DESCRIPTION", &edit.description)),
    ]
}

fn wrap_calendar(kind: &str, uid: &str) -> String {
    format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Outpost Mail//EN\r\nBEGIN:{kind}\r\nUID:{}\r\nEND:{kind}\r\nEND:VCALENDAR\r\n", escape(uid))
}

/// A new calendar file holding one event.
pub fn new_event(uid: &str, edit: &EventEdit, now: i64) -> String {
    change(&wrap_calendar("VEVENT", uid), "VEVENT", &event_changes(edit, now)).expect("the file just written has an event")
}

/// The event's file with the user's changes. `None` if it holds no event.
/// A repeating event keeps its repeat rule; its sequence number goes up so
/// other programs see the change.
pub fn edit_event(text: &str, edit: &EventEdit, now: i64) -> Option<String> {
    let lines = unfold(text);
    let (_, _, own) = component(&lines, "VEVENT")?;
    let sequence = value_of(&own, "SEQUENCE").and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(0) + 1;
    let mut changes = event_changes(edit, now);
    changes.push(("SEQUENCE", Some(format!("SEQUENCE:{sequence}"))));
    change(text, "VEVENT", &changes)
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskEdit {
    pub summary: String,
    pub description: String,
    /// A day (midnight UTC), or nothing.
    pub due: Option<i64>,
    pub completed: bool,
}

fn task_changes(edit: &TaskEdit, now: i64) -> Vec<(&'static str, Option<String>)> {
    vec![
        ("DTSTAMP", Some(format!("DTSTAMP:{}", stamp(now)))),
        ("LAST-MODIFIED", Some(format!("LAST-MODIFIED:{}", stamp(now)))),
        ("SUMMARY", Some(format!("SUMMARY:{}", escape(&edit.summary)))),
        ("DESCRIPTION", optional("DESCRIPTION", &edit.description)),
        ("DUE", edit.due.map(|due| format!("DUE;VALUE=DATE:{}", date(due)))),
        ("STATUS", Some(format!("STATUS:{}", if edit.completed { "COMPLETED" } else { "NEEDS-ACTION" }))),
        ("COMPLETED", edit.completed.then(|| format!("COMPLETED:{}", stamp(now)))),
        ("PERCENT-COMPLETE", edit.completed.then(|| "PERCENT-COMPLETE:100".to_string())),
    ]
}

pub fn new_task(uid: &str, edit: &TaskEdit, now: i64) -> String {
    change(&wrap_calendar("VTODO", uid), "VTODO", &task_changes(edit, now)).expect("the file just written has a task")
}

/// The task's file with the user's changes. A due time set elsewhere is
/// kept unless the day was changed here.
pub fn edit_task(text: &str, edit: &TaskEdit, now: i64, tz: &str) -> Option<String> {
    let mut changes = task_changes(edit, now);
    if let Some(Item::Task(before)) = read_calendar(text, tz) {
        let same_day = match (before.due, edit.due) {
            (Some(a), Some(b)) => a.div_euclid(86_400) == b.div_euclid(86_400) || (!before.due_all_day && a == b),
            (None, None) => true,
            _ => false,
        };
        if same_day {
            changes.retain(|(name, _)| *name != "DUE");
        }
    }
    change(text, "VTODO", &changes)
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContactEdit {
    pub name: String,
    pub emails: Vec<String>,
    pub phones: Vec<String>,
    pub organisation: String,
    pub note: String,
}

/// The contact's file with the user's changes, or a new file when `text` is
/// `None`. Addresses and numbers that were already there keep their line
/// (and with it their labels, such as "work"); others are added or removed.
pub fn write_card(text: Option<&str>, uid: &str, edit: &ContactEdit) -> Option<String> {
    let fresh = format!("BEGIN:VCARD\r\nVERSION:3.0\r\nUID:{}\r\nEND:VCARD\r\n", escape(uid));
    let text = text.unwrap_or(&fresh);
    let lines = unfold(text);
    let (_, finish, own) = component(&lines, "VCARD")?;
    let name = edit.name.trim();
    let wanted = |list: &[String]| -> Vec<String> { list.iter().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).collect() };
    let (emails, phones) = (wanted(&edit.emails), wanted(&edit.phones));

    let mut dropped: Vec<usize> = Vec::new();
    let mut kept_emails: Vec<String> = Vec::new();
    let mut kept_phones: Vec<String> = Vec::new();
    let mut had_n = None;
    for (i, line) in &own {
        let value = unescape(&line.value).trim().to_string();
        match line.name.as_str() {
            "FN" | "ORG" | "NOTE" => dropped.push(*i),
            "N" => had_n = Some((*i, value)),
            "EMAIL" if emails.iter().any(|e| e.eq_ignore_ascii_case(&value)) && !kept_emails.contains(&value.to_lowercase()) => {
                kept_emails.push(value.to_lowercase())
            }
            "TEL" if phones.contains(&value) && !kept_phones.contains(&value) => kept_phones.push(value),
            "EMAIL" | "TEL" => dropped.push(*i),
            _ => {}
        }
    }
    let mut added: Vec<String> = vec![format!("FN:{}", escape(name))];
    // The structured name is rewritten only if the display name changed (or
    // there was none): "family;given".
    let before = read_card(text).map(|c| c.name).unwrap_or_default();
    if had_n.is_none() || before != name {
        if let Some((i, _)) = had_n {
            dropped.push(i);
        }
        let (given, family) = name.rsplit_once(' ').unwrap_or((name, ""));
        added.push(format!("N:{};{};;;", escape(family), escape(given)));
    }
    if !edit.organisation.trim().is_empty() {
        added.push(format!("ORG:{}", escape(&edit.organisation)));
    }
    for email in emails.iter().filter(|e| !kept_emails.contains(&e.to_lowercase())) {
        added.push(format!("EMAIL:{}", escape(email)));
    }
    for phone in phones.iter().filter(|p| !kept_phones.contains(p)) {
        added.push(format!("TEL:{}", escape(phone)));
    }
    if !edit.note.trim().is_empty() {
        added.push(format!("NOTE:{}", escape(&edit.note)));
    }

    let mut out = String::with_capacity(text.len() + 128);
    for (i, line) in lines.iter().enumerate() {
        if i == finish {
            for new in &added {
                out.push_str(&fold(new));
            }
        }
        if !dropped.contains(&i) {
            out.push_str(&fold(line));
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BERLIN: &str = "Europe/Berlin";

    /// Unix seconds of a UTC date and time.
    fn at(y: i64, m: i64, d: i64, h: i64, min: i64) -> i64 {
        days_from_civil(y, m, d) * 86_400 + h * 3600 + min * 60
    }

    fn ics(body: &str) -> String {
        format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Other//EN\r\n{}\r\nEND:VCALENDAR\r\n", body.trim().replace('\n', "\r\n"))
    }

    #[test]
    fn dates_convert_both_ways() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2026, 11, 18), 20_775);
        for days in [-800_000, -1, 0, 59, 60, 11_016, 20_775, 800_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days, "{y}-{m}-{d}");
        }
        assert_eq!(civil_from_days(days_from_civil(2024, 2, 29)), (2024, 2, 29));
        assert_eq!(stamp(at(2026, 11, 18, 9, 5)), "20261118T090500Z");
        assert_eq!(date(at(2026, 11, 18, 0, 0)), "20261118");
    }

    #[test]
    fn an_event_is_read_with_its_times_in_any_form() {
        // UTC.
        let utc = ics("BEGIN:VEVENT\nUID:a\nDTSTART:20261120T090000Z\nDTEND:20261120T103000Z\nSUMMARY:Planning\\, part two\nLOCATION:Room 4\\; upstairs\nDESCRIPTION:Bring\\nnotes\nEND:VEVENT");
        let Some(Item::Event(e)) = read_calendar(&utc, BERLIN) else { panic!() };
        assert_eq!((e.start, e.end, e.all_day, e.recurring), (at(2026, 11, 20, 9, 0), at(2026, 11, 20, 10, 30), false, false));
        assert_eq!((e.summary.as_str(), e.location.as_str(), e.description.as_str()), ("Planning, part two", "Room 4; upstairs", "Bring\nnotes"));

        // A named zone: 09:00 in New York in November is 14:00 UTC.
        let zoned = ics("BEGIN:VEVENT\nUID:b\nDTSTART;TZID=America/New_York:20261120T090000\nDTEND;TZID=America/New_York:20261120T100000\nSUMMARY:Call\nEND:VEVENT");
        let Some(Item::Event(e)) = read_calendar(&zoned, BERLIN) else { panic!() };
        assert_eq!((e.start, e.end), (at(2026, 11, 20, 14, 0), at(2026, 11, 20, 15, 0)));

        // No zone at all: the user's own. 09:00 in Berlin in November is 08:00 UTC.
        let floating = ics("BEGIN:VEVENT\nUID:c\nDTSTART:20261120T090000\nDURATION:PT45M\nSUMMARY:Dentist\nEND:VEVENT");
        let Some(Item::Event(e)) = read_calendar(&floating, BERLIN) else { panic!() };
        assert_eq!((e.start, e.end), (at(2026, 11, 20, 8, 0), at(2026, 11, 20, 8, 45)));

        // All day: the days, whatever the zone. Two days here.
        let days = ics("BEGIN:VEVENT\nUID:d\nDTSTART;VALUE=DATE:20261224\nDTEND;VALUE=DATE:20261226\nSUMMARY:Holiday\nEND:VEVENT");
        for tz in [BERLIN, "Pacific/Auckland", "America/Los_Angeles"] {
            let Some(Item::Event(e)) = read_calendar(&days, tz) else { panic!() };
            assert_eq!((e.start, e.end, e.all_day), (at(2026, 12, 24, 0, 0), at(2026, 12, 26, 0, 0), true), "{tz}");
        }
        // One day with no end given.
        let one = ics("BEGIN:VEVENT\nUID:e\nDTSTART;VALUE=DATE:20261224\nSUMMARY:Birthday\nEND:VEVENT");
        let Some(Item::Event(e)) = read_calendar(&one, BERLIN) else { panic!() };
        assert_eq!(e.end - e.start, 86_400);

        // A long line folded by another program, and a nested alarm whose
        // own lines are not the event's.
        let folded = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:f\r\nDTSTART:20261120T090000Z\r\nSUMMARY:A very long\r\n  title that was\r\n\t folded\r\nBEGIN:VALARM\r\nDESCRIPTION:Reminder text\r\nSUMMARY:Alarm\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let Some(Item::Event(e)) = read_calendar(folded, BERLIN) else { panic!() };
        assert_eq!((e.summary.as_str(), e.description.as_str()), ("A very long title that was folded", ""));

        assert_eq!(read_calendar("BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n", BERLIN), None);
        assert_eq!(read_calendar("nonsense", BERLIN), None);
    }

    #[test]
    fn repeats_are_expanded_in_the_events_own_zone() {
        // Weekly at 09:00 Berlin time, across the end of summer time: the
        // clock time stays, so the UTC time moves by an hour.
        let weekly = ics("BEGIN:VEVENT\nUID:w\nDTSTART;TZID=Europe/Berlin:20261012T090000\nDTEND;TZID=Europe/Berlin:20261012T100000\nRRULE:FREQ=WEEKLY;COUNT=5\nEXDATE;TZID=Europe/Berlin:20261019T090000\nSUMMARY:Standup\nEND:VEVENT");
        let Some(Item::Event(e)) = read_calendar(&weekly, "UTC") else { panic!() };
        assert!(e.recurring);
        assert_eq!(e.start, at(2026, 10, 12, 7, 0));
        let all = occurrences(&weekly, "UTC", 0, i64::MAX / 2);
        let starts: Vec<i64> = all.iter().map(|o| o.0).collect();
        assert_eq!(
            starts,
            [at(2026, 10, 12, 7, 0), at(2026, 10, 26, 8, 0), at(2026, 11, 2, 8, 0), at(2026, 11, 9, 8, 0)],
            "the excluded week is missing, and the hour shifts when the clocks go back"
        );
        assert!(all.iter().all(|o| o.1 - o.0 == 3600 && !o.2));
        // Only what falls in the window asked for.
        let window = occurrences(&weekly, "UTC", at(2026, 10, 20, 0, 0), at(2026, 11, 3, 0, 0));
        assert_eq!(window.len(), 2);
        // One occurrence moved to another time by an override.
        let moved = ics("BEGIN:VEVENT\nUID:m\nDTSTART:20261102T090000Z\nDTEND:20261102T100000Z\nRRULE:FREQ=DAILY;COUNT=3\nSUMMARY:Daily\nEND:VEVENT\nBEGIN:VEVENT\nUID:m\nRECURRENCE-ID:20261103T090000Z\nDTSTART:20261103T150000Z\nDTEND:20261103T160000Z\nSUMMARY:Daily (moved)\nEND:VEVENT");
        let starts: Vec<i64> = occurrences(&moved, "UTC", 0, i64::MAX / 2).iter().map(|o| o.0).collect();
        assert_eq!(starts, [at(2026, 11, 2, 9, 0), at(2026, 11, 3, 15, 0), at(2026, 11, 4, 9, 0)]);
        let Some(Item::Event(e)) = read_calendar(&moved, "UTC") else { panic!() };
        assert_eq!(e.summary, "Daily", "the series, not the one moved occurrence");
        // A yearly all-day event.
        let birthday = ics("BEGIN:VEVENT\nUID:y\nDTSTART;VALUE=DATE:20200229\nRRULE:FREQ=YEARLY\nSUMMARY:Leap day\nEND:VEVENT");
        let found = occurrences(&birthday, "America/Los_Angeles", at(2024, 1, 1, 0, 0), at(2029, 1, 1, 0, 0));
        assert_eq!(found, [(at(2024, 2, 29, 0, 0), at(2024, 3, 1, 0, 0), true), (at(2028, 2, 29, 0, 0), at(2028, 3, 1, 0, 0), true)]);
    }

    #[test]
    fn repeats_written_in_utc_stay_in_utc() {
        // Whatever this computer's zone, 09:00 UTC is 09:00 UTC.
        let daily = ics("BEGIN:VEVENT\nUID:u\nDTSTART:20261120T090000Z\nDTEND:20261120T100000Z\nRRULE:FREQ=DAILY;COUNT=3\nSUMMARY:Standup\nEND:VEVENT");
        let single = daily.replace("RRULE:FREQ=DAILY;COUNT=3\r\n", "");
        for tz in ["UTC", BERLIN, "America/Los_Angeles", "Pacific/Auckland"] {
            let starts: Vec<i64> = occurrences(&daily, tz, 0, i64::MAX / 2).iter().map(|o| o.0).collect();
            assert_eq!(starts, [at(2026, 11, 20, 9, 0), at(2026, 11, 21, 9, 0), at(2026, 11, 22, 9, 0)], "{tz}");
            assert_eq!(occurrences(&single, tz, 0, i64::MAX / 2), [(at(2026, 11, 20, 9, 0), at(2026, 11, 20, 10, 0), false)], "{tz}");
            let Some(Item::Event(e)) = read_calendar(&daily, tz) else { panic!() };
            assert_eq!((e.start, e.end), (at(2026, 11, 20, 9, 0), at(2026, 11, 20, 10, 0)), "{tz}");
        }
        // A repeat in a named zone is that zone's clock time, whatever the
        // computer's; one with no zone is the computer's clock time.
        let zoned = ics("BEGIN:VEVENT\nUID:z\nDTSTART;TZID=Europe/Berlin:20261120T090000\nDURATION:PT1H\nRRULE:FREQ=DAILY;COUNT=2\nSUMMARY:Z\nEND:VEVENT");
        let floating = ics("BEGIN:VEVENT\nUID:f\nDTSTART:20261120T090000\nDURATION:PT1H\nRRULE:FREQ=DAILY;COUNT=2\nSUMMARY:F\nEND:VEVENT");
        for tz in ["UTC", "America/Los_Angeles"] {
            let starts: Vec<i64> = occurrences(&zoned, tz, 0, i64::MAX / 2).iter().map(|o| o.0).collect();
            assert_eq!(starts, [at(2026, 11, 20, 8, 0), at(2026, 11, 21, 8, 0)], "{tz}");
        }
        let starts: Vec<i64> = occurrences(&floating, "America/Los_Angeles", 0, i64::MAX / 2).iter().map(|o| o.0).collect();
        assert_eq!(starts, [at(2026, 11, 20, 17, 0), at(2026, 11, 21, 17, 0)]);
    }

    #[test]
    fn an_event_is_written_and_changed_without_disturbing_the_rest() {
        let now = at(2026, 11, 18, 12, 0);
        let edit = EventEdit {
            summary: "Lunch, with Dana; maybe".into(),
            location: "Cafe".into(),
            description: "Line one\nLine two".into(),
            start: at(2026, 11, 20, 12, 0),
            end: at(2026, 11, 20, 13, 0),
            all_day: false,
        };
        let text = new_event("uid-1", &edit, now);
        assert!(text.starts_with("BEGIN:VCALENDAR\r\n") && text.ends_with("END:VCALENDAR\r\n"), "{text}");
        assert!(text.contains("SUMMARY:Lunch\\, with Dana\\; maybe\r\n") && text.contains("DESCRIPTION:Line one\\nLine two\r\n"), "{text}");
        let Some(Item::Event(e)) = read_calendar(&text, BERLIN) else { panic!() };
        assert_eq!((e.uid.as_str(), e.summary.as_str(), e.start, e.end), ("uid-1", "Lunch, with Dana; maybe", edit.start, edit.end));
        assert!(ICalendar::parse(&text).is_ok());

        // Another program's event, with things Outpost does not handle.
        let theirs = ics("BEGIN:VEVENT\nUID:t\nDTSTAMP:20260101T000000Z\nSEQUENCE:2\nDTSTART;TZID=Europe/Berlin:20261120T090000\nDURATION:PT1H\nSUMMARY:Old title\nLOCATION:Old place\nX-CUSTOM-THING;FOO=bar:keep me\nATTENDEE;CN=\"Dubois: Dana\":mailto:dana@example.com\nRRULE:FREQ=WEEKLY\nBEGIN:VALARM\nACTION:DISPLAY\nDESCRIPTION:Soon\nTRIGGER:-PT15M\nEND:VALARM\nEND:VEVENT");
        let changed = edit_event(&theirs, &EventEdit { summary: "New title".into(), location: String::new(), ..edit.clone() }, now).unwrap();
        for kept in ["X-CUSTOM-THING;FOO=bar:keep me\r\n", "ATTENDEE;CN=\"Dubois: Dana\":mailto:dana@example.com\r\n", "RRULE:FREQ=WEEKLY\r\n", "BEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:Soon\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\n", "PRODID:-//Other//EN\r\n"] {
            assert!(changed.contains(kept), "lost {kept:?} in\n{changed}");
        }
        assert!(changed.contains("SEQUENCE:3\r\n") && !changed.contains("SEQUENCE:2"), "{changed}");
        assert!(!changed.contains("Old title") && !changed.contains("LOCATION") && !changed.contains("DURATION"), "{changed}");
        assert_eq!(changed.matches("SUMMARY:").count(), 1);
        let Some(Item::Event(e)) = read_calendar(&changed, BERLIN) else { panic!() };
        assert_eq!((e.summary.as_str(), e.start, e.end, e.recurring), ("New title", edit.start, edit.end, true));

        // All day, and a title long enough to need folding.
        let long = "x".repeat(200) + " ü end";
        let day = EventEdit { summary: long.clone(), start: at(2026, 12, 24, 0, 0), end: at(2026, 12, 24, 0, 0), all_day: true, ..EventEdit::default() };
        let text = new_event("uid-2", &day, now);
        assert!(text.lines().all(|l| l.len() <= 75), "{text}");
        assert!(text.contains("DTSTART;VALUE=DATE:20261224\r\n") && text.contains("DTEND;VALUE=DATE:20261225\r\n"), "{text}");
        let Some(Item::Event(e)) = read_calendar(&text, BERLIN) else { panic!() };
        assert_eq!((e.summary, e.all_day, e.end - e.start), (long, true, 86_400));
    }

    #[test]
    fn tasks_are_read_written_and_ticked_off() {
        let now = at(2026, 11, 18, 12, 0);
        let theirs = ics("BEGIN:VTODO\nUID:t1\nSUMMARY:File taxes\nDUE;TZID=Europe/Berlin:20261130T170000\nPRIORITY:1\nSTATUS:NEEDS-ACTION\nEND:VTODO");
        let Some(Item::Task(t)) = read_calendar(&theirs, "UTC") else { panic!() };
        assert_eq!((t.summary.as_str(), t.due, t.due_all_day, t.completed), ("File taxes", Some(at(2026, 11, 30, 16, 0)), false, false));

        // Ticking it off keeps the due time and the priority.
        let done = edit_task(&theirs, &TaskEdit { summary: t.summary.clone(), description: String::new(), due: t.due, completed: true }, now, "UTC").unwrap();
        assert!(done.contains("STATUS:COMPLETED\r\n") && done.contains("COMPLETED:20261118T120000Z\r\n") && done.contains("PRIORITY:1\r\n"), "{done}");
        assert!(done.contains("DUE;TZID=Europe/Berlin:20261130T170000\r\n"), "{done}");
        let Some(Item::Task(t)) = read_calendar(&done, "UTC") else { panic!() };
        assert!(t.completed);
        // Un-ticking removes every sign of completion.
        let undone = edit_task(&done, &TaskEdit { summary: t.summary.clone(), description: String::new(), due: t.due, completed: false }, now, "UTC").unwrap();
        assert!(!undone.contains("COMPLETED") && !undone.contains("PERCENT-COMPLETE") && undone.contains("STATUS:NEEDS-ACTION"), "{undone}");
        // A new day replaces the due time; no day removes it.
        let moved = edit_task(&undone, &TaskEdit { summary: "File taxes".into(), description: String::new(), due: Some(at(2026, 12, 5, 0, 0)), completed: false }, now, "UTC").unwrap();
        assert!(moved.contains("DUE;VALUE=DATE:20261205\r\n") && moved.matches("DUE").count() == 1, "{moved}");
        let none = edit_task(&moved, &TaskEdit { summary: "File taxes".into(), ..TaskEdit::default() }, now, "UTC").unwrap();
        assert!(!none.contains("DUE"), "{none}");

        let text = new_task("uid-t", &TaskEdit { summary: "Buy milk".into(), description: String::new(), due: Some(at(2026, 11, 19, 0, 0)), completed: false }, now);
        let Some(Item::Task(t)) = read_calendar(&text, BERLIN) else { panic!() };
        assert_eq!((t.uid.as_str(), t.summary.as_str(), t.due, t.due_all_day, t.completed), ("uid-t", "Buy milk", Some(at(2026, 11, 19, 0, 0)), true, false));
        assert!(ICalendar::parse(&text).is_ok());
    }

    #[test]
    fn contacts_are_read_and_changed_keeping_what_was_there() {
        let theirs = "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:c1\r\nN:Dubois;Dana;;;\r\nFN:Dana Dubois\r\nORG:Example Corp;IT\r\nitem1.EMAIL;TYPE=INTERNET,WORK:dana@example.com\r\nEMAIL;TYPE=HOME:dana@home.example\r\nTEL;TYPE=CELL:+1 555 0100\r\nNOTE:Met at the\\, conference\r\nPHOTO;ENCODING=b;TYPE=JPEG:AAAA\r\nBDAY:1980-01-02\r\nEND:VCARD\r\n";
        let c = read_card(theirs).unwrap();
        assert_eq!(
            c,
            Contact {
                uid: "c1".into(),
                name: "Dana Dubois".into(),
                emails: vec!["dana@example.com".into(), "dana@home.example".into()],
                phones: vec!["+1 555 0100".into()],
                organisation: "Example Corp, IT".into(),
                note: "Met at the, conference".into(),
            }
        );
        // No display name: made from the parts.
        assert_eq!(read_card("BEGIN:VCARD\r\nN:Okafor;Sam;;;\r\nEND:VCARD\r\n").unwrap().name, "Sam Okafor");

        // One address dropped, one added, the phone kept, the note cleared.
        let changed = write_card(
            Some(theirs),
            "c1",
            &ContactEdit {
                name: "Dana Dubois".into(),
                emails: vec!["DANA@example.com".into(), "dd@new.example".into()],
                phones: vec!["+1 555 0100".into()],
                organisation: "Example Corp".into(),
                note: String::new(),
            },
        )
        .unwrap();
        for kept in ["item1.EMAIL;TYPE=INTERNET,WORK:dana@example.com\r\n", "TEL;TYPE=CELL:+1 555 0100\r\n", "PHOTO;ENCODING=b;TYPE=JPEG:AAAA\r\n", "BDAY:1980-01-02\r\n", "N:Dubois;Dana;;;\r\n", "UID:c1\r\n"] {
            assert!(changed.contains(kept), "lost {kept:?} in\n{changed}");
        }
        assert!(!changed.contains("dana@home.example") && !changed.contains("NOTE") && changed.contains("EMAIL:dd@new.example\r\n"), "{changed}");
        let c = read_card(&changed).unwrap();
        assert_eq!((c.emails.len(), c.organisation.as_str(), c.note.as_str()), (2, "Example Corp", ""));

        // A new contact, and a changed name.
        let new = write_card(None, "c2", &ContactEdit { name: "Priya Raman".into(), emails: vec!["priya@example.com".into(), " ".into()], ..ContactEdit::default() }).unwrap();
        assert!(new.contains("FN:Priya Raman\r\n") && new.contains("N:Raman;Priya;;;\r\n") && new.contains("UID:c2\r\n"), "{new}");
        assert_eq!(read_card(&new).unwrap().emails, ["priya@example.com"]);
        let renamed = write_card(Some(&new), "c2", &ContactEdit { name: "Priya R. Shah".into(), emails: vec!["priya@example.com".into()], ..ContactEdit::default() }).unwrap();
        assert!(renamed.contains("N:Shah;Priya R.;;;\r\n") && renamed.matches("\r\nN:").count() == 1, "{renamed}");
    }
}
