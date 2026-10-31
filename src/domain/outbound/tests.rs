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
fn native_headers_use_utf16_coordinates_and_leave_bodies_literal() {
    let title = SessionTitle::new(" \t 🚀[Review] *draft*\n ");
    let directory = DirectoryLabel::new(" workspace/📁 <tag> ")
        .unwrap()
        .unwrap();
    let body = "  <code> & *literal*\n\nNext line\r\n ";
    let expected = format!("🚀[Review] *draft*\nworkspace/📁 <tag>\n{body}");
    let text = format_text(&title, Some(&directory), body).unwrap();
    assert_eq!(text.as_str(), expected);
    assert_eq!(
        format_caption(&title, Some(&directory), body).unwrap(),
        text
    );
    assert_eq!(
        serde_json::to_value(text.entities()).unwrap(),
        serde_json::json!([
            {"type":"bold", "offset":0, "length":18},
            {"type":"italic", "offset":19, "length":18}
        ])
    );
    let notice = format_notice(Some(&title), Some(&directory), body).unwrap();
    assert_eq!(notice.as_str(), format!("🔧 {expected}"));
    for (regular, service) in text.entities().iter().zip(notice.entities()) {
        assert_eq!(service.offset, regular.offset + 3); // 🔧 is two UTF-16 units.
        assert_eq!(service.length, regular.length);
        assert_eq!(service.kind, regular.kind);
    }
    let bare = format_notice(None, None, UNROUTABLE_NOTICE).unwrap();
    assert_eq!(bare.as_str(), format!("🔧 {UNROUTABLE_NOTICE}"));
    assert!(bare.entities().is_empty());
    assert_eq!(
        format_caption(&title, None, "").unwrap().as_str(),
        title.as_str()
    );
    assert_eq!(
        format_text(&title, Some(&directory), "").unwrap().as_str(),
        "🚀[Review] *draft*\nworkspace/📁 <tag>"
    );
}

#[test]
fn directory_labels_are_opt_in_bounded_safe_single_lines() {
    for input in ["", " \t\r\n\u{0}\u{202e}\u{2066}"] {
        assert_eq!(DirectoryLabel::new(input).unwrap(), None);
    }
    let label = DirectoryLabel::new("  workspace\r\n📁\u{202e}\u{2066}\u{0} <b> ")
        .unwrap()
        .unwrap();
    assert_eq!(label.as_str(), "workspace 📁 <b>");
    assert_eq!(SessionTitle::new("a\u{202e}b\u{2066}c").as_str(), "abc");
    let hundred = "📁".repeat(100);
    assert_eq!(
        DirectoryLabel::new(&hundred).unwrap().unwrap().as_str(),
        hundred
    );
    let error = DirectoryLabel::new(&(hundred + "x")).unwrap_err();
    assert_eq!(error, AppError::InvalidRequest("directory_label"));
    assert!(!format!("{label:?} {error:?}").contains("workspace"));
}

#[test]
fn text_captions_and_notices_count_every_visible_scalar() {
    let title = SessionTitle::new("T");
    let directory = DirectoryLabel::new("📁").unwrap().unwrap();
    for unit in ["a", "Я", "e\u{301}", "👨‍👩‍👧‍👦", "𐐀", "line\n"] {
        for (notice, with_directory, framing) in [
            (false, false, 2),
            (false, true, 4),
            (true, false, 4),
            (true, true, 6),
        ] {
            let directory = with_directory.then_some(&directory);
            let width = unit.chars().count();
            let budget = 1_000 - framing;
            let body = unit.repeat(budget / width) + &"x".repeat(budget % width);
            let format = |body: &str| {
                if notice {
                    format_notice(Some(&title), directory, body)
                } else {
                    format_text(&title, directory, body)
                }
            };
            let output = format(&body).unwrap();
            assert_eq!(output.as_str().chars().count(), 1_000);
            assert!(output.as_str().ends_with(&body));
            let details = length_details(format(&(body + "x")).unwrap_err());
            assert_eq!(details.actual_characters, 1_001);
            assert_eq!(
                details.actual_utf16_units,
                output.as_str().encode_utf16().count() + 1
            );
            assert_eq!(details.native_limit_utf16, 4_096);
        }
    }
    let body = "a".repeat(998);
    assert_eq!(
        format_caption(&title, None, &body)
            .unwrap()
            .as_str()
            .chars()
            .count(),
        1_000
    );
    let details = length_details(format_caption(&title, None, &(body + "x")).unwrap_err());
    assert_eq!(details.actual_characters, 1_001);
    assert_eq!(details.native_limit_utf16, 1_024);
    assert_eq!(
        format_notice(None, None, &"a".repeat(998))
            .unwrap()
            .as_str()
            .chars()
            .count(),
        1_000
    );
    let details = length_details(format_notice(None, None, &"a".repeat(999)).unwrap_err());
    assert_eq!(details.actual_characters, 1_001);
}

