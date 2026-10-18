//! Parsing and validating a model answer as [`ImageInsight`] v1 (ADR-0020).
//!
//! Lenient about packaging (code fences, `<think>` blocks, prose around the object, a wrapper object, common
//! category synonyms, confidence given in percent), strict about content: required fields, known categories,
//! counts as non-negative integers, confidence in 0-1, a non-empty subject. Everything else is normalized
//! (lower-case de-duplicated tags and pets, `total >= adults + children`, at most 3 secondary categories).
//! The error message is fed back to the model in the repair round, so it names the problem precisely.

use ps_model::{ImageInsight, SceneCategory};
use serde_json::{Map, Value};

/// Maximum tags kept.
pub const MAX_TAGS: usize = 15;
/// Maximum secondary categories kept.
pub const MAX_ALSO: usize = 3;

/// Why an answer is not a valid insight. The message is shown to the model in the repair round.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ValidationError(pub String);

fn err(msg: impl Into<String>) -> ValidationError {
    ValidationError(msg.into())
}

/// Parse a model's text answer into a validated, normalized insight.
pub fn parse_insight(text: &str) -> Result<ImageInsight, ValidationError> {
    let json = extract_json_object(text).ok_or_else(|| {
        err("the answer contains no JSON object (it must be a single JSON object starting with `{`)")
    })?;
    let value: Value = serde_json::from_str(json)
        .map_err(|e| err(format!("the answer is not valid JSON: {e}")))?;
    validate_value(value)
}

/// Validate an already-parsed JSON value.
pub fn validate_value(value: Value) -> Result<ImageInsight, ValidationError> {
    let Value::Object(mut obj) = value else {
        return Err(err("the answer must be a JSON object"));
    };
    obj = unwrap_single_wrapper(obj);
    normalize_fields(&mut obj)?;
    let insight: ImageInsight = serde_json::from_value(Value::Object(obj))
        .map_err(|e| err(format!("the JSON does not match the schema: {e}")))?;
    finish(insight)
}

/// `{"insight": {...}}` or `{"image": {...}}` -> the inner object.
fn unwrap_single_wrapper(obj: Map<String, Value>) -> Map<String, Value> {
    if !obj.contains_key("category") && obj.len() == 1 {
        let inner = obj.values().next().cloned();
        if let Some(Value::Object(inner)) = inner
            && inner.contains_key("category")
        {
            return inner;
        }
    }
    obj
}

fn category_of(s: &str) -> Option<SceneCategory> {
    let s = s.trim().to_ascii_lowercase().replace([' ', '-'], "_");
    let alias = match s.as_str() {
        "pet" | "dog" | "cat" | "dogs" | "cats" => "pets",
        "animal" | "wildlife" | "bird" | "birds" => "animals",
        "person" | "group" | "family" | "crowd" => "people",
        "selfie" | "headshot" => "portrait",
        "architecture" | "city" | "building" | "buildings" | "street" | "cityscape" => "urban",
        "art" | "illustration" | "graphic" | "graphics" | "drawing" | "painting" | "meme" => {
            "artwork"
        }
        "documents" | "text" | "receipt" => "document",
        "screen" | "screen_capture" | "screenshots" => "screenshot",
        "celebration" | "party" | "wedding" | "birthday" => "event",
        "sport" => "sports",
        "scenery" | "seascape" => "landscape",
        "plant" | "plants" | "flower" | "flowers" => "nature",
        "objects" | "product" | "still_life" => "object",
        "meal" | "drink" => "food",
        other => other,
    };
    SceneCategory::ALL.into_iter().find(|c| c.as_str() == alias)
}

