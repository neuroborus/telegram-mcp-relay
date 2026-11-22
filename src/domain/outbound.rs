//! Outbound body formatting, isolated native header/body entities and final Unicode limits.

use std::{borrow::Cow, fmt};

use unicode_segmentation::UnicodeSegmentation;

mod entities;
mod markup;
pub(crate) use entities::validate_body_entities;
pub use entities::{EntityKind, MessageEntity};
pub use markup::OutboundFormat;
pub(crate) use markup::prepare_body;

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

pub const START_ACK_TEXT: &str = "✅ Request received. Working on it.";
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
            // New framing adds a blank line; retain the old single-newline shapes.
            let header = header.strip_suffix('\n').unwrap_or(header);
            let mut lines = header.split('\n');
            let title = lines.next().unwrap_or_default();
            // Brackets may belong to an old literal title rather than the new framing.
            let valid_title =
                |label: &str| SessionTitle::normalize_stored(label, true).as_str() == label;
            return (valid_title(title)
                || title
                    .strip_prefix('[')
                    .and_then(|s| s.strip_suffix(']'))
                    .is_some_and(valid_title))
                && lines.next().is_none_or(|directory| {
                    DirectoryLabel::from_stored(directory)
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
        && title.is_some_and(|title| SessionTitle::normalize_stored(title, false).as_str() == title)
}

/// A single-line display label, never a session identity or routing key.
#[derive(Clone, PartialEq, Eq)]
pub struct SessionTitle(String);

impl SessionTitle {
    /// Collapse whitespace, strip unsafe controls and keep at most 50 graphemes.
    /// Overlong labels reserve the last grapheme for a visible ellipsis.
    /// A single grapheme can contain many scalars; final output still needs validation.
    pub fn parse(input: &str) -> Result<Self, AppError> {
        let normalized = normalize_line(input.chars(), true);
        if !is_meaningful_title(&normalized) {
            return Err(AppError::InvalidTitle);
        }
        let title = Self::truncate(normalized);
        title
            .is_meaningful()
            .then_some(title)
            .ok_or(AppError::InvalidTitle)
    }

    pub(crate) fn from_stored(input: &str) -> Self {
        let normalized = normalize_line(input.chars(), true);
        if normalized.is_empty() || starts_with_path(&comparable_label(&normalized)) {
            return Self(FALLBACK_TITLE.to_owned());
        }
        Self::truncate(normalized)
    }

    // Match notices already sent with legacy titles, including ones now rejected for display.
    fn normalize_stored(input: &str, safe: bool) -> Self {
        let normalized = normalize_line(input.chars(), safe);
        if normalized.is_empty() {
            return Self(FALLBACK_TITLE.to_owned());
        }
        Self::truncate(normalized)
    }

    fn truncate(mut normalized: String) -> Self {
        let mut graphemes = normalized.grapheme_indices(true);
        let cutoff = graphemes
            .nth(TITLE_GRAPHEME_LIMIT - 1)
            .map(|(offset, _)| offset);
        if graphemes.next().is_some() {
            // An overlong label necessarily has the reserved fiftieth grapheme.
            normalized.truncate(cutoff.unwrap_or(normalized.len()));
            normalized.truncate(normalized.trim_end().len());
            normalized.push('…');
        }
        Self(normalized)
    }

    pub(crate) fn is_meaningful(&self) -> bool {
        is_meaningful_title(self.as_str())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_meaningful_title(title: &str) -> bool {
    let comparable = comparable_label(title);
    if starts_with_path(&comparable) {
        return false;
    }
    let title = placeholder_text(&comparable);
    !title.is_empty()
        && ![
            "agent session",
            "session",
            "agent",
            "untitled",
            "new chat",
            "new session",
            "conversation",
            "chat",
        ]
        .iter()
        .any(|placeholder| title.eq_ignore_ascii_case(placeholder))
}

fn placeholder_text(mut title: &str) -> &str {
    let mut cutoff = false;
    loop {
        // A cutoff can remove one or more closing brackets.
        cutoff |= title.trim_end_matches(['[', ']', ' ']).ends_with('…');
        title = title.trim_matches(['[', ']', '…', ' ']);
        let Some(open) = title.chars().next() else {
            return title;
        };
        let Some(close) = label_bracket_end(open) else {
            return title;
        };
        let inner = &title[open.len_utf8()..];
        let Some(inner) = inner.strip_suffix(close).or(cutoff.then_some(inner)) else {
            return title;
        };
        title = inner;
    }
}

/// Project or workspace display context with no filesystem or routing authority.
#[derive(Clone, PartialEq, Eq)]
pub struct DirectoryLabel(String);

impl DirectoryLabel {
    /// Validate a short, nonblank display label before admitting it to registration.
    pub fn parse(input: &str) -> Result<Self, AppError> {
        Self::from_stored(input)
            .filter(Self::is_project_label)
            .ok_or(AppError::InvalidDirectoryLabel)
    }

    /// Read legacy labels without applying current project-label validation.
    pub(crate) fn from_stored(input: &str) -> Option<Self> {
        let normalized = normalize_line(input.chars(), true);
        (!normalized.is_empty() && normalized.chars().count() <= DIRECTORY_SCALAR_LIMIT)
            .then_some(Self(normalized))
    }

    pub(crate) fn is_project_label(&self) -> bool {
        let label = comparable_label(&self.0);
        !label.is_empty() && !starts_with_path(&label)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Match only after leading display wrappers and whitespace in the comparison text.
fn starts_with_path(label: &str) -> bool {
    let prefix = label.trim_start_matches(|c: char| {
        c.is_whitespace()
            || label_bracket_end(c).is_some()
            || matches!(
                c,
                '\'' | '"' | '`' | '＇' | '＂' | '‘'..='‚' | '“'..='„' | '«' | '»' | '‹' | '›'
            )
    });
    let drive_path = matches!(
        prefix.as_bytes(),
        [drive, b':', b'/' | b'\\', ..] if drive.is_ascii_alphabetic()
    );
    prefix.starts_with(['/', '~']) || prefix.starts_with(r"\\") || drive_path
}

fn label_bracket_end(open: char) -> Option<char> {
    Some(match open {
        '[' => ']',
        '(' => ')',
        '{' => '}',
        '<' => '>',
        '（' => '）',
        '［' => '］',
        '｛' => '｝',
        '＜' => '＞',
        '〈' => '〉',
        '《' => '》',
        '「' => '」',
        '『' => '』',
        '【' => '】',
        '〔' => '〕',
        _ => return None,
    })
}

fn comparable_label(label: &str) -> Cow<'_, str> {
    // Ignore invisible modifiers for validation while preserving them in display text.
    // Treat blank Braille and Hangul spaces as separators, like ordinary whitespace.
    let is_blank = |c| matches!(c, '\u{2800}' | '\u{3164}' | '\u{ffa0}');
    let is_ignorable = |c| {
        matches!(c, '\u{34f}' | '\u{115f}'..='\u{1160}' | '\u{17b4}'..='\u{17b5}'
            | '\u{180b}'..='\u{180d}' | '\u{180f}' | '\u{200c}'..='\u{200d}'
            | '\u{fe00}'..='\u{fe0f}' | '\u{fff0}'..='\u{fff8}' | '\u{1bca0}'..='\u{1bca3}'
            | '\u{1d173}'..='\u{1d17a}' | '\u{e0000}'..='\u{e0fff}')
    };
    if label.contains(|c| is_blank(c) || is_ignorable(c)) {
        Cow::Owned(normalize_line(
            label
                .chars()
                .filter(|&c| !is_ignorable(c))
                .map(|c| if is_blank(c) { ' ' } else { c }),
            true,
        ))
    } else {
        Cow::Borrowed(label)
    }
}

fn normalize_line(input: impl Iterator<Item = char>, safe: bool) -> String {
    let mut normalized = String::new();
    let mut space = false;
    for c in input {
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

/// Constructed by validated formatters; header and optional body spans stay separate.
#[derive(Clone, PartialEq, Eq)]
pub struct FormattedMessage {
    text: String,
    entities: Vec<MessageEntity>,
}

impl FormattedMessage {
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Native Telegram spans in absolute UTF-16 coordinates of the complete visible text.
    pub fn entities(&self) -> &[MessageEntity] {
        &self.entities
    }

    /// Apply explicit entities to the verbatim body following this formatter's header.
    pub(crate) fn with_body_entities(
        mut self,
        body: &str,
        entities: &[MessageEntity],
    ) -> Result<Self, AppError> {
        validate_body_entities(body, entities)?;
        let framing = self
            .text
            .strip_suffix(body)
            .ok_or(AppError::InvalidRequest("body entities"))?
            .encode_utf16()
            .count();
        for entity in entities {
            let mut shifted = entity.clone();
            shifted.offset = framing
                .checked_add(entity.offset)
                .ok_or(AppError::InvalidRequest("body entities"))?;
            self.entities.push(shifted);
        }
        Ok(self)
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
redacted_debug!(
    SessionTitle,
    DirectoryLabel,
    FormattedMessage,
    MessageEntity
);

/// Format text/questions with native header entities and a verbatim body.
pub fn format_text(
    title: &SessionTitle,
    directory: Option<&DirectoryLabel>,
    body: &str,
) -> Result<FormattedMessage, AppError> {
    format_outbound("", Some(title.as_str()), directory, body, TEXT_UTF16_LIMIT)
}

/// An empty document description leaves only the title and optional directory.
pub fn format_caption(
    title: &SessionTitle,
    directory: Option<&DirectoryLabel>,
    body: &str,
) -> Result<FormattedMessage, AppError> {
    format_outbound(
        "",
        Some(title.as_str()),
        directory,
        body,
        CAPTION_UTF16_LIMIT,
    )
}

/// Every service notice begins with the exact prefix, before any known-session header.
pub fn format_notice(
    title: Option<&SessionTitle>,
    directory: Option<&DirectoryLabel>,
    body: &str,
) -> Result<FormattedMessage, AppError> {
    let title = title.map(|title| {
        if title.is_meaningful() {
            title.as_str()
        } else {
            FALLBACK_TITLE
        }
    });
    let directory = directory.filter(|label| label.is_project_label());
    format_outbound(SERVICE_PREFIX, title, directory, body, TEXT_UTF16_LIMIT)
}

fn format_outbound(
    prefix: &str,
    title: Option<&str>,
    directory: Option<&DirectoryLabel>,
    body: &str,
    native_limit_utf16: usize,
) -> Result<FormattedMessage, AppError> {
    // Unknown-session notices have no directory context either.
    let directory = directory.filter(|_| title.is_some());
    let separator = title.is_some() && !body.is_empty();
    let framing = 2 * usize::from(title.is_some())
        + 2 * usize::from(separator)
        + usize::from(directory.is_some());
    let (actual_characters, actual_utf16_units) = prefix
        .chars()
        .chain(title.into_iter().flat_map(str::chars))
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
            length: title.encode_utf16().count() + 2,
            url: None,
            language: None,
        });
        text.push('[');
        text.push_str(title);
        text.push(']');
    }
    if let Some(directory) = directory {
        text.push('\n');
        entities.push(MessageEntity {
            kind: EntityKind::Italic,
            offset: text.encode_utf16().count(),
            length: directory.as_str().encode_utf16().count(),
            url: None,
            language: None,
        });
        text.push_str(directory.as_str());
    }
    if separator {
        text.push_str("\n\n");
    }
    text.push_str(body);
    Ok(FormattedMessage { text, entities })
}

#[cfg(test)]
mod tests;
