//! Literal outbound text, native header entities and final Unicode limits.

use std::fmt;

use serde::Serialize;
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    config::OUTBOUND_SCALAR_LIMIT,
    error::{AppError, TextTooLong},
};

pub(crate) const TITLE_GRAPHEME_LIMIT: usize = 50;
pub(crate) const DIRECTORY_SCALAR_LIMIT: usize = 100;
pub(crate) const FALLBACK_TITLE: &str = "Agent Session";
pub(crate) const SERVICE_PREFIX: &str = "🔧 ";
// Conservatively count native Telegram limits in UTF-16 units.
pub(crate) const TEXT_UTF16_LIMIT: usize = 4_096;
pub(crate) const CAPTION_UTF16_LIMIT: usize = 1_024;

pub(crate) const UNCONFIRMED_NOTICE: &str =
    "Reply saved, but the agent has not confirmed processing it yet.";
pub(crate) const UNROUTABLE_NOTICE: &str = "Could not identify the target session for this Reply. Please reply to a newer message from that agent.";

/// Reserved wording is only a fallback for bot-authored targets without an ordinary route.
/// Retain recognition of already-sent plain/bracketed notices after presentation changes.
pub(crate) fn is_service_notice(text: &str) -> bool {
    if text == UNROUTABLE_NOTICE || text == UNCONFIRMED_NOTICE {
        return true;
    }
    if let Some(text) = text.strip_prefix(SERVICE_PREFIX) {
        if text == UNROUTABLE_NOTICE || text == UNCONFIRMED_NOTICE {
            return true;
        }
        if let Some((header, UNCONFIRMED_NOTICE)) = text.rsplit_once('\n') {
            let mut lines = header.split('\n');
            let title = lines.next().unwrap_or_default();
            return SessionTitle::new(title).as_str() == title
                && lines.next().is_none_or(|directory| {
                    DirectoryLabel::new(directory)
                        .ok()
                        .flatten()
                        .is_some_and(|label| label.as_str() == directory)
                })
                && lines.next().is_none();
        }
    }
    let Some((header, body)) = text.split_once('\n') else {
        return false;
    };
    let title = header.strip_prefix('[').and_then(|s| s.strip_suffix(']'));
    body == UNCONFIRMED_NOTICE
        && title.is_some_and(|title| SessionTitle::normalize(title, false).as_str() == title)
}

/// A single-line display label, never a session identity or routing key.
#[derive(Clone, PartialEq, Eq)]
pub struct SessionTitle(String);

impl SessionTitle {
    /// Collapse whitespace, strip unsafe controls and keep at most 50 graphemes.
    /// A single grapheme can contain many scalars; final output still needs validation.
    pub fn new(input: &str) -> Self {
        Self::normalize(input, true)
    }

