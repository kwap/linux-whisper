use serde::{Deserialize, Serialize};

use crate::model::Segment;

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

/// Controls which formatting passes are applied to transcribed text.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct FormatOptions {
    /// Master switch — when `false`, formatting returns the input unchanged.
    pub enabled: bool,
    /// Use a local LLM for intelligent formatting (opt-in, requires model download).
    pub llm_enabled: bool,
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            llm_enabled: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Public API — basic timestamp-gap formatter
// ---------------------------------------------------------------------------

/// Minimum gap (in seconds) between consecutive segments to insert a paragraph break.
const PARAGRAPH_GAP_SECS: f64 = 1.5;

/// Format transcribed segments using timestamp gaps for paragraph breaks.
///
/// When the gap between one segment's end and the next segment's start exceeds
/// [`PARAGRAPH_GAP_SECS`], a paragraph break (`\n\n`) is inserted. Whitespace
/// is normalized, sentences are capitalized, and space-before-punctuation is
/// cleaned up.
pub fn basic_format_segments(segments: &[Segment], opts: &FormatOptions) -> String {
    if !opts.enabled || segments.is_empty() {
        return join_segments(segments);
    }

    let mut result = String::with_capacity(segments.len() * 40);

    for (i, seg) in segments.iter().enumerate() {
        let text = seg.text.trim();
        if text.is_empty() {
            continue;
        }

        if i > 0 && !result.is_empty() {
            let prev = &segments[i - 1];
            let gap = seg.start - prev.end;

            if gap > PARAGRAPH_GAP_SECS {
                // Trim trailing whitespace before paragraph break.
                let trimmed = result.trim_end().to_string();
                result.clear();
                result.push_str(&trimmed);
                result.push_str("\n\n");
            } else {
                result.push(' ');
            }
        }

        result.push_str(text);
    }

    let mut text = normalize_whitespace(&result);
    text = capitalize_sentences(&text);
    text = cleanup(&text);
    text
}

