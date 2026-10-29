use super::*;

fn length_details(error: AppError) -> TextTooLong {
    assert_eq!(error.code(), "TEXT_TOO_LONG");
    let AppError::TextTooLong(details) = error else {
        panic!("expected structured length details");
    };
    assert_eq!(details.limit, 1_000);
    assert!(details.guidance.contains("telegram_send_file"));
    assert!(details.guidance.contains(".md") && details.guidance.contains(".txt"));
    details
}

#[test]
fn titles_normalize_whitespace_remove_controls_and_fall_back() {
    for (input, expected) in [
        ("", "Agent Session"),
        (" \t\r\n\u{0}\u{7f}\u{85}\u{2028}\u{a0}", "Agent Session"),
        (
            "  Alpha\t\r\n Beta\u{0}\u{7f}\u{2003}Gamma\u{2029} ",
            "Alpha Beta Gamma",
        ),
        ("a\u{0}b\u{1b}c", "abc"),
        (" [A] *bold* <tag> ", "[A] *bold* <tag>"),
    ] {
        let title = SessionTitle::new(input);
        assert_eq!(title.as_str(), expected);
        assert_eq!(SessionTitle::new(title.as_str()), title);
    }
}

#[test]
fn title_cutoff_preserves_whole_extended_graphemes() {
    for grapheme in [
        "A",
        "Я",
        "e\u{301}",
        "👩🏽‍💻",
        "👨‍👩‍👧‍👦",
        "🇫🇷",
        "\u{1100}\u{1161}\u{11a8}",
    ] {
        let fifty = grapheme.repeat(50);
        assert_eq!(SessionTitle::new(&fifty).as_str(), fifty);
        assert_eq!(SessionTitle::new(&grapheme.repeat(51)).as_str(), fifty);
    }
    // A separator at the truncation boundary must not leave trailing whitespace.
    let input = format!("{} b", "a".repeat(49));
    assert_eq!(SessionTitle::new(&input).as_str(), "a".repeat(49));
}

#[test]
fn headers_are_shared_plain_text_and_bodies_are_unchanged() {
    let title = SessionTitle::new(" \t [Review] *draft*\n ");
    let body = "  <code> & *literal*\n\nNext line\r\n ";
    let expected = format!("[[Review] *draft*]\n{body}");
    assert_eq!(format_text(&title, body).unwrap(), expected);
    assert_eq!(format_caption(&title, body).unwrap(), expected);
    assert_eq!(format_notice(Some(&title), body).unwrap(), expected);
    assert_eq!(format_notice(None, body).unwrap(), body);
    assert_eq!(format_caption(&title, "").unwrap(), "[[Review] *draft*]");
    assert_eq!(format_text(&title, "").unwrap(), "[[Review] *draft*]");
    assert_eq!(format_notice(None, ""), Err(AppError::EmptyText));
    assert_eq!(AppError::EmptyText.code(), "TEXT_EMPTY");
}

#[test]
fn text_questions_and_notices_enforce_final_scalar_boundaries() {
    let title = SessionTitle::new("T");
    for unit in ["a", "Я", "e\u{301}", "👨‍👩‍👧‍👦", "𐐀", "line\n"] {
        // Four header scalars leave 996 for the body, regardless of its UTF-8 size.
        let width = unit.chars().count();
        let body = unit.repeat(996 / width) + &"x".repeat(996 % width);
        let expected = format!("[T]\n{body}");
        for result in [
            format_text(&title, &body),
            format_notice(Some(&title), &body),
        ] {
            let output = result.unwrap();
            assert_eq!(output, expected);
            assert_eq!(output.chars().count(), 1_000);
        }
        let oversized = format!("{body}x");
        for result in [
            format_text(&title, &oversized),
            format_notice(Some(&title), &oversized),
        ] {
            let details = length_details(result.unwrap_err());
            assert_eq!(details.actual_characters, 1_001);
            assert_eq!(
                details.actual_utf16_units,
                expected.encode_utf16().count() + 1
            );
            assert_eq!(details.native_limit_utf16, 4_096);
        }
    }
    assert_eq!(
        format_notice(None, &"a".repeat(1_000)).unwrap(),
        "a".repeat(1_000)
    );
    let details = length_details(format_notice(None, &"a".repeat(1_001)).unwrap_err());
    assert_eq!(details.actual_characters, 1_001);
}

#[test]
fn captions_enforce_the_bridge_cap_for_bmp_text() {
    let title = SessionTitle::new("T");
    for unit in ["a", "Я", "e\u{301}", "line\n"] {
        let width = unit.chars().count();
        let body = unit.repeat(996 / width) + &"x".repeat(996 % width);
        assert_eq!(
            format_caption(&title, &body).unwrap(),
            format!("[T]\n{body}")
        );
        let details = length_details(format_caption(&title, &(body + "x")).unwrap_err());
        assert_eq!(details.actual_characters, 1_001);
        assert_eq!(details.actual_utf16_units, 1_001);
        assert_eq!(details.native_limit_utf16, 1_024);
    }
}

#[test]
fn native_caption_boundary_includes_supplementary_title_and_body() {
    let title = SessionTitle::new("🚀");
    // 1,000 scalars, including the header; 24 supplementary scalars add 24 UTF-16 units.
    let body = "🚀".repeat(23) + &"a".repeat(973);
    let caption = format_caption(&title, &body).unwrap();
    assert_eq!(caption.chars().count(), 1_000);
    assert_eq!(caption.encode_utf16().count(), 1_024);

    let body = "🚀".repeat(24) + &"a".repeat(972);
    let details = length_details(format_caption(&title, &body).unwrap_err());
    assert_eq!(details.actual_characters, 1_000);
    assert_eq!(details.actual_utf16_units, 1_025);
    assert_eq!(details.native_limit_utf16, 1_024);
    // The native text bound permits the same scalar-valid body in a message/question.
    assert_eq!(format_text(&title, &body).unwrap().chars().count(), 1_000);

    let details = length_details(format_caption(&title, &"😀".repeat(510)).unwrap_err());
    assert_eq!(details.actual_characters, 514);
    assert_eq!(details.actual_utf16_units, 1_025);
}

#[test]
fn normalization_precedes_length_checks_and_large_graphemes_are_not_shortened() {
    let title = SessionTitle::new(&format!(" \n{} discarded", "a".repeat(50)));
    let body = "b".repeat(947);
    assert_eq!(format_text(&title, &body).unwrap().chars().count(), 1_000);

    // One visible grapheme may already exceed the scalar cap, even without a description.
    let huge_grapheme = format!("a{}", "\u{301}".repeat(1_000));
    let title = SessionTitle::new(&huge_grapheme);
    assert_eq!(title.as_str(), huge_grapheme);
    for result in [format_text(&title, ""), format_caption(&title, "")] {
        let details = length_details(result.unwrap_err());
        assert_eq!(details.actual_characters, 1_003);
        assert_eq!(details.actual_utf16_units, 1_003);
    }
}

#[test]
fn diagnostics_expose_counts_and_guidance_without_input() {
    let title = SessionTitle::new("synthetic-private-title");
    let body = "synthetic-private-body".repeat(100);
    let error = format_text(&title, &body).unwrap_err();
    let diagnostics = format!("{error} {error:?} {title:?}");
    assert!(!diagnostics.contains("synthetic-private"));
    assert!(diagnostics.contains("TEXT_TOO_LONG"));
    assert!(diagnostics.contains("telegram_send_file"));
    let details = length_details(error);
    assert_eq!(details.actual_characters, 2_226);
}
