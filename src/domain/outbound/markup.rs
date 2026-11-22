//! Strict outbound Markdown converted to literal text and body-relative Telegram entities.

use std::borrow::Cow;

use rmcp::schemars::JsonSchema;
use serde::Deserialize;

use super::{
    EntityKind, MessageEntity,
    entities::{BODY_ENTITY_LIMIT, valid_entity_url, valid_pre_language},
};
use crate::error::AppError;

/// Select strict Markdown or literal text for an outbound MCP body.
#[derive(Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(crate = "rmcp::schemars")]
pub enum OutboundFormat {
    Markdown,
    Plain,
}

type PreparedBody<'a> = (Cow<'a, str>, Cow<'a, [MessageEntity]>);

/// Keep the legacy native-entity path verbatim when format is omitted.
pub(crate) fn prepare_body<'a>(
    input: &'a str,
    format: Option<OutboundFormat>,
    entities: &'a [MessageEntity],
) -> Result<PreparedBody<'a>, AppError> {
    match (format, entities.is_empty()) {
        (Some(OutboundFormat::Plain), _) | (None, false) => Ok((input.into(), entities.into())),
        (Some(OutboundFormat::Markdown), false) => Err(AppError::InvalidRequest(
            "format (use plain with native entities)",
        )),
        (None | Some(OutboundFormat::Markdown), true) => {
            let (text, entities) = Parser::new(input).parse()?;
            Ok((text.into(), entities.into()))
        }
    }
}

struct Parser<'a> {
    input: &'a str,
    rest: &'a str,
    text: String,
    units: usize,
    entities: Vec<MessageEntity>,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input,
            rest: input,
            text: String::with_capacity(input.len()),
            units: 0,
            entities: Vec::new(),
        }
    }

    fn parse(mut self) -> Result<(String, Vec<MessageEntity>), AppError> {
        while let Some(c) = self.rest.chars().next() {
            let at = self.byte();
            match c {
                '*' => self.style()?,
                '`' if self.rest.starts_with("```") => self.fence()?,
                '`' => self.code()?,
                '[' => self.link()?,
                ']' => return Err(self.invalid(at)),
                _ => {
                    let c = self.character(c)?;
                    self.push(c);
                }
            }
        }
        Ok((self.text, self.entities))
    }

    fn style(&mut self) -> Result<(), AppError> {
        let at = self.byte();
        let offset = self.units;
        let content = self.rest.trim_start_matches('*');
        let stars = self.rest.len() - content.len();
        let kind = match stars {
            1 => EntityKind::Italic,
            2 => EntityKind::Bold,
            _ => return Err(self.invalid(at)),
        };
        if content.starts_with(char::is_whitespace) {
            return Err(self.invalid(at));
        }
        self.rest = content;
        loop {
            match self.rest.chars().next() {
                None => return Err(self.invalid(at)),
                Some('*') => {
                    let after = self.rest.trim_start_matches('*');
                    if self.rest.len() - after.len() != stars
                        || self.text.ends_with(char::is_whitespace)
                    {
                        return Err(self.invalid(self.byte()));
                    }
                    self.rest = after;
                    return self.entity(kind, offset, at).map(|_| ());
                }
                Some('`' | '[' | ']' | '\r' | '\n') => return Err(self.invalid(self.byte())),
                Some(c) => {
                    let c = self.character(c)?;
                    self.push(c);
                }
            }
        }
    }

    fn code(&mut self) -> Result<(), AppError> {
        let at = self.byte();
        let offset = self.units;
        self.rest = &self.rest[1..];
        while let Some(c) = self.rest.chars().next() {
            match c {
                '`' => {
                    self.rest = &self.rest[1..];
                    return self.entity(EntityKind::Code, offset, at).map(|_| ());
                }
                '\r' | '\n' => break,
                '\\' if self.rest[1..].starts_with(['\\', '`']) => {
                    let c = self.character(c)?;
                    self.push(c);
                }
                _ => {
                    self.rest = &self.rest[c.len_utf8()..];
                    self.push(c);
                }
            }
        }
        Err(self.invalid(at))
    }

    fn fence(&mut self) -> Result<(), AppError> {
        let at = self.byte();
        let offset = self.units;
        if at > 0 && self.input.as_bytes()[at - 1] != b'\n' {
            return Err(self.invalid(at));
        }
        let Some((opening, body)) = self.rest.split_once('\n') else {
            return Err(self.invalid(at));
        };
        let language = opening[3..].strip_suffix('\r').unwrap_or(&opening[3..]);
        if !language.is_empty() && (!valid_pre_language(language) || language.contains('`')) {
            return Err(self.invalid(at + 3));
        }
        let mut end = 0;
        for line in body.split_inclusive('\n') {
            if without_line_ending(line) == "```" {
                let content = without_line_ending(&body[..end]);
                self.text.push_str(content);
                self.units += content.encode_utf16().count();
                // Keep the newline following the closing fence as a body separator.
                self.rest = &body[end + 3..];
                self.entity(EntityKind::Pre, offset, at)?.language =
                    (!language.is_empty()).then(|| language.to_owned());
                return Ok(());
            }
            end += line.len();
        }
        Err(self.invalid(at))
    }

    fn link(&mut self) -> Result<(), AppError> {
        let at = self.byte();
        let offset = self.units;
        let first_entity = self.entities.len();
        self.rest = &self.rest[1..];
        loop {
            match self.rest.chars().next() {
                None => return Err(self.invalid(at)),
                Some(']') => break,
                Some('*') => self.style()?,
                Some('[' | '`' | '\r' | '\n') => return Err(self.invalid(self.byte())),
                Some(c) => {
                    let c = self.character(c)?;
                    self.push(c);
                }
            }
        }
        self.rest = self
            .rest
            .strip_prefix("](")
            .ok_or_else(|| self.invalid(at))?;
        let url_at = self.byte();
        let mut url = String::new();
        loop {
            match self.rest.chars().next() {
                None => return Err(self.invalid(at)),
                Some(')') => {
                    self.rest = &self.rest[1..];
                    break;
                }
                Some('(') => return Err(self.invalid(self.byte())),
                Some(c) => url.push(self.character(c)?),
            }
        }
        // The URL parser can skip extra slashes in HTTP(S) URLs. Require the authority here.
        let web_authority = url.split_once("://").is_some_and(|(scheme, rest)| {
            (scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
                && !rest.is_empty()
                && !rest.starts_with(['/', '?', '#'])
        });
        if !web_authority || url.contains('\\') || !valid_entity_url(&url) {
            return Err(self.invalid(url_at));
        }
        self.entity(EntityKind::TextLink, offset, at)?.url = Some(url);
        // Put the outer link before any style spans in its label.
        self.entities[first_entity..].rotate_right(1);
        Ok(())
    }

    fn character(&mut self, c: char) -> Result<char, AppError> {
        let at = self.byte();
        self.rest = &self.rest[c.len_utf8()..];
        if c != '\\' {
            return Ok(c);
        }
        let Some(escaped @ ('\\' | '*' | '`' | '[' | ']' | '(' | ')')) = self.rest.chars().next()
        else {
            return Err(self.invalid(at));
        };
        self.rest = &self.rest[1..];
        Ok(escaped)
    }

    fn push(&mut self, c: char) {
        self.text.push(c);
        self.units += c.len_utf16();
    }

    fn entity(
        &mut self,
        kind: EntityKind,
        offset: usize,
        at: usize,
    ) -> Result<&mut MessageEntity, AppError> {
        if self.entities.len() == BODY_ENTITY_LIMIT || self.units == offset {
            return Err(self.invalid(at));
        }
        self.entities.push(MessageEntity {
            kind,
            offset,
            length: self.units - offset,
            url: None,
            language: None,
        });
        Ok(self.entities.last_mut().expect("entity was just inserted"))
    }

    fn byte(&self) -> usize {
        self.input.len() - self.rest.len()
    }

    fn invalid(&self, at: usize) -> AppError {
        AppError::InvalidMarkup {
            position: self.input[..at].chars().count(),
        }
    }
}

