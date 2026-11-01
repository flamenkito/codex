//! Prefixed rows preserve authored whitespace and exclude display gutters from source text.

use super::*;
use pretty_assertions::assert_eq;
use std::sync::Arc;

#[test]
fn user_prompt_gutters_preserve_source_ranges_on_wrapped_unicode_lines() {
    let message = "Привет 世界 https://example.test/a long wrapped message\nещё одна строка";
    for cell in [
        new_user_prompt(message.to_string(), Vec::new(), Vec::new(), Vec::new()),
        new_spoken_user_prompt(message.to_string()),
        new_user_prompt(
            message.to_string(),
            Vec::new(),
            vec![PathBuf::from("fixture.png")],
            Vec::new(),
        ),
    ] {
        for width in [12, 24, 60] {
            let lines = cell.transcript_hyperlink_lines(width);
            for line in &lines {
                let visible = line.line.to_string();
                if let Some(source) = &line.source {
                    assert_eq!(
                        visible.get(source.prefix_bytes..source.prefix_bytes + source.range.len()),
                        Some(&source.text[source.range.clone()]),
                        "width {width}, row {visible:?}",
                    );
                }
                assert!(line.width() <= usize::from(width));
            }
        }
    }
}

#[test]
fn prefixed_wrapping_retains_one_source_line_across_widths_and_gutters() {
    let source = "  let result = compute(alpha, beta, gamma);  print(result);";
    let styled = Line::from(vec![
        "  let result = ".into(),
        "compute".red().bold(),
        "(alpha, beta, gamma);  print(result);".into(),
    ])
    .cyan()
    .italic();
    let cell = PrefixedWrappedHistoryCell::new(styled, "✔ ".green(), "    ");
    for width in [16, 24, 48] {
        let lines = cell.transcript_hyperlink_lines(width);
        let first = lines
            .first()
            .expect("wrapped source")
            .source
            .as_ref()
            .expect("source range");
        assert_eq!(first.text.as_ref(), source);
        assert_eq!(
            first.styled_range(0..source.len()),
            Line::from(vec![
                "  let result = ".cyan().italic(),
                "compute".red().bold().italic(),
                "(alpha, beta, gamma);  print(result);".cyan().italic(),
            ])
        );
        assert_eq!(first.range.start, 0);
        let restyled = lines[0].clone().style(Style::new().on_blue());
        assert_eq!(
            restyled
                .source
                .as_ref()
                .unwrap()
                .styled_range(0..source.len()),
            Line::from(vec![
                "  let result = ".cyan().italic().on_blue(),
                "compute".red().bold().italic().on_blue(),
                "(alpha, beta, gamma);  print(result);"
                    .cyan()
                    .italic()
                    .on_blue(),
            ])
        );
        for line in &lines {
            let origin = line.source.as_ref().expect("source range");
            let visible = line.line.to_string();
            assert!(Arc::ptr_eq(&origin.text, &first.text));
            assert_eq!(
                &visible[origin.prefix_bytes..origin.prefix_bytes + origin.range.len()],
                &source[origin.range.clone()],
            );
            assert!(line.line.width() <= usize::from(width));
        }
        assert_eq!(
            lines
                .last()
                .expect("last row")
                .source
                .as_ref()
                .expect("source")
                .range
                .end,
            source.len()
        );
    }
}

#[test]
fn prefixed_wrapping_keeps_hard_lines_distinct_even_when_their_text_repeats() {
    let cell = PrefixedWrappedHistoryCell::new(
        Text::from(vec![
            Line::from("repeated text"),
            Line::from("repeated text"),
        ]),
        "✔ ",
        "  ",
    );
    let lines = cell.transcript_hyperlink_lines(/*width*/ 40);
    let first = lines[0].source.as_ref().expect("first source");
    let second = lines[1].source.as_ref().expect("second source");
    assert_eq!(
        (first.text.as_ref(), second.text.as_ref()),
        ("repeated text", "repeated text")
    );
    assert!(!Arc::ptr_eq(&first.text, &second.text));
    assert!(cell.display_hyperlink_lines(/*width*/ 0).is_empty());
}