fn normalize_fields(obj: &mut Map<String, Value>) -> Result<(), ValidationError> {
    // category: required, known (after synonyms).
    match obj.get("category") {
        None | Some(Value::Null) => return Err(err("missing required field `category`")),
        Some(Value::String(s)) => {
            let c = category_of(s).ok_or_else(|| {
                err(format!(
                    "`category` is \"{s}\", which is not one of: {}",
                    SceneCategory::ALL.map(|c| c.as_str()).join(", ")
                ))
            })?;
            obj.insert("category".into(), Value::String(c.as_str().into()));
        }
        Some(other) => return Err(err(format!("`category` must be a string, got {other}"))),
    }
    // also: unknown secondary categories are dropped rather than repaired.
    let also: Vec<Value> = match obj.get("also") {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .filter_map(category_of)
            .map(|c| Value::String(c.as_str().into()))
            .collect(),
        Some(Value::String(s)) => category_of(s)
            .map(|c| vec![Value::String(c.as_str().into())])
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    obj.insert("also".into(), Value::Array(also));
    // pets: accept [{"kind": "dog", "count": 2}] and "dog" as well as ["dog"].
    let pets: Vec<Value> = match obj.get("pets") {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|p| match p {
                Value::String(s) => Some(s.clone()),
                Value::Object(o) => o
                    .get("kind")
                    .or_else(|| o.get("type"))
                    .or_else(|| o.get("name"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                _ => None,
            })
            .map(Value::String)
            .collect(),
        Some(Value::String(s)) if !s.trim().is_empty() => vec![Value::String(s.clone())],
        _ => Vec::new(),
    };
    obj.insert("pets".into(), Value::Array(pets));
    // tags: accept a comma-separated string.
    if let Some(Value::String(s)) = obj.get("tags") {
        let tags = s.split(',').map(|t| Value::String(t.to_owned())).collect();
        obj.insert("tags".into(), Value::Array(tags));
    }
    if obj.get("tags").is_none_or(Value::is_null) {
        obj.insert("tags".into(), Value::Array(Vec::new()));
    }
    // setting: empty string -> null.
    if let Some(Value::String(s)) = obj.get("setting")
        && s.trim().is_empty()
    {
        obj.insert("setting".into(), Value::Null);
    }
    // confidence: percent -> fraction; numeric strings accepted.
    let conf = match obj.get("confidence") {
        None | Some(Value::Null) => return Err(err("missing required field `confidence`")),
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.trim().trim_end_matches('%').trim().parse::<f64>().ok(),
        Some(_) => None,
    }
    .ok_or_else(|| err("`confidence` must be a number between 0.0 and 1.0"))?;
    let conf = if conf > 1.0 && conf <= 100.0 {
        conf / 100.0
    } else {
        conf
    };
    if !(0.0..=1.0).contains(&conf) || !conf.is_finite() {
        return Err(err(format!(
            "`confidence` is {conf}, but it must be between 0.0 and 1.0"
        )));
    }
    obj.insert("confidence".into(), Value::from(conf));
    // people: counts must be non-negative integers; a missing object is an error (the model must count).
    match obj.get_mut("people") {
        Some(Value::Object(p)) => {
            for key in ["total", "adults", "children"] {
                match p.get(key) {
                    None | Some(Value::Null) => {
                        p.insert(key.into(), Value::from(0));
                    }
                    Some(Value::Number(n)) if n.as_u64().is_some() => {}
                    Some(Value::Number(n))
                        if n.as_f64().is_some_and(|f| f >= 0.0 && f.fract() == 0.0) =>
                    {
                        let v = n.as_f64().unwrap_or(0.0) as u64;
                        p.insert(key.into(), Value::from(v));
                    }
                    Some(other) => {
                        return Err(err(format!(
                            "`people.{key}` must be a whole number >= 0, got {other}"
                        )));
                    }
                }
            }
        }
        _ => {
            return Err(err(
                "missing required object `people` with `total`, `adults` and `children`",
            ));
        }
    }
    Ok(())
}

fn clean_list(items: &mut Vec<String>, max: usize) {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(items.len().min(max));
    for t in items.drain(..) {
        let t = t
            .trim()
            .trim_matches(|c| c == '#' || c == '"')
            .trim()
            .to_lowercase();
        if t.is_empty() || t == "none" || t == "n/a" {
            continue;
        }
        if seen.insert(t.clone()) {
            out.push(t);
        }
        if out.len() == max {
            break;
        }
    }
    *items = out;
}

fn finish(mut i: ImageInsight) -> Result<ImageInsight, ValidationError> {
    let subject = i
        .subject
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_owned();
    if subject.is_empty() {
        return Err(err(
            "`subject` is empty; it must be one sentence describing the image",
        ));
    }
    i.subject = subject;
    let primary = i.category;
    let mut seen = std::collections::HashSet::new();
    i.also.retain(|c| *c != primary && seen.insert(*c));
    i.also.truncate(MAX_ALSO);
    clean_list(&mut i.tags, MAX_TAGS);
    clean_list(&mut i.pets, MAX_TAGS);
    let p = &mut i.people;
    if p.adults + p.children == 0 && p.total > 0 {
        // The prompt says "when unsure, count as an adult".
        p.adults = p.total;
    }
    p.total = p.total.max(p.adults + p.children);
    i.setting = i
        .setting
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty() && s != "null" && s != "unknown");
    Ok(i)
}

/// The text of the first balanced JSON object in `text`, skipping `<think>...</think>` reasoning and markdown
/// fences.
pub fn extract_json_object(text: &str) -> Option<&str> {
    let mut t = text;
    if let Some(end) = t.rfind("</think>") {
        t = &t[end + "</think>".len()..];
    }
    let bytes = t.as_bytes();
    let mut start = None;
    let (mut depth, mut in_str, mut escaped) = (0usize, false, false);
    for (i, &b) in bytes.iter().enumerate() {
        match start {
            None => {
                if b == b'{' {
                    start = Some(i);
                    depth = 1;
                }
            }
            Some(s) => {
                if in_str {
                    match b {
                        _ if escaped => escaped = false,
                        b'\\' => escaped = true,
                        b'"' => in_str = false,
                        _ => {}
                    }
                    continue;
                }
                match b {
                    b'"' => in_str = true,
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(&t[s..=i]);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    const GOOD: &str = r#"{"subject":"A girl hugs a dog in a garden.","category":"pets","also":["people","pets"],
      "is_photo":true,"people":{"total":1,"adults":0,"children":1},"pets":["Dog"],"tags":["garden","Dog","dog"],
      "setting":"outdoor garden","confidence":0.9}"#;

    #[test]
    fn parses_and_normalizes() {
        let i = parse_insight(&format!(
            "<think>hmm {{</think>Here you go:\n```json\n{GOOD}\n```"
        ))
        .unwrap();
        assert_eq!(i.category, SceneCategory::Pets);
        assert_eq!(i.also, vec![SceneCategory::People]);
        assert_eq!(i.pets, vec!["dog"]);
        assert_eq!(i.tags, vec!["garden", "dog"]);
        assert_eq!(
            (i.people.total, i.people.adults, i.people.children),
            (1, 0, 1)
        );
    }

    #[test]
    fn lenient_packaging_strict_content() {
        let v = r#"{"insight":{"subject":"x","category":"Selfie","is_photo":true,
            "people":{"total":2},"pets":[{"kind":"cat","count":1}],"confidence":"85%","setting":""}}"#;
        let i = parse_insight(v).unwrap();
        assert_eq!(i.category, SceneCategory::Portrait);
        assert_eq!(i.people.adults, 2);
        assert!((i.confidence - 0.85).abs() < 1e-6);
        assert_eq!(i.pets, vec!["cat"]);
        assert_eq!(i.setting, None);

        assert!(
            parse_insight("I cannot help")
                .unwrap_err()
                .0
                .contains("no JSON object")
        );
        let e = parse_insight(r#"{"subject":"x","is_photo":true,"people":{},"confidence":0.5}"#)
            .unwrap_err();
        assert!(e.0.contains("category"), "{e}");
        let e = parse_insight(r#"{"subject":"x","category":"spaceship","is_photo":true,"people":{},"confidence":0.5}"#)
            .unwrap_err();
        assert!(e.0.contains("spaceship"));
        let e = parse_insight(r#"{"subject":"x","category":"food","is_photo":true,"people":{"total":-1},"confidence":0.5}"#)
            .unwrap_err();
        assert!(e.0.contains("people.total"));
        let e = parse_insight(
            r#"{"subject":" ","category":"food","is_photo":true,"people":{},"confidence":0.5}"#,
        )
        .unwrap_err();
        assert!(e.0.contains("subject"));
        let e = parse_insight(
            r#"{"subject":"x","category":"food","is_photo":true,"people":{},"confidence":7000}"#,
        )
        .unwrap_err();
        assert!(e.0.contains("confidence"));
    }
}
