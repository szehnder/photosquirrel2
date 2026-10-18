//! XMP sidecars (ADR-0022): `<stem>.xmp` next to the exported file(s), which Lightroom and Apple Photos read.
//!
//! Content: `dc:subject` keywords (effective insight keywords, user tags, people and pet counts),
//! `dc:description`, and PhotoSquirrel provenance in its own namespace (content identity, full hash, the
//! exported files and every duplicate's source path). A RAW+JPEG set or a Live Photo pair shares one stem and
//! therefore one sidecar. Sidecars are written with `quick-xml` and parsed back ([`parse`]) before they are
//! renamed into place; AI content only ever affects sidecar text, never what is exported (ADR-0021).

use std::collections::HashSet;

use quick_xml::Writer;
use quick_xml::events::{BytesDecl, BytesEnd, BytesPI, BytesStart, BytesText, Event};
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;

use crate::input::ExportUnit;

pub const NS_X: &str = "adobe:ns:meta/";
pub const NS_RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
pub const NS_DC: &str = "http://purl.org/dc/elements/1.1/";
pub const NS_PS: &str = "https://photosquirrel.app/ns/xmp/1.0/";

/// What a sidecar says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sidecar {
    pub subjects: Vec<String>,
    pub description: Option<String>,
    /// Catalog identity (hex) of the primary file.
    pub content_hash: String,
    pub identity_kind: String,
    /// Full BLAKE3 (hex) of the primary file, when known.
    pub full_hash: Option<String>,
    /// `<library file name> <full hash hex>` for each file sharing the stem.
    pub files: Vec<String>,
    /// Source paths of every copy.
    pub sources: Vec<String>,
    pub people: Option<u32>,
    pub pets: Option<u32>,
}

fn plural(n: u32, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// Keywords and description of the units sharing a sidecar: effective insight keywords, user tags, and
/// people/pet counts as keywords ("1 person", "2 pets"). Case-insensitively deduplicated, in order.
pub fn subjects_of(
    units: &[&ExportUnit],
) -> (Vec<String>, Option<String>, Option<u32>, Option<u32>) {
    let mut out = Vec::new();
    let mut description = None;
    let (mut people, mut pets) = (None, None);
    for u in units {
        if let Some(i) = &u.insight {
            out.extend(i.keywords());
            if i.people.total > 0 {
                out.push(plural(i.people.total, "person", "people"));
                if i.people.children > 0 {
                    out.push(plural(i.people.children, "child", "children"));
                }
            }
            if !i.pets.is_empty() {
                out.push(plural(i.pets.len() as u32, "pet", "pets"));
            }
            people = people.or(Some(i.people.total));
            pets = pets.or(Some(i.pets.len() as u32));
            if description.is_none() && !i.subject.trim().is_empty() {
                description = Some(i.subject.clone());
            }
        }
        out.extend(u.user_tags.iter().cloned());
        if let Some(d) = &u.description {
            description = Some(d.clone());
        }
    }
    let mut seen = HashSet::new();
    out.retain(|k| !k.trim().is_empty() && seen.insert(k.to_lowercase()));
    (out, description, people, pets)
}

/// Drop characters XML 1.0 cannot carry (C0 controls other than tab/newline/CR, U+FFFE, U+FFFF).
fn clean(s: &str) -> String {
    s.chars()
        .filter(|&c| {
            matches!(c, '\t' | '\n' | '\r') || (c >= ' ' && c != '\u{FFFE}' && c != '\u{FFFF}')
        })
        .collect()
}

type W = Writer<Vec<u8>>;

fn text(w: &mut W, name: &str, value: &str) -> std::io::Result<()> {
    w.create_element(name)
        .write_text_content(BytesText::new(&clean(value)))?;
    Ok(())
}

fn bag(w: &mut W, name: &str, items: &[String]) -> std::io::Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    w.write_event(Event::Start(BytesStart::new(name)))?;
    w.write_event(Event::Start(BytesStart::new("rdf:Bag")))?;
    for i in items {
        text(w, "rdf:li", i)?;
    }
    w.write_event(Event::End(BytesEnd::new("rdf:Bag")))?;
    w.write_event(Event::End(BytesEnd::new(name)))?;
    Ok(())
}