fn without_line_ending(line: &str) -> &str {
    line.strip_suffix("\r\n")
        .or_else(|| line.strip_suffix('\n'))
        .unwrap_or(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::outbound::validate_body_entities;

    #[test]
    fn renders_literal_bodies_and_native_utf16_spans() {
        use EntityKind::{Bold, Code, Italic, Pre, TextLink};
        // (kind, UTF-16 offset, UTF-16 length, URL or language)
        type Span<'a> = (EntityKind, usize, usize, Option<&'a str>);
        let cases: &[(&str, &str, &[Span<'_>])] = &[
            (
                "XML <tag> and snake_case (ok)\n\n- first\n1. second",
                "XML <tag> and snake_case (ok)\n\n- first\n1. second",
                &[],
            ),
            (r"\*\[x\]\(y\)\\\`", "*[x](y)\\`", &[]),
            (
                "😀 **👩‍💻** *e\u{301}* `x_y`",
                "😀 👩‍💻 e\u{301} x_y",
                &[
                    (Bold, 3, 5, None),
                    (Italic, 9, 2, None),
                    (Code, 12, 3, None),
                ],
            ),
            (
                "*first* *second*",
                "first second",
                &[(Italic, 0, 5, None), (Italic, 6, 6, None)],
            ),
            (r"**\*\*x\***", "**x*", &[(Bold, 0, 4, None)]),
            (
                "*outer“*😀*”tail*",
                "outer“😀”tail",
                &[(Italic, 0, 6, None), (Italic, 8, 5, None)],
            ),
            ("**Done!**Next", "Done!Next", &[(Bold, 0, 5, None)]),
            (
                "**Select (**then **continue**",
                "Select (then continue",
                &[(Bold, 0, 8, None), (Bold, 13, 8, None)],
            ),
            (
                "**Select (**/**😀**",
                "Select (/😀",
                &[(Bold, 0, 8, None), (Bold, 9, 2, None)],
            ),
            (
                "[**Done!**Next](https://example.invalid)",
                "Done!Next",
                &[
                    (TextLink, 0, 9, Some("https://example.invalid")),
                    (Bold, 0, 5, None),
                ],
            ),
            (
                "[Parenthesis: *(*](https://example.invalid)",
                "Parenthesis: (",
                &[
                    (TextLink, 0, 14, Some("https://example.invalid")),
                    (Italic, 13, 1, None),
                ],
            ),
            (r"`*x*\`[y]`", "*x*`[y]", &[(Code, 0, 7, None)]),
            (
                r"\[ [**😀** *docs*](https://example.invalid/a\(b\))",
                "[ 😀 docs",
                &[
                    (TextLink, 2, 7, Some("https://example.invalid/a(b)")),
                    (Bold, 2, 2, None),
                    (Italic, 5, 4, None),
                ],
            ),
            (
                "[link](HtTp://example.invalid)",
                "link",
                &[(TextLink, 0, 4, Some("HtTp://example.invalid"))],
            ),
            (
                "[**outer(**inner**)tail**](https://example.invalid)",
                "outer(inner)tail",
                &[
                    (TextLink, 0, 16, Some("https://example.invalid")),
                    (Bold, 0, 6, None),
                    (Bold, 11, 5, None),
                ],
            ),
            (
                "Before\n```rust\r\n😀 *x*\r\ny\r\n```\nAfter",
                "Before\n😀 *x*\r\ny\nAfter",
                &[(Pre, 7, 9, Some("rust"))],
            ),
            ("```\nsnake_case\n```", "snake_case", &[(Pre, 0, 10, None)]),
        ];
        for &(input, expected, entities) in cases {
            let (text, actual) = Parser::new(input).parse().unwrap();
            assert_eq!(text, expected);
            validate_body_entities(&text, &actual).unwrap();
            let spans: Vec<_> = actual
                .iter()
                .map(|span| {
                    (
                        span.kind,
                        span.offset,
                        span.length,
                        span.url.as_deref().or(span.language.as_deref()),
                    )
                })
                .collect();
            assert_eq!(spans, entities, "{input:?}");
        }
    }

    #[test]
    fn rejects_invalid_markup_at_a_scalar_position_without_echoing_input() {
        for (input, position) in [
            ("😀é **unclosed", 3),
            ("*unclosed", 0),
            ("**", 0),
            ("****", 0),
            ("*first**second*", 6),
            ("*a***b***c*", 2),
            ("**bold*", 6),
            ("* leading*", 0),
            ("**\u{a0}bold**", 0),
            ("*trailing *", 10),
            ("**bold\u{a0}**", 7),
            ("*a\nb*", 2),
            ("*a`b`*", 2),
            ("*[x](https://example.invalid)*", 1),
            ("**bold *nested***", 7),
            ("*italic **nested** after*", 8),
            ("[*italic **nested** after*](https://example.invalid)", 9),
            ("*outer“**😀**”tail*", 7),
            ("😀 *outer（***inner***）tail*", 9),
            ("``", 0),
            ("`open", 0),
            ("dangling ]", 9),
            ("[plain brackets]", 0),
            ("[`code`](https://example.invalid)", 1),
            ("[outer [nested]](https://example.invalid)", 7),
            ("[name](mailto:user@example.invalid)", 7),
            ("[name](https://)", 7),
            ("[label](https:///example.invalid)", 8),
            ("[label](HTTP:////example.invalid)", 8),
            ("[label](https://?example.invalid)", 8),
            ("[label](https://#example.invalid)", 8),
            (r"[label](https://example.invalid\\path)", 8),
            ("[name](https://example.invalid/a b)", 7),
            ("[name](https://example.invalid/a(b))", 32),
            ("[](http://example.invalid)", 0),
            ("prefix ```\nx\n```", 7),
            ("```bad lang\nx\n```", 3),
            ("```\n```", 0),
            ("```\nunclosed", 0),
            (r"\q", 0),
            ("\\", 0),
        ] {
            let Err(error) = Parser::new(input).parse() else {
                panic!("accepted invalid markup: {input:?}");
            };
            assert_eq!(error, AppError::InvalidMarkup { position }, "{input:?}");
            assert_eq!(error.code(), "MARKUP_INVALID");
        }
    }

    #[test]
    fn bounds_generated_entities_including_links_with_label_styles() {
        let input = "*x* ".repeat(BODY_ENTITY_LIMIT);
        let (text, entities) = Parser::new(&input).parse().unwrap();
        assert_eq!(entities.len(), BODY_ENTITY_LIMIT);
        validate_body_entities(&text, &entities).unwrap();
        let input = "*x* ".repeat(BODY_ENTITY_LIMIT - 1) + "[**x**](https://example.invalid)";
        assert_eq!(
            Parser::new(&input).parse().err(),
            Some(AppError::InvalidMarkup { position: 396 })
        );
    }
}
