//! Reading `PROPFIND` answers (`207 Multi-Status`) with `quick-xml`.
//!
//! The parser is deliberately forgiving about namespaces (elements are matched by local name) and
//! strict about everything else it uses: only properties reported with status 200 count, numbers
//! that do not parse are dropped (not guessed), and nothing in the document is ever followed:
//! the `href` values are returned as text and validated by the caller. Custom XML entities are
//! not expanded (an unknown entity is an error), so entity-expansion attacks do not apply.

use crate::error::DavError;
use quick_xml::Reader;
use quick_xml::events::Event;

/// One `response` element.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Prop {
    /// The `href`, still percent-encoded and possibly absolute.
    pub href: String,
    /// `resourcetype` contains `collection`.
    pub is_collection: bool,
    /// `getcontentlength`, if it parsed.
    pub size: Option<u64>,
    /// `getetag`, verbatim.
    pub etag: Option<String>,
    /// `getlastmodified` as ms since the epoch, if it parsed.
    pub mtime_ms: Option<i64>,
    /// ownCloud/Nextcloud `permissions` (letters such as `RGDNVW` or `SGCK`).
    pub oc_permissions: Option<String>,
}

fn month(m: &str) -> Option<i64> {
    const M: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    M.iter()
        .position(|x| x.eq_ignore_ascii_case(m))
        .map(|i| i as i64 + 1)
}

/// Days since 1970-01-01 of a civil date (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Parse an RFC 1123 date (`Tue, 15 Nov 1994 08:12:31 GMT`) into ms since the epoch.
#[must_use]
pub fn parse_http_date(s: &str) -> Option<i64> {
    let s = s.trim();
    let rest = s.split_once(',').map_or(s, |(_, r)| r).trim();
    let mut it = rest.split_whitespace();
    let day: i64 = it.next()?.parse().ok()?;
    let mon = month(it.next()?)?;
    let year: i64 = it.next()?.parse().ok()?;
    let time = it.next()?;
    let mut t = time.split(':');
    let (h, mi, se): (i64, i64, i64) = (
        t.next()?.parse().ok()?,
        t.next()?.parse().ok()?,
        t.next()?.parse().ok()?,
    );
    if !(1..=9999).contains(&year)
        || !(1..=31).contains(&day)
        || !(0..24).contains(&h)
        || !(0..60).contains(&mi)
        || !(0..61).contains(&se)
    {
        return None;
    }
    let days = days_from_civil(year, mon, day);
    Some(((days * 24 + h) * 60 + mi) * 60_000 + se * 1000)
}

fn entity(name: &str) -> Option<char> {
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        _ => {
            let num = name.strip_prefix('#')?;
            let code = match num.strip_prefix(['x', 'X']) {
                Some(h) => u32::from_str_radix(h, 16).ok()?,
                None => num.parse().ok()?,
            };
            char::from_u32(code)?
        }
    })
}

#[derive(Default)]
struct PropState {
    cur: Prop,
    /// Text of the property element being read.
    text: String,
    /// Properties seen inside the current `propstat`.
    stat: Prop,
    stat_ok: bool,
    stat_text: String,
}

