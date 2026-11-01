//! Explicit body-relative Telegram entities; no markup interpretation or effects.

use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::AppError;

pub(crate) const BODY_ENTITY_LIMIT: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(crate = "rmcp::schemars")]
pub enum EntityKind {
    Bold,
    Italic,
    Underline,
    Strikethrough,
    Spoiler,
    Code,
    Pre,
    TextLink,
    Blockquote,
    ExpandableBlockquote,
}

impl EntityKind {
    fn style(self) -> bool {
        matches!(
            self,
            Self::Bold | Self::Italic | Self::Underline | Self::Strikethrough | Self::Spoiler
        )
    }

    fn verbatim(self) -> bool {
        matches!(self, Self::Code | Self::Pre)
    }
}

/// Native entity coordinates count UTF-16 units. Tool arguments use body/caption-relative
/// offsets excluding headers; formatted messages use absolute offsets including framing.
/// Overlaps require containment and a bold/italic/underline/strikethrough/spoiler span.
/// Code/pre cannot overlap; quotes and other non-style entities cannot contain each other.
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct MessageEntity {
    #[serde(rename = "type")]
    pub kind: EntityKind,
    pub offset: usize,
    /// Positive length; request spans must start/end on scalar boundaries within the body.
    pub length: usize,
    /// Required only for text_link: an absolute http/https/tg/mailto/tel URL, at most 2,048 bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Optional only for pre: a nonempty language name, at most 64 bytes, without whitespace.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

/// Validate the official nesting rules before admission, waiting interests or I/O.
pub(crate) fn validate_body_entities(
    body: &str,
    entities: &[MessageEntity],
) -> Result<(), AppError> {
    let invalid = || AppError::InvalidRequest("body entities");
    if entities.len() > BODY_ENTITY_LIMIT {
        return Err(invalid());
    }
    if entities.is_empty() {
        return Ok(());
    }
    let mut boundaries = vec![0];
    let mut units = 0;
    for c in body.chars() {
        units += c.len_utf16(); // UTF-16 length cannot exceed the allocated UTF-8 byte length.
        boundaries.push(units);
    }
    let mut spans: Vec<(usize, usize, EntityKind)> = Vec::with_capacity(entities.len());
    for entity in entities {
        let end = entity
            .offset
            .checked_add(entity.length)
            .ok_or_else(invalid)?;
        if entity.length == 0
            || boundaries.binary_search(&entity.offset).is_err()
            || boundaries.binary_search(&end).is_err()
            || (entity.kind == EntityKind::TextLink) != entity.url.is_some()
            || (entity.kind != EntityKind::Pre && entity.language.is_some())
        {
            return Err(invalid());
        }
        if let Some(url) = &entity.url {
            if url.len() > 2_048 || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
                return Err(invalid());
            }
            let parsed = reqwest::Url::parse(url).map_err(|_| invalid())?;
            if !matches!(parsed.scheme(), "http" | "https" | "tg" | "mailto" | "tel")
                || (parsed.host_str().is_none() && parsed.path().is_empty())
            {
                return Err(invalid());
            }
        }
        if entity.language.as_ref().is_some_and(|language| {
            language.is_empty()
                || language.len() > 64
                || language
                    .chars()
                    .any(|c| c.is_whitespace() || c.is_control())
        }) {
            return Err(invalid());
        }
        for &(start, previous_end, kind) in &spans {
            if entity.offset >= previous_end || start >= end {
                continue;
            }
            let contained = (start <= entity.offset && end <= previous_end)
                || (entity.offset <= start && previous_end <= end);
            if !contained
                || entity.kind.verbatim()
                || kind.verbatim()
                || !(entity.kind.style() || kind.style())
            {
                return Err(invalid());
            }
        }
        spans.push((entity.offset, end, entity.kind));
    }
    Ok(())
}