/// Render a sidecar as an XMP packet.
pub fn render(s: &Sidecar) -> Vec<u8> {
    let mut w = Writer::new_with_indent(Vec::new(), b' ', 1);
    // Writing into a Vec cannot fail.
    let _ = (|| -> std::io::Result<()> {
        w.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;
        w.write_event(Event::PI(BytesPI::new(
            "xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"",
        )))?;
        w.write_event(Event::Start(
            BytesStart::new("x:xmpmeta").with_attributes([("xmlns:x", NS_X)]),
        ))?;
        w.write_event(Event::Start(
            BytesStart::new("rdf:RDF").with_attributes([("xmlns:rdf", NS_RDF)]),
        ))?;
        w.write_event(Event::Start(
            BytesStart::new("rdf:Description").with_attributes([
                ("rdf:about", ""),
                ("xmlns:dc", NS_DC),
                ("xmlns:photosquirrel", NS_PS),
            ]),
        ))?;
        bag(&mut w, "dc:subject", &s.subjects)?;
        if let Some(d) = &s.description {
            w.write_event(Event::Start(BytesStart::new("dc:description")))?;
            w.write_event(Event::Start(BytesStart::new("rdf:Alt")))?;
            w.create_element("rdf:li")
                .with_attribute(("xml:lang", "x-default"))
                .write_text_content(BytesText::new(&clean(d)))?;
            w.write_event(Event::End(BytesEnd::new("rdf:Alt")))?;
            w.write_event(Event::End(BytesEnd::new("dc:description")))?;
        }
        text(&mut w, "photosquirrel:contentHash", &s.content_hash)?;
        text(&mut w, "photosquirrel:identityKind", &s.identity_kind)?;
        if let Some(h) = &s.full_hash {
            text(&mut w, "photosquirrel:fullHash", h)?;
        }
        if let Some(n) = s.people {
            text(&mut w, "photosquirrel:peopleCount", &n.to_string())?;
        }
        if let Some(n) = s.pets {
            text(&mut w, "photosquirrel:petCount", &n.to_string())?;
        }
        bag(&mut w, "photosquirrel:files", &s.files)?;
        bag(&mut w, "photosquirrel:sources", &s.sources)?;
        w.write_event(Event::End(BytesEnd::new("rdf:Description")))?;
        w.write_event(Event::End(BytesEnd::new("rdf:RDF")))?;
        w.write_event(Event::End(BytesEnd::new("x:xmpmeta")))?;
        w.write_event(Event::PI(BytesPI::new("xpacket end=\"w\"")))?;
        Ok(())
    })();
    let mut out = w.into_inner();
    out.push(b'\n');
    out
}

/// A sidecar failed validation.
#[derive(Debug, thiserror::Error)]
#[error("invalid XMP: {0}")]
pub struct XmpError(pub String);

fn err(e: impl std::fmt::Display) -> XmpError {
    XmpError(e.to_string())
}

