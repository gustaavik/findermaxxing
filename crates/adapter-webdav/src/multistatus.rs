//! Parsing PROPFIND `207 Multi-Status` responses (RFC 4918 §14.16). Only
//! elements in the `DAV:` namespace count, whatever prefix a server uses.

use domain::Timestamp;
use quick_xml::NsReader;
use quick_xml::events::Event;
use quick_xml::name::{Namespace, ResolveResult};

const DAV_NAMESPACE: &str = "DAV:";
const OK_STATUS: &str = " 200 ";

/// One `<response>`: an item and the properties the server reported for it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct DavItem {
    pub href: String,
    pub is_collection: bool,
    pub size: Option<u64>,
    pub modified: Option<Timestamp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Href,
    Length,
    Modified,
    Status,
}

/// Properties of a `<propstat>` only count if its status is 200.
#[derive(Default)]
struct PropStat {
    is_collection: bool,
    size: Option<u64>,
    modified: Option<Timestamp>,
    ok: bool,
}

pub(crate) fn parse(xml: &str) -> Result<Vec<DavItem>, String> {
    let mut reader = NsReader::from_str(xml);
    let mut parser = Parser::default();
    loop {
        let (namespace, event) = reader.read_resolved_event().map_err(|e| e.to_string())?;
        let in_dav = matches!(namespace, ResolveResult::Bound(Namespace(ns)) if ns == DAV_NAMESPACE);
        match event {
            Event::Start(element) | Event::Empty(element) if in_dav => parser.start(element.local_name().as_ref()),
            Event::Text(content) => parser.text.push_str(&content.into_inner()),
            Event::CData(content) => parser.text.push_str(&content.into_inner()),
            Event::GeneralRef(reference) => {
                let raw = format!("&{};", reference.into_inner());
                parser.text.push_str(&quick_xml::escape::unescape(&raw).map_err(|e| e.to_string())?);
            }
            Event::End(element) if in_dav => parser.end(element.local_name().as_ref()),
            Event::Eof => return Ok(parser.items),
            _ => {}
        }
    }
}

#[derive(Default)]
struct Parser {
    items: Vec<DavItem>,
    item: Option<DavItem>,
    propstat: Option<PropStat>,
    field: Option<Field>,
    text: String,
}

impl Parser {
    fn start(&mut self, name: &str) {
        match name {
            "response" => self.item = Some(DavItem::default()),
            "propstat" => self.propstat = Some(PropStat::default()),
            "collection" => {
                if let Some(stat) = self.propstat.as_mut() {
                    stat.is_collection = true;
                }
            }
            "href" => self.field = Some(Field::Href),
            "getcontentlength" => self.field = Some(Field::Length),
            "getlastmodified" => self.field = Some(Field::Modified),
            "status" => self.field = Some(Field::Status),
            _ => {}
        }
        self.text.clear();
    }

    fn end(&mut self, name: &str) {
        let value = std::mem::take(&mut self.text).trim().to_owned();
        match (name, self.field.take()) {
            // A response's own href; propstats carry no href of their own.
            ("href", Some(Field::Href)) if self.propstat.is_none() => {
                if let Some(item) = self.item.as_mut() {
                    item.href = value;
                }
            }
            ("getcontentlength", Some(Field::Length)) => self.with_stat(|stat| stat.size = value.parse().ok()),
            ("getlastmodified", Some(Field::Modified)) => {
                self.with_stat(|stat| stat.modified = parse_http_date(&value))
            }
            ("status", Some(Field::Status)) => self.with_stat(|stat| stat.ok = value.contains(OK_STATUS)),
            ("propstat", _) => {
                if let (Some(stat), Some(item)) = (self.propstat.take(), self.item.as_mut())
                    && stat.ok
                {
                    item.is_collection |= stat.is_collection;
                    item.size = item.size.or(stat.size);
                    item.modified = item.modified.or(stat.modified);
                }
            }
            ("response", _) => self.items.extend(self.item.take()),
            _ => {}
        }
    }

    fn with_stat(&mut self, change: impl FnOnce(&mut PropStat)) {
        if let Some(stat) = self.propstat.as_mut() {
            change(stat);
        }
    }
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const MILLIS_PER_SECOND: i64 = 1000;
const SECONDS_PER_DAY: i64 = 86_400;

/// `Wed, 04 Nov 2026 16:39:18 GMT` (RFC 1123, the form WebDAV mandates).
pub(crate) fn parse_http_date(value: &str) -> Option<Timestamp> {
    let mut parts = value.split_whitespace().skip(1);
    let day: i64 = parts.next()?.parse().ok()?;
    let month = MONTHS.iter().position(|m| Some(*m) == parts.clone().next())?;
    parts.next();
    let year: i64 = parts.next()?.parse().ok()?;
    let mut clock = parts.next()?.split(':').map(|n| n.parse::<i64>().ok());
    let (hour, minute, second) = (clock.next()??, clock.next()??, clock.next()??);
    let days = days_from_civil(year, i64::try_from(month).ok()? + 1, day);
    Some(Timestamp::from_unix_millis(
        ((days * SECONDS_PER_DAY) + hour * 3600 + minute * 60 + second) * MILLIS_PER_SECOND,
    ))
}

/// Days since 1970-01-01 (Howard Hinnant's civil-date algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    const APACHE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:">
<D:response xmlns:lp1="DAV:"><D:href>/probe/</D:href>
<D:propstat><D:prop><lp1:resourcetype><D:collection/></lp1:resourcetype>
<lp1:getlastmodified>Wed, 04 Nov 2026 16:39:18 GMT</lp1:getlastmodified></D:prop>
<D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>
<D:response xmlns:lp1="DAV:"><D:href>/probe/a%20&amp;%20b.txt</D:href>
<D:propstat><D:prop><lp1:resourcetype/><lp1:getcontentlength>213</lp1:getcontentlength></D:prop>
<D:status>HTTP/1.1 200 OK</D:status></D:propstat>
<D:propstat><D:prop><x:quota xmlns:x="urn:other"/><D:getcontentlength>9</D:getcontentlength></D:prop>
<D:status>HTTP/1.1 404 Not Found</D:status></D:propstat></D:response>
</D:multistatus>"#;

    #[test]
    fn parses_items_whatever_the_prefix_and_ignores_failed_propstats() {
        let items = parse(APACHE).unwrap();
        assert_eq!(items.len(), 2);
        assert!(items[0].is_collection);
        assert_eq!(items[0].modified, parse_http_date("Wed, 04 Nov 2026 16:39:18 GMT"));
        assert_eq!(items[1].href, "/probe/a%20&%20b.txt");
        assert_eq!(items[1].size, Some(213), "the 404 propstat's length is ignored");
        assert!(!items[1].is_collection);
    }

    #[test]
    fn http_dates_become_unix_time() {
        assert_eq!(parse_http_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(Timestamp::from_unix_millis(0)));
        assert_eq!(
            parse_http_date("Sun, 06 Nov 1994 08:49:37 GMT"),
            Some(Timestamp::from_unix_millis(784_111_777_000))
        );
        assert_eq!(parse_http_date("not a date"), None);
    }
}
