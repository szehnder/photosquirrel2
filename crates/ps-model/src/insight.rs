use serde::{Deserialize, Serialize};

/// Schema version of [`ImageInsight`]; bump on incompatible changes (ADR-0020).
pub const INSIGHT_SCHEMA_VERSION: u32 = 1;

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