/// Parse and validate an XMP packet: well-formed XML, every prefix bound, an `x:xmpmeta` root holding
/// `rdf:RDF/rdf:Description`. Returns what it says.
pub fn parse(bytes: &[u8]) -> Result<Sidecar, XmpError> {
    let s = std::str::from_utf8(bytes).map_err(err)?;
    let mut r = NsReader::from_str(s);
    // No trimming: text around entity references (`a &amp; b`) must keep its spaces. Leaf text is collected
    // per element (the buffer resets at each start tag), so indentation never leaks into values.
    // Stack of (namespace, local name).
    let mut stack: Vec<(String, String)> = Vec::new();
    let mut out = Sidecar::default();
    let (mut saw_root, mut saw_desc) = (false, false);
    let mut buf = String::new();
    loop {
        let (ns, ev) = r.read_resolved_event().map_err(err)?;
        let ns = match ns {
            ResolveResult::Bound(n) => String::from_utf8_lossy(n.as_ref()).into_owned(),
            ResolveResult::Unbound => String::new(),
            ResolveResult::Unknown(p) => {
                return Err(XmpError(format!(
                    "unbound prefix {}",
                    String::from_utf8_lossy(&p)
                )));
            }
        };
        match ev {
            Event::Start(e) => {
                let local = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                if stack.is_empty() {
                    if ns != NS_X || local != "xmpmeta" {
                        return Err(XmpError(format!("root is {local}, not x:xmpmeta")));
                    }
                    saw_root = true;
                }
                if ns == NS_RDF && local == "Description" {
                    saw_desc =
                        stack.len() == 2 && stack[1] == (NS_RDF.to_owned(), "RDF".to_owned());
                }
                stack.push((ns, local));
                buf.clear();
            }
            Event::Text(t) => buf.push_str(&t.decode().map_err(err)?),
            Event::GeneralRef(g) => {
                if let Some(c) = g.resolve_char_ref().map_err(err)? {
                    buf.push(c);
                } else {
                    let name = g.decode().map_err(err)?;
                    buf.push(match name.as_ref() {
                        "amp" => '&',
                        "lt" => '<',
                        "gt" => '>',
                        "quot" => '"',
                        "apos" => '\'',
                        other => return Err(XmpError(format!("unknown entity {other}"))),
                    });
                }
            }
            Event::End(_) => {
                let Some((ns, local)) = stack.pop() else {
                    return Err(XmpError("unbalanced end tag".into()));
                };
                let parent = |k: usize| stack.len().checked_sub(k).and_then(|i| stack.get(i));
                let value = std::mem::take(&mut buf);
                let in_ps = |n: &str| {
                    parent(1).is_some_and(|p| p.0 == NS_RDF && p.1 == "Bag")
                        && parent(2).is_some_and(|p| p.0 == n)
                };
                if ns == NS_RDF && local == "li" {
                    let grand = parent(2).map(|p| (p.0.as_str(), p.1.as_str()));
                    match grand {
                        Some((NS_DC, "subject")) => out.subjects.push(value),
                        Some((NS_DC, "description")) => out.description = Some(value),
                        Some((NS_PS, "files")) if in_ps(NS_PS) => out.files.push(value),
                        Some((NS_PS, "sources")) if in_ps(NS_PS) => out.sources.push(value),
                        _ => {}
                    }
                } else if ns == NS_PS {
                    match local.as_str() {
                        "contentHash" => out.content_hash = value,
                        "identityKind" => out.identity_kind = value,
                        "fullHash" => out.full_hash = Some(value),
                        "peopleCount" => out.people = value.parse().ok(),
                        "petCount" => out.pets = value.parse().ok(),
                        _ => {}
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !stack.is_empty() {
        return Err(XmpError("unclosed elements".into()));
    }
    if !saw_root || !saw_desc {
        return Err(XmpError("missing x:xmpmeta/rdf:RDF/rdf:Description".into()));
    }
    if out.content_hash.len() != 64 {
        return Err(XmpError("missing photosquirrel:contentHash".into()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_parse_round_trip_with_escapes() {
        let s = Sidecar {
            subjects: vec!["pets".into(), "a & b <c>".into(), "Rex".into()],
            description: Some("A dog \u{1} on a \"beach\"".into()),
            content_hash: "ab".repeat(32),
            identity_kind: "full".into(),
            full_hash: Some("ab".repeat(32)),
            files: vec!["IMG_1.JPG abab".into()],
            sources: vec!["/mnt/a/IMG_1.JPG".into()],
            people: Some(0),
            pets: Some(1),
        };
        let bytes = render(&s);
        let back = parse(&bytes).expect("valid");
        let mut expect = s.clone();
        expect.description = Some("A dog  on a \"beach\"".into());
        assert_eq!(back, expect);
        assert!(parse(b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><a></b></x:xmpmeta>").is_err());
        assert!(parse(b"<y:foo/>").is_err());
    }
}
