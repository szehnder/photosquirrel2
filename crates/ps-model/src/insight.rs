use serde::{Deserialize, Serialize};

/// Schema version of [`ImageInsight`]; bump on incompatible changes (ADR-0020).
pub const INSIGHT_SCHEMA_VERSION: u32 = 1;

/// JSON Schema (draft 2020-12) of [`ImageInsight`] v1, as sent to vision models (embedded in prompts and passed
/// as structured-output schema where the provider supports it, ADR-0020). Every property is required and no
/// others are allowed, so it is valid for strict structured-output modes; ranges (confidence 0-1, counts) are
/// checked by `ps-llm` validation rather than in the schema, because not every server supports those keywords.
/// Kept in lock-step with the serde shape of [`ImageInsight`] by a unit test.
pub const INSIGHT_JSON_SCHEMA: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "ImageInsight",
  "type": "object",
  "additionalProperties": false,
  "required": ["subject", "category", "also", "is_photo", "people", "pets", "tags", "setting", "confidence"],
  "properties": {
    "subject": { "type": "string" },
    "category": {
      "type": "string",
      "enum": ["portrait", "people", "pets", "animals", "nature", "landscape", "travel", "urban", "food", "event",
               "sports", "document", "screenshot", "artwork", "object", "other"]
    },
    "also": {
      "type": "array",
      "items": {
        "type": "string",
        "enum": ["portrait", "people", "pets", "animals", "nature", "landscape", "travel", "urban", "food",
                 "event", "sports", "document", "screenshot", "artwork", "object", "other"]
      }
    },
    "is_photo": { "type": "boolean" },
    "people": {
      "type": "object",
      "additionalProperties": false,
      "required": ["total", "adults", "children"],
      "properties": {
        "total": { "type": "integer" },
        "adults": { "type": "integer" },
        "children": { "type": "integer" }
      }
    },
    "pets": { "type": "array", "items": { "type": "string" } },
    "tags": { "type": "array", "items": { "type": "string" } },
    "setting": { "type": ["string", "null"] },
    "confidence": { "type": "number" }
  }
}"#;

/// Broad scene category asked of the LLM.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneCategory {
    Portrait,
    People,
    Pets,
    Animals,
    Nature,
    Landscape,
    Travel,
    Urban,
    Food,
    Event,
    Sports,
    Document,
    Screenshot,
    Artwork,
    Object,
    Other,
}

impl SceneCategory {
    pub const ALL: [SceneCategory; 16] = [
        SceneCategory::Portrait,
        SceneCategory::People,
        SceneCategory::Pets,
        SceneCategory::Animals,
        SceneCategory::Nature,
        SceneCategory::Landscape,
        SceneCategory::Travel,
        SceneCategory::Urban,
        SceneCategory::Food,
        SceneCategory::Event,
        SceneCategory::Sports,
        SceneCategory::Document,
        SceneCategory::Screenshot,
        SceneCategory::Artwork,
        SceneCategory::Object,
        SceneCategory::Other,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            SceneCategory::Portrait => "portrait",
            SceneCategory::People => "people",
            SceneCategory::Pets => "pets",
            SceneCategory::Animals => "animals",
            SceneCategory::Nature => "nature",
            SceneCategory::Landscape => "landscape",
            SceneCategory::Travel => "travel",
            SceneCategory::Urban => "urban",
            SceneCategory::Food => "food",
            SceneCategory::Event => "event",
            SceneCategory::Sports => "sports",
            SceneCategory::Document => "document",
            SceneCategory::Screenshot => "screenshot",
            SceneCategory::Artwork => "artwork",
            SceneCategory::Object => "object",
            SceneCategory::Other => "other",
        }
    }
}

/// People in the image, as estimated by the model.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeopleEstimate {
    pub total: u32,
    pub adults: u32,
    pub children: u32,
}

/// What the vision model says about one image (schema v1). Advisory only (ADR-0021).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImageInsight {
    /// One sentence: the general subject.
    pub subject: String,
    pub category: SceneCategory,
    /// Secondary categories, most relevant first.
    #[serde(default)]
    pub also: Vec<SceneCategory>,
    pub is_photo: bool,
    pub people: PeopleEstimate,
    /// Pet / animal kinds seen ("dog", "cat", ...).
    #[serde(default)]
    pub pets: Vec<String>,
    /// Free-form keywords for search and XMP sidecars.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Indoor/outdoor and setting, e.g. "outdoor beach".
    #[serde(default)]
    pub setting: Option<String>,
    /// Model's confidence 0.0-1.0.
    pub confidence: f32,
}

impl ImageInsight {
    pub fn has_people(&self) -> bool {
        self.people.total > 0
    }

    pub fn has_children(&self) -> bool {
        self.people.children > 0
    }

    pub fn has_pets(&self) -> bool {
        !self.pets.is_empty() || self.category == SceneCategory::Pets
    }