/// Parse a multistatus body.
///
/// # Errors
/// [`DavError::Protocol`] for malformed XML.
pub fn parse_multistatus(xml: &[u8]) -> Result<Vec<Prop>, DavError> {
    let bad = |m: &str| DavError::Protocol(format!("malformed listing ({m})"));
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut out = Vec::new();
    let mut st = PropState::default();
    loop {
        let ev = reader
            .read_event_into(&mut buf)
            .map_err(|e| bad(&e.to_string()))?;
        match ev {
            Event::Start(e) => {
                let name = e.local_name().as_ref().to_owned();
                match name.as_str() {
                    "response" => st = PropState::default(),
                    "propstat" => {
                        st.stat = Prop::default();
                        st.stat_ok = false;
                        st.stat_text.clear();
                    }
                    _ => {}
                }
                st.text.clear();
                stack.push(name);
            }
            Event::Empty(e) => {
                let name = e.local_name().as_ref().to_owned();
                if name == "collection" && stack.last().is_some_and(|p| p == "resourcetype") {
                    st.stat.is_collection = true;
                }
            }
            Event::Text(t) => {
                st.text.push_str(&t);
            }
            Event::CData(c) => {
                st.text.push_str(&c);
            }
            Event::GeneralRef(r) => {
                st.text
                    .push(entity(&r).ok_or_else(|| bad("unknown entity"))?);
            }
            Event::End(e) => {
                let name = e.local_name().as_ref().to_owned();
                let parent = stack
                    .len()
                    .checked_sub(2)
                    .and_then(|i| stack.get(i))
                    .cloned();
                let text = std::mem::take(&mut st.text);
                let text = text.trim();
                match name.as_str() {
                    "href" if parent.as_deref() == Some("response") => {
                        st.cur.href = text.to_owned();
                    }
                    "status" if parent.as_deref() == Some("propstat") => {
                        // "HTTP/1.1 200 OK"
                        st.stat_ok = text.split_whitespace().nth(1) == Some("200");
                    }
                    "getcontentlength" => st.stat.size = text.parse().ok(),
                    "getetag" if !text.is_empty() => st.stat.etag = Some(text.to_owned()),
                    "getlastmodified" => st.stat.mtime_ms = parse_http_date(text),
                    "permissions" if !text.is_empty() => {
                        st.stat.oc_permissions = Some(text.to_owned());
                    }
                    "propstat" => {
                        if st.stat_ok {
                            let p = std::mem::take(&mut st.stat);
                            st.cur.is_collection |= p.is_collection;
                            st.cur.size = st.cur.size.or(p.size);
                            st.cur.etag = st.cur.etag.take().or(p.etag);
                            st.cur.mtime_ms = st.cur.mtime_ms.or(p.mtime_ms);
                            st.cur.oc_permissions =
                                st.cur.oc_permissions.take().or(p.oc_permissions);
                        }
                        st.stat = Prop::default();
                    }
                    "response" => {
                        let done = std::mem::take(&mut st.cur);
                        if !done.href.is_empty() {
                            out.push(done);
                        }
                    }
                    _ => {}
                }
                stack.pop();
            }
            Event::Eof => break,
            Event::Comment(_) | Event::Decl(_) | Event::PI(_) | Event::DocType(_) => {}
        }
        buf.clear();
    }
    if !stack.is_empty() {
        return Err(bad("truncated"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:oc="http://owncloud.org/ns">
 <d:response><d:href>/dav/Notes/d/</d:href>
  <d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype><oc:permissions>SGCK</oc:permissions></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
 </d:response>
 <d:response><d:href>/dav/Notes/h/00000001.nvhdr</d:href>
  <d:propstat><d:prop><d:resourcetype/><d:getcontentlength>42</d:getcontentlength><d:getetag>"abc"</d:getetag><d:getlastmodified>Tue, 15 Nov 1994 08:12:31 GMT</d:getlastmodified></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
  <d:propstat><d:prop><oc:permissions/></d:prop><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat>
 </d:response>
 <d:response><d:href>/dav/Notes/a%26b.txt</d:href>
  <d:propstat><d:prop><d:getcontentlength>nope</d:getcontentlength></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
 </d:response>
</d:multistatus>"#;

    #[test]
    fn parses_collections_files_and_ignores_failed_propstats() {
        let r = parse_multistatus(SAMPLE.as_bytes()).unwrap();
        assert_eq!(r.len(), 3);
        assert!(r[0].is_collection);
        assert_eq!(r[0].oc_permissions.as_deref(), Some("SGCK"));
        assert!(!r[1].is_collection);
        assert_eq!(r[1].size, Some(42));
        assert_eq!(r[1].etag.as_deref(), Some("\"abc\""));
        assert_eq!(r[1].mtime_ms, Some(784_887_151_000));
        assert_eq!(r[2].size, None);
        assert_eq!(r[2].href, "/dav/Notes/a%26b.txt");
    }

    #[test]
    fn malformed_and_hostile_documents_are_errors_not_panics() {
        assert!(parse_multistatus(b"<d:multistatus><d:response>").is_err());
        let bomb = r#"<!DOCTYPE x [<!ENTITY a "aaaa">]><d:multistatus xmlns:d="DAV:"><d:response><d:href>&a;</d:href></d:response></d:multistatus>"#;
        assert!(parse_multistatus(bomb.as_bytes()).is_err());
        assert!(
            parse_multistatus(b"not xml at all <<<").is_err()
                || parse_multistatus(b"x").unwrap().is_empty()
        );
        assert_eq!(parse_multistatus(b"").unwrap().len(), 0);
    }

    #[test]
    fn absurd_years_are_rejected_not_overflowed() {
        // Found by fuzzing: a huge year overflowed the millisecond arithmetic.
        assert_eq!(
            parse_http_date("Tue, 15 Nov 9223372036854775807 08:12:31 GMT"),
            None
        );
        assert_eq!(parse_http_date("Tue, 15 Nov 10000 08:12:31 GMT"), None);
        assert!(parse_http_date("Tue, 15 Nov 9999 08:12:31 GMT").is_some());
    }

    #[test]
    fn http_dates() {
        assert_eq!(parse_http_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        assert_eq!(parse_http_date("garbage"), None);
        assert_eq!(parse_http_date("Tue, 99 Nov 1994 08:12:31 GMT"), None);
    }
}
