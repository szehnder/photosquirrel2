//! The versioned insight prompt (ADR-0020).
//!
//! The prompt text is part of the result's provenance: change it only together with [`PROMPT_VERSION`], and
//! re-record the request fixtures (`PHOTOSQUIRREL_UPDATE_FIXTURES=1 cargo test -p ps-llm`). The schema is
//! [`ps_model::INSIGHT_JSON_SCHEMA`]; the rules below say how to fill it, in the order of the schema.

use ps_model::{INSIGHT_JSON_SCHEMA, INSIGHT_SCHEMA_VERSION};
use serde_json::Value;

/// Version of the prompt wording below. Bump on every wording change.
pub const PROMPT_VERSION: u32 = 1;

/// System message: role and output discipline.
pub const SYSTEM_V1: &str = "You are a careful photo-cataloguing assistant for a private family photo library. \
You look at exactly one image and describe only what is visible in it. You never guess the identity, name, age \
in years, ethnicity or location of a person, and you never invent details you cannot see. You answer with a \
single JSON object that matches the given schema and nothing else: no prose, no explanations, no markdown code \
fences.";

/// The instructions that precede the schema in the user message.
pub const RULES_V1: &str = "\
Describe the attached image as one JSON object with exactly these fields:

- \"subject\": one plain sentence of at most 20 words saying what the image shows, for example \"Two children \
build a sandcastle on a sunny beach.\" No names, no speculation.
- \"category\": the single best fit, one of:
  portrait (one or two people are the clear subject, close up or posed), people (people are the main subject: \
groups, crowds, people at a distance or doing something), pets (dogs, cats and other domestic pets), \
animals (wild, farm or zoo animals, birds, insects), nature (plants, flowers, trees, close-up nature), \
landscape (wide scenery: mountains, sea, sky, fields), travel (landmarks, sightseeing, hotels, airports, \
tourist spots), urban (streets, buildings, architecture, city life), food (meals, dishes, drinks), \
event (party, wedding, birthday, holiday celebration, concert, ceremony), sports (playing or watching sport), \
document (paper, receipt, form, whiteboard, sign or page photographed for its text), screenshot (a capture of \
a computer, phone or TV screen), artwork (painting, drawing, illustration, graphic design, logo, meme, rendered \
image), object (a single item or product close up), other.
  When people are clearly visible and important, prefer portrait or people over the setting.
- \"also\": up to 3 further categories from the same list, most relevant first, not repeating \"category\"; \
[] when none apply.
- \"is_photo\": true when the image is a photograph of a real scene taken with a camera or phone; false for \
screenshots, scanned or photographed documents whose purpose is the text, digital graphics, illustrations, \
renders, memes and icons.
- \"people\": counts of the people visible, including partly visible people and people in the background. \
\"children\" are people who look younger than about 16 (babies, toddlers, kids, young teenagers); \"adults\" are \
everyone else; when unsure whether someone is a child, count them as an adult. \"total\" = adults + children. \
Use 0 for all three when there are no people. People in pictures, posters or screens count only when the image \
is about them.
- \"pets\": the kinds of animals visible, one lowercase singular common name per kind (\"dog\", \"cat\", \
\"horse\", \"parrot\"), each kind once; [] when there are no animals.
- \"tags\": 3 to 10 lowercase search keywords for what is visible: objects, activities, scenery, season, \
weather, time of day, dominant colours. No names, no duplicates, no words already used as the category.
- \"setting\": a short phrase starting with \"indoor\" or \"outdoor\", for example \"outdoor beach\" or \
\"indoor kitchen\"; null when it cannot be told (for example screenshots or graphics).
- \"confidence\": your overall confidence in this description, from 0.0 (guessing) to 1.0 (certain).

The JSON Schema of the answer:";

/// One prompt: system message, user text (rules + schema) and the schema for structured-output modes.
#[derive(Clone, Debug, PartialEq)]
pub struct Prompt {
    pub version: u32,
    pub schema_version: u32,
    pub system: String,
    pub user: String,
    pub schema: Value,
}

impl Prompt {
    /// The ImageInsight v1 prompt.
    pub fn insight_v1() -> Prompt {
        let schema: Value = serde_json::from_str(INSIGHT_JSON_SCHEMA).unwrap_or(Value::Null);
        // Compact schema in the prompt: fewer tokens for small local models.
        let compact = serde_json::to_string(&schema).unwrap_or_default();
        Prompt {
            version: PROMPT_VERSION,
            schema_version: INSIGHT_SCHEMA_VERSION,
            system: SYSTEM_V1.to_owned(),
            user: format!("{RULES_V1}\n{compact}\n\nAnswer with the JSON object only."),
            schema,
        }
    }
}

impl Default for Prompt {
    fn default() -> Self {
        Self::insight_v1()
    }
}

/// The follow-up message of the single repair round: the validation error and the instruction to answer
/// again with JSON only.
pub fn repair_message(error: &str) -> String {
    format!(
        "Your previous answer could not be used: {error}. Reply again with only the corrected JSON object for \
         the same image, matching the schema exactly, with no other text."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_embeds_schema_and_every_category() {
        let p = Prompt::insight_v1();
        assert!(p.schema.is_object());
        assert!(p.user.contains("\"additionalProperties\":false"));
        for c in ps_model::SceneCategory::ALL {
            assert!(p.user.contains(c.as_str()), "{c:?} explained");
        }
        assert!(repair_message("missing field `category`").contains("missing field `category`"));
    }
}