/// Simple join of segment texts (used when formatting is disabled).
fn join_segments(segments: &[Segment]) -> String {
    segments
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

fn normalize_whitespace(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut prev_space = false;

    for ch in input.chars() {
        if ch == '\n' {
            // Preserve newlines.
            prev_space = false;
            result.push(ch);
        } else if ch == ' ' || ch == '\t' {
            if !prev_space && !result.is_empty() && !result.ends_with('\n') {
                result.push(' ');
            }
            prev_space = true;
        } else {
            prev_space = false;
            result.push(ch);
        }
    }

    result.trim().to_string()
}

fn capitalize_sentences(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut capitalize_next = true;

    for ch in input.chars() {
        if capitalize_next && ch.is_alphabetic() {
            for upper in ch.to_uppercase() {
                result.push(upper);
            }
            capitalize_next = false;
        } else {
            result.push(ch);
            if ch == '.' || ch == '!' || ch == '?' || ch == '\n' {
                capitalize_next = true;
            } else if ch != ' ' {
                capitalize_next = false;
            }
        }
    }

    result
}

fn cleanup(input: &str) -> String {
    let mut text = input.to_string();

    // Remove space before punctuation.
    text = text.replace(" .", ".");
    text = text.replace(" ,", ",");
    text = text.replace(" !", "!");
    text = text.replace(" ?", "?");
    text = text.replace(" ;", ";");
    text = text.replace(" :", ":");

    // Collapse 3+ newlines to exactly 2.
    while text.contains("\n\n\n") {
        text = text.replace("\n\n\n", "\n\n");
    }

    // Trim trailing whitespace per line.
    text = text
        .lines()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n");

    text.trim().to_string()
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Segment;

    fn default_opts() -> FormatOptions {
        FormatOptions::default()
    }

    fn disabled_opts() -> FormatOptions {
        FormatOptions {
            enabled: false,
            ..Default::default()
        }
    }

    // ─────────────── FormatOptions defaults ───────────────

    #[test]
    fn format_options_default_enabled_llm_disabled() {
        let opts = FormatOptions::default();
        assert!(opts.enabled);
        assert!(!opts.llm_enabled);
    }

    #[test]
    fn format_options_serde_round_trip() {
        let opts = FormatOptions {
            enabled: false,
            llm_enabled: true,
        };
        let toml_str = toml::to_string(&opts).expect("serialize");
        let restored: FormatOptions = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(opts, restored);
    }

    #[test]
    fn format_options_deserialize_empty_uses_defaults() {
        let opts: FormatOptions = toml::from_str("").expect("deserialize empty");
        assert_eq!(opts, FormatOptions::default());
    }

    // ─────────────── Disabled / identity ───────────────

    #[test]
    fn disabled_returns_joined_text() {
        let segs = vec![
            Segment::new(0.0, 1.0, "hello"),
            Segment::new(1.0, 2.0, "world"),
        ];
        let result = basic_format_segments(&segs, &disabled_opts());
        assert_eq!(result, "hello world");
    }

    #[test]
    fn empty_segments_returns_empty() {
        assert_eq!(basic_format_segments(&[], &default_opts()), "");
    }

    // ─────────────── Timestamp-gap paragraph breaks ───────────────

    #[test]
    fn no_paragraph_break_for_small_gap() {
        let segs = vec![
            Segment::new(0.0, 1.0, "Hello there."),
            Segment::new(1.5, 2.5, "General Kenobi."),
        ];
        let result = basic_format_segments(&segs, &default_opts());
        assert!(
            !result.contains("\n\n"),
            "gap of 0.5s should not create paragraph break, got:\n{result}"
        );
    }

    #[test]
    fn paragraph_break_for_large_gap() {
        let segs = vec![
            Segment::new(0.0, 1.0, "First paragraph."),
            Segment::new(3.0, 4.0, "Second paragraph."),
        ];
        let result = basic_format_segments(&segs, &default_opts());
        assert!(
            result.contains("\n\n"),
            "gap of 2.0s should create paragraph break, got:\n{result}"
        );
        assert_eq!(result, "First paragraph.\n\nSecond paragraph.");
    }

    #[test]
    fn paragraph_break_exactly_at_threshold() {
        let segs = vec![
            Segment::new(0.0, 1.0, "First."),
            Segment::new(2.5, 3.5, "Second."),
        ];
        let result = basic_format_segments(&segs, &default_opts());
        assert!(
            !result.contains("\n\n"),
            "gap of exactly 1.5s should not break, got:\n{result}"
        );
    }

    #[test]
    fn paragraph_break_just_above_threshold() {
        let segs = vec![
            Segment::new(0.0, 1.0, "First."),
            Segment::new(2.51, 3.5, "Second."),
        ];
        let result = basic_format_segments(&segs, &default_opts());
        assert!(
            result.contains("\n\n"),
            "gap of 1.51s should break, got:\n{result}"
        );
    }

    #[test]
    fn multiple_paragraph_breaks() {
        let segs = vec![
            Segment::new(0.0, 1.0, "Paragraph one."),
            Segment::new(3.0, 4.0, "Paragraph two."),
            Segment::new(6.0, 7.0, "Paragraph three."),
        ];
        let result = basic_format_segments(&segs, &default_opts());
        let paragraphs: Vec<&str> = result.split("\n\n").collect();
        assert_eq!(paragraphs.len(), 3, "expected 3 paragraphs, got:\n{result}");
    }

    #[test]
    fn mixed_gaps() {
        let segs = vec![
            Segment::new(0.0, 1.0, "Sentence one."),
            Segment::new(1.2, 2.0, "Sentence two."),
            Segment::new(4.0, 5.0, "New paragraph."),
            Segment::new(5.5, 6.0, "Still same paragraph."),
        ];
        let result = basic_format_segments(&segs, &default_opts());
        assert!(result.contains("Sentence one. Sentence two."));
        assert!(result.contains("\n\nNew paragraph. Still same paragraph."));
    }

    // ─────────────── Whitespace normalization ───────────────

    #[test]
    fn collapse_multiple_spaces_in_segment() {
        let segs = vec![Segment::new(0.0, 1.0, "hello   world")];
        let result = basic_format_segments(&segs, &default_opts());
        assert_eq!(result, "Hello world");
    }

    #[test]
    fn trim_leading_trailing_in_segment() {
        let segs = vec![Segment::new(0.0, 1.0, "  hello  ")];
        let result = basic_format_segments(&segs, &default_opts());
        assert_eq!(result, "Hello");
    }

    // ─────────────── Sentence capitalization ───────────────

    #[test]
    fn capitalize_start_of_text() {
        let segs = vec![Segment::new(0.0, 1.0, "hello world.")];
        let result = basic_format_segments(&segs, &default_opts());
        assert!(result.starts_with('H'));
    }

    #[test]
    fn capitalize_after_period() {
        let segs = vec![Segment::new(0.0, 1.0, "hello. world.")];
        let result = basic_format_segments(&segs, &default_opts());
        assert_eq!(result, "Hello. World.");
    }

    #[test]
    fn capitalize_after_paragraph_break() {
        let segs = vec![
            Segment::new(0.0, 1.0, "first."),
            Segment::new(3.0, 4.0, "second."),
        ];
        let result = basic_format_segments(&segs, &default_opts());
        assert!(result.contains("First."));
        assert!(result.contains("Second."));
    }

    #[test]
    fn already_capitalized_unchanged() {
        let segs = vec![Segment::new(0.0, 1.0, "Hello. World.")];
        let result = basic_format_segments(&segs, &default_opts());
        assert_eq!(result, "Hello. World.");
    }

    // ─────────────── Cleanup ───────────────

    #[test]
    fn remove_space_before_period() {
        let segs = vec![Segment::new(0.0, 1.0, "hello .")];
        let result = basic_format_segments(&segs, &default_opts());
        assert_eq!(result, "Hello.");
    }

    #[test]
    fn remove_space_before_comma() {
        let segs = vec![Segment::new(0.0, 1.0, "hello , world")];
        let result = basic_format_segments(&segs, &default_opts());
        assert_eq!(result, "Hello, world");
    }

    // ─────────────── Integration ───────────────

    #[test]
    fn full_dictation_with_pauses() {
        let segs = vec![
            Segment::new(0.0, 2.0, "I had a really productive day today."),
            Segment::new(2.2, 4.0, "I managed to get a lot of things done."),
            Segment::new(6.0, 8.0, "First I went to the store."),
            Segment::new(8.5, 10.0, "Then I cleaned the apartment."),
            Segment::new(12.0, 14.0, "Overall it was a great day."),
        ];
        let result = basic_format_segments(&segs, &default_opts());

        assert!(result.contains(
            "I had a really productive day today. I managed to get a lot of things done."
        ));
        assert!(result.contains("\n\nFirst I went to the store."));
        assert!(result.contains("store. Then I cleaned"));
        assert!(result.contains("\n\nOverall it was a great day."));
    }

    #[test]
    fn single_segment_formatted() {
        let segs = vec![Segment::new(0.0, 1.0, "hello world")];
        let result = basic_format_segments(&segs, &default_opts());
        assert_eq!(result, "Hello world");
    }

    #[test]
    fn empty_text_segments_skipped() {
        let segs = vec![
            Segment::new(0.0, 1.0, "Hello."),
            Segment::new(1.0, 2.0, "  "),
            Segment::new(2.0, 3.0, "World."),
        ];
        let result = basic_format_segments(&segs, &default_opts());
        assert_eq!(result, "Hello. World.");
    }
}