#[test]
fn native_caption_boundary_includes_supplementary_title_directory_and_body() {
    let title = SessionTitle::new("🚀");
    let directory = DirectoryLabel::new("📁").unwrap().unwrap();
    // Four framing scalars plus 996 body scalars; 24 astral symbols add 24 UTF-16 units.
    let body = "🚀".repeat(22) + &"a".repeat(974);
    let caption = format_caption(&title, Some(&directory), &body).unwrap();
    assert_eq!(caption.as_str().chars().count(), 1_000);
    assert_eq!(caption.as_str().encode_utf16().count(), 1_024);
    let body = "🚀".repeat(23) + &"a".repeat(973);
    let details = length_details(format_caption(&title, Some(&directory), &body).unwrap_err());
    assert_eq!(details.actual_characters, 1_000);
    assert_eq!(details.actual_utf16_units, 1_025);
    assert_eq!(details.native_limit_utf16, 1_024);
    assert_eq!(
        format_text(&title, Some(&directory), &body)
            .unwrap()
            .as_str()
            .chars()
            .count(),
        1_000
    );
}

#[test]
fn normalization_precedes_length_checks_and_large_graphemes_are_not_shortened() {
    let title = SessionTitle::new(&format!(" \n{} discarded", "a".repeat(50)));
    let body = "b".repeat(949);
    assert_eq!(
        format_text(&title, None, &body)
            .unwrap()
            .as_str()
            .chars()
            .count(),
        1_000
    );
    let huge_grapheme = format!("a{}", "\u{301}".repeat(1_000));
    let title = SessionTitle::new(&huge_grapheme);
    assert_eq!(title.as_str(), huge_grapheme);
    for result in [
        format_text(&title, None, ""),
        format_caption(&title, None, ""),
    ] {
        let details = length_details(result.unwrap_err());
        assert_eq!(details.actual_characters, 1_001);
        assert_eq!(details.actual_utf16_units, 1_001);
    }
}

#[test]
fn service_recognition_preserves_legacy_notices_and_rejects_other_bodies() {
    for text in [
        UNCONFIRMED_NOTICE.to_owned(),
        UNROUTABLE_NOTICE.to_owned(),
        format!("[Old title]\n{UNCONFIRMED_NOTICE}"),
        format!("[Old\u{202e} title]\n{UNCONFIRMED_NOTICE}"),
        format!("🔧 {UNROUTABLE_NOTICE}"),
        format!("🔧 Title\n{UNCONFIRMED_NOTICE}"),
        format!("🔧 Title\nworkspace/📁\n{UNCONFIRMED_NOTICE}"),
    ] {
        assert!(is_service_notice(&text));
    }
    for text in [
        "🔧 ordinary agent text".to_owned(),
        format!("🔧 Title\nworkspace\nextra line\n{UNCONFIRMED_NOTICE}"),
        format!("🔧 Title\n{UNCONFIRMED_NOTICE}\nextra text"),
        format!("Title\n{UNCONFIRMED_NOTICE}"),
    ] {
        assert!(!is_service_notice(&text));
    }
}

#[test]
fn diagnostics_expose_counts_and_guidance_without_input() {
    let title = SessionTitle::new("synthetic-private-title");
    let body = "synthetic-private-body".repeat(100);
    let error = format_text(&title, None, &body).unwrap_err();
    let diagnostics = format!("{error} {error:?} {title:?}");
    assert!(!diagnostics.contains("synthetic-private"));
    assert!(diagnostics.contains("TEXT_TOO_LONG"));
    assert!(diagnostics.contains("telegram_send_file"));
    let details = length_details(error);
    assert_eq!(details.actual_characters, 2_224);
}