    /// Keywords for sidecars: category, tags, pets, and people hints.
    pub fn keywords(&self) -> Vec<String> {
        let mut out = vec![self.category.as_str().to_owned()];
        out.extend(self.also.iter().map(|c| c.as_str().to_owned()));
        out.extend(self.tags.iter().cloned());
        out.extend(self.pets.iter().cloned());
        if self.has_children() {
            out.push("children".into());
        }
        if self.has_people() {
            out.push("people".into());
        }
        let mut seen = std::collections::HashSet::new();
        out.retain(|k| seen.insert(k.to_lowercase()));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"subject":"A dog on a sofa","category":"pets","also":["people"],"is_photo":true,
        "people":{"total":1,"adults":1,"children":0},"pets":["dog"],"tags":["sofa"],"setting":"indoor living room",
        "confidence":0.8}"#;

    /// Minimal draft-2020-12 check for the keywords the schema uses (type, enum, required, properties,
    /// additionalProperties, items).
    fn conforms(schema: &serde_json::Value, v: &serde_json::Value) -> Result<(), String> {
        use serde_json::Value;
        let type_ok = |t: &str| match t {
            "object" => v.is_object(),
            "array" => v.is_array(),
            "string" => v.is_string(),
            "boolean" => v.is_boolean(),
            "integer" => v.is_u64() || v.is_i64(),
            "number" => v.is_number(),
            "null" => v.is_null(),
            _ => false,
        };
        match &schema["type"] {
            Value::String(t) if !type_ok(t) => return Err(format!("expected {t}, got {v}")),
            Value::Array(ts) if !ts.iter().filter_map(Value::as_str).any(type_ok) => {
                return Err(format!("expected one of {ts:?}, got {v}"));
            }
            _ => {}
        }
        if let Some(e) = schema["enum"].as_array()
            && !e.contains(v)
        {
            return Err(format!("{v} not in enum"));
        }
        if let Some(obj) = v.as_object() {
            for r in schema["required"].as_array().into_iter().flatten() {
                let r = r.as_str().unwrap_or_default();
                if !obj.contains_key(r) {
                    return Err(format!("missing required property `{r}`"));
                }
            }
            let props = schema["properties"].as_object();
            for (k, child) in obj {
                match props.and_then(|p| p.get(k)) {
                    Some(s) => conforms(s, child)?,
                    None if schema["additionalProperties"] == Value::Bool(false) => {
                        return Err(format!("unexpected property `{k}`"));
                    }
                    None => {}
                }
            }
        }
        if let (Some(items), Some(arr)) = (schema.get("items"), v.as_array()) {
            for child in arr {
                conforms(items, child)?;
            }
        }
        Ok(())
    }

    #[test]
    fn json_schema_validates_sample_and_rejects_missing_category() {
        let schema: serde_json::Value =
            serde_json::from_str(INSIGHT_JSON_SCHEMA).expect("schema is JSON");
        let sample: serde_json::Value = serde_json::from_str(SAMPLE).expect("sample");
        conforms(&schema, &sample).expect("sample conforms");
        let typed: ImageInsight =
            serde_json::from_value(sample.clone()).expect("sample deserializes");
        // The schema and the serde shape agree: re-serializing a typed value conforms too.
        conforms(&schema, &serde_json::to_value(&typed).expect("ser"))
            .expect("round trip conforms");

        let mut missing = sample.clone();
        missing.as_object_mut().expect("object").remove("category");
        let err = conforms(&schema, &missing).expect_err("missing category rejected");
        assert!(err.contains("category"), "{err}");
        assert!(serde_json::from_value::<ImageInsight>(missing).is_err());

        let mut bad = sample;
        bad["category"] = "selfie".into();
        assert!(conforms(&schema, &bad).is_err());

        // The enum lists exactly the categories, in order.
        let listed: Vec<&str> = schema["properties"]["category"]["enum"]
            .as_array()
            .expect("enum")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        let all: Vec<&str> = SceneCategory::ALL.iter().map(|c| c.as_str()).collect();
        assert_eq!(listed, all);
    }

    #[test]
    fn insight_json_round_trip_and_keywords() {
        let json = r#"{"subject":"A boy and a dog on a beach","category":"people","also":["pets","travel"],
            "is_photo":true,"people":{"total":1,"adults":0,"children":1},"pets":["dog"],
            "tags":["beach","sunset","Dog"],"setting":"outdoor beach","confidence":0.9}"#;
        let i: ImageInsight = serde_json::from_str(json).expect("parse");
        assert!(i.has_children() && i.has_pets());
        let k = i.keywords();
        assert_eq!(k[0], "people");
        assert!(k.contains(&"children".to_owned()));
        // "Dog" and "dog" deduplicate case-insensitively.
        assert_eq!(
            k.iter().filter(|s| s.eq_ignore_ascii_case("dog")).count(),
            1
        );
    }
}
