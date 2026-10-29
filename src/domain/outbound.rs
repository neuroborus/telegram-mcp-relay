//! Plain-text session headers and final outbound Unicode limits.

use std::fmt;

use unicode_segmentation::UnicodeSegmentation;

use crate::{
    config::OUTBOUND_SCALAR_LIMIT,
    error::{AppError, TextTooLong},
};

const TITLE_GRAPHEME_LIMIT: usize = 50;
const FALLBACK_TITLE: &str = "Agent Session";
// Conservatively count native Telegram limits in UTF-16 units, without entity parsing.
const TEXT_UTF16_LIMIT: usize = 4_096;
const CAPTION_UTF16_LIMIT: usize = 1_024;

/// A single-line display label, never a session identity or routing key.
#[derive(Clone, PartialEq, Eq)]
pub struct SessionTitle(String);

impl SessionTitle {
    /// Collapse Unicode whitespace, remove other controls and keep at most 50 graphemes.
    /// A single grapheme can contain many scalars; final output still needs validation.
    pub fn new(input: &str) -> Self {
        let mut normalized = String::new();
        let mut space = false;
        for c in input.chars() {
            if c.is_whitespace() {
                space = !normalized.is_empty();
            } else if !c.is_control() {
                if space {
                    normalized.push(' ');
                }
                normalized.push(c);
                space = false;
            }
        }
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

impl fmt::Debug for SessionTitle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SessionTitle([REDACTED])")
    }
}

/// Format a session text or question. The body is preserved verbatim, without markup parsing.
pub fn format_text(title: &SessionTitle, body: &str) -> Result<String, AppError> {
    format_outbound(Some(title), body, TEXT_UTF16_LIMIT)
}

/// Format a document caption; an empty description produces only the bracketed title.
pub fn format_caption(title: &SessionTitle, body: &str) -> Result<String, AppError> {
    format_outbound(Some(title), body, CAPTION_UTF16_LIMIT)
}

/// Format a service notice; an unknown route has no session title to prefix.
pub fn format_notice(title: Option<&SessionTitle>, body: &str) -> Result<String, AppError> {
    format_outbound(title, body, TEXT_UTF16_LIMIT)
}

fn format_outbound(
    title: Option<&SessionTitle>,
    body: &str,
    native_limit_utf16: usize,
) -> Result<String, AppError> {
    let separator = title.is_some() && !body.is_empty();
    let framing = if title.is_some() { 2 } else { 0 } + usize::from(separator);
    // Count the final header, optional newline and body before allocating the output.
    let (actual_characters, actual_utf16_units) = title
        .into_iter()
        .flat_map(|title| title.as_str().chars())
        .chain(body.chars())
        .fold((framing, framing), |(scalars, units), c| {
            (scalars + 1, units + c.len_utf16())
        });
    if actual_characters == 0 {
        return Err(AppError::EmptyText);
    }
    if actual_characters > OUTBOUND_SCALAR_LIMIT || actual_utf16_units > native_limit_utf16 {
        return Err(AppError::TextTooLong(TextTooLong {
            limit: OUTBOUND_SCALAR_LIMIT,
            actual_characters,
            native_limit_utf16,
            actual_utf16_units,
            guidance: "Put the full material in a .md, .txt or other suitable file and use telegram_send_file with a short caption.",
        }));
    }

    let mut output = String::new();
    if let Some(title) = title {
        output.push('[');
        output.push_str(title.as_str());
        output.push(']');
    }
    if separator {
        output.push('\n');
    }
    output.push_str(body);
    Ok(output)
}

#[cfg(test)]
mod tests;