    fn normalize(input: &str, safe: bool) -> Self {
        let mut normalized = normalize_line(input, safe);
        let end = normalized
            .grapheme_indices(true)
            .nth(TITLE_GRAPHEME_LIMIT)
            .map_or(normalized.len(), |(offset, _)| offset);
        normalized.truncate(end);
        normalized.truncate(normalized.trim_end().len());
        if normalized.is_empty() {
            normalized.push_str(FALLBACK_TITLE);
        }
        Self(normalized)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Opt-in display context only; never a filesystem authority, instruction or route.
#[derive(Clone, PartialEq, Eq)]
pub struct DirectoryLabel(String);

impl DirectoryLabel {
    /// Blank labels are omitted; oversized normalized labels are rejected, not truncated.
    pub fn new(input: &str) -> Result<Option<Self>, AppError> {
        let normalized = normalize_line(input, true);
        if normalized.chars().count() > DIRECTORY_SCALAR_LIMIT {
            return Err(AppError::InvalidRequest("directory_label"));
        }
        Ok((!normalized.is_empty()).then_some(Self(normalized)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn normalize_line(input: &str, safe: bool) -> String {
    let mut normalized = String::new();
    let mut space = false;
    for c in input.chars() {
        if c.is_whitespace() {
            space = !normalized.is_empty();
        } else if c.is_control() || (safe && unsafe_format_control(c)) {
            continue;
        } else {
            if space {
                normalized.push(' ');
            }
            normalized.push(c);
            space = false;
        }
    }
    normalized
}

fn unsafe_format_control(c: char) -> bool {
    // Remove directional overrides/isolates and invisible formatting controls; retain
    // ZWJ/ZWNJ and variation selectors so ordinary scripts and emoji stay intact.
    matches!(c, '\u{ad}' | '\u{61c}' | '\u{180e}' | '\u{200b}' | '\u{200e}'..='\u{200f}'
        | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}' | '\u{fff9}'..='\u{fffb}')
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Bold,
    Italic,
}

/// Telegram entity coordinates count UTF-16 code units, never bytes or scalars.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MessageEntity {
    #[serde(rename = "type")]
    pub kind: EntityKind,
    pub offset: usize,
    pub length: usize,
}

/// Constructed only by validated formatters; entities cover server-owned header spans.
#[derive(Clone, PartialEq, Eq)]
pub struct FormattedMessage {
    text: String,
    entities: Vec<MessageEntity>,
}

impl FormattedMessage {
    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn entities(&self) -> &[MessageEntity] {
        &self.entities
    }
}

macro_rules! redacted_debug {
    ($($name:ident),+) => {$(
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(concat!(stringify!($name), "([REDACTED])"))
            }
        }
    )+};
}
redacted_debug!(SessionTitle, DirectoryLabel, FormattedMessage);

/// Format text/questions with native header entities and a verbatim body.
pub fn format_text(
    title: &SessionTitle,
    directory: Option<&DirectoryLabel>,
    body: &str,
) -> Result<FormattedMessage, AppError> {
    format_outbound("", Some(title), directory, body, TEXT_UTF16_LIMIT)
}

/// An empty document description leaves only the title and optional directory.
pub fn format_caption(
    title: &SessionTitle,
    directory: Option<&DirectoryLabel>,
    body: &str,
) -> Result<FormattedMessage, AppError> {
    format_outbound("", Some(title), directory, body, CAPTION_UTF16_LIMIT)
}

/// Every service notice begins with the exact prefix, before any known-session header.
pub fn format_notice(
    title: Option<&SessionTitle>,
    directory: Option<&DirectoryLabel>,
    body: &str,
) -> Result<FormattedMessage, AppError> {
    format_outbound(SERVICE_PREFIX, title, directory, body, TEXT_UTF16_LIMIT)
}

fn format_outbound(
    prefix: &str,
    title: Option<&SessionTitle>,
    directory: Option<&DirectoryLabel>,
    body: &str,
    native_limit_utf16: usize,
) -> Result<FormattedMessage, AppError> {
    // Unknown-session notices have no directory context either.
    let directory = directory.filter(|_| title.is_some());
    let separator = title.is_some() && !body.is_empty();
    let framing = usize::from(separator) + usize::from(directory.is_some());
    let (actual_characters, actual_utf16_units) = prefix
        .chars()
        .chain(title.into_iter().flat_map(|title| title.as_str().chars()))
        .chain(
            directory
                .into_iter()
                .flat_map(|label| label.as_str().chars()),
        )
        .chain(body.chars())
        .fold((framing, framing), |(scalars, units), c| {
            (scalars + 1, units + c.len_utf16())
        });
    if actual_characters > OUTBOUND_SCALAR_LIMIT || actual_utf16_units > native_limit_utf16 {
        return Err(AppError::TextTooLong(TextTooLong {
            limit: OUTBOUND_SCALAR_LIMIT,
            actual_characters,
            native_limit_utf16,
            actual_utf16_units,
            guidance: "Put the full material in a .md, .txt or other suitable file and use telegram_send_file with a short caption.",
        }));
    }

    let mut text = prefix.to_owned();
    let mut entities = Vec::with_capacity(2);
    if let Some(title) = title {
        entities.push(MessageEntity {
            kind: EntityKind::Bold,
            offset: prefix.encode_utf16().count(),
            length: title.as_str().encode_utf16().count(),
        });
        text.push_str(title.as_str());
    }
    if let Some(directory) = directory {
        text.push('\n');
        entities.push(MessageEntity {
            kind: EntityKind::Italic,
            offset: text.encode_utf16().count(),
            length: directory.as_str().encode_utf16().count(),
        });
        text.push_str(directory.as_str());
    }
    if separator {
        text.push('\n');
    }
    text.push_str(body);
    Ok(FormattedMessage { text, entities })
}

#[cfg(test)]
mod tests;
