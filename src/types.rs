use std::collections::BTreeMap;

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Boundary {
    Word,
    Sentence,
}

impl Boundary {
    pub(crate) fn metadata_flags(self) -> (&'static str, &'static str) {
        match self {
            Boundary::Word => ("true", "false"),
            Boundary::Sentence => ("false", "true"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceTag {
    #[serde(rename = "ContentCategories", default)]
    pub content_categories: Vec<String>,
    #[serde(rename = "VoicePersonalities", default)]
    pub voice_personalities: Vec<String>,
}

impl Default for VoiceTag {
    fn default() -> Self {
        Self {
            content_categories: Vec::new(),
            voice_personalities: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Voice {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "ShortName")]
    pub short_name: String,
    #[serde(rename = "Gender")]
    pub gender: String,
    #[serde(rename = "Locale")]
    pub locale: String,
    #[serde(rename = "SuggestedCodec", default)]
    pub suggested_codec: Option<String>,
    #[serde(rename = "FriendlyName", default)]
    pub friendly_name: Option<String>,
    #[serde(rename = "Status", default)]
    pub status: Option<String>,
    #[serde(rename = "VoiceTag", default)]
    pub voice_tag: VoiceTag,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundaryEvent {
    pub kind: Boundary,
    pub offset_ticks: u64,
    pub duration_ticks: u64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SynthesisEvent {
    Audio(Bytes),
    Boundary(BoundaryEvent),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynthesisResult {
    pub audio: Vec<u8>,
    pub boundaries: Vec<BoundaryEvent>,
}
