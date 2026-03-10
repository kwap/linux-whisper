//! Export transcript data to various text-based formats.
//!
//! Supported formats:
//! - **TXT**: Plain text, one segment per line.
//! - **SRT**: SubRip subtitle format.
//! - **VTT**: WebVTT subtitle format.
//! - **CSV**: Comma-separated values with proper quoting.

use crate::model::Transcript;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Supported export formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    /// Plain text (one segment per line).
    Txt,
    /// SubRip subtitle format.
    Srt,
    /// WebVTT subtitle format.
    Vtt,
    /// Comma-separated values.
    Csv,
}

/// Errors that can occur during export.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    /// A formatting or data error.
    #[error("Format error: {0}")]
    FormatError(String),

    /// An I/O error (e.g. when writing to a file).
    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Export a [`Transcript`] to the requested [`ExportFormat`].
///
/// Returns the formatted string on success.
pub fn export(transcript: &Transcript, format: ExportFormat) -> Result<String, ExportError> {
    match format {
        ExportFormat::Txt => export_txt(transcript),
        ExportFormat::Srt => export_srt(transcript),
        ExportFormat::Vtt => export_vtt(transcript),
        ExportFormat::Csv => export_csv(transcript),
    }
}

// ---------------------------------------------------------------------------
// Format-specific implementations (private)
// ---------------------------------------------------------------------------

/// Export as plain text: one segment's text per line.
fn export_txt(transcript: &Transcript) -> Result<String, ExportError> {
    let lines: Vec<&str> = transcript
        .segments
        .iter()
        .map(|s| s.text.as_str())
        .collect();
    Ok(lines.join("\n"))
}

/// Export as SubRip (.srt) subtitles.
///
/// ```text
/// 1
/// 00:00:00,000 --> 00:00:03,200
/// Hello, this is a test.
///
/// 2
/// ...
/// ```
fn export_srt(transcript: &Transcript) -> Result<String, ExportError> {
    let mut out = String::new();

    for (i, seg) in transcript.segments.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        // Sequence number (1-based)
        out.push_str(&(i + 1).to_string());
        out.push('\n');

        // Timestamps
        out.push_str(&format_timestamp_srt(seg.start));
        out.push_str(" --> ");
        out.push_str(&format_timestamp_srt(seg.end));
        out.push('\n');

        // Text
        out.push_str(&seg.text);
        out.push('\n');
    }

    Ok(out)
}

/// Export as WebVTT (.vtt) subtitles.
///
/// ```text
/// WEBVTT
///
/// 00:00:00.000 --> 00:00:03.200
/// Hello, this is a test.
///
/// ...
/// ```
fn export_vtt(transcript: &Transcript) -> Result<String, ExportError> {
    let mut out = String::from("WEBVTT\n");

    for seg in &transcript.segments {
        out.push('\n');

        // Timestamps
        out.push_str(&format_timestamp_vtt(seg.start));
        out.push_str(" --> ");
        out.push_str(&format_timestamp_vtt(seg.end));
        out.push('\n');

        // Text
        out.push_str(&seg.text);
        out.push('\n');
    }

    Ok(out)
}

/// Export as CSV with a header row.
///
/// Text fields are always double-quoted, and any internal double-quotes are
/// escaped by doubling them (RFC 4180).
fn export_csv(transcript: &Transcript) -> Result<String, ExportError> {
    let mut out = String::from("start,end,text\n");

    for seg in &transcript.segments {
        // Escape double quotes inside the text by doubling them.
        let escaped = seg.text.replace('"', "\"\"");
        out.push_str(&format!(
            "{:.3},{:.3},\"{}\"\n",
            seg.start, seg.end, escaped
        ));
    }

    Ok(out)
}

// ---------------------------------------------------------------------------
// Timestamp helpers
// ---------------------------------------------------------------------------

/// Format seconds as `HH:MM:SS,mmm` (SRT-style, comma separator).
fn format_timestamp_srt(seconds: f64) -> String {
    let total_ms = (seconds * 1000.0).round() as u64;
    let ms = total_ms % 1000;
    let total_secs = total_ms / 1000;
    let s = total_secs % 60;
    let total_mins = total_secs / 60;
    let m = total_mins % 60;
    let h = total_mins / 60;
    format!("{:02}:{:02}:{:02},{:03}", h, m, s, ms)
}

/// Format seconds as `HH:MM:SS.mmm` (VTT-style, period separator).
fn format_timestamp_vtt(seconds: f64) -> String {
    let total_ms = (seconds * 1000.0).round() as u64;
    let ms = total_ms % 1000;
    let total_secs = total_ms / 1000;
    let s = total_secs % 60;
    let total_mins = total_secs / 60;
    let m = total_mins % 60;
    let h = total_mins / 60;
    format!("{:02}:{:02}:{:02}.{:03}", h, m, s, ms)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Segment, Transcript, TranscriptSource};

    // -- helpers ----------------------------------------------------------

    /// Build a small transcript with two segments for reuse across tests.
    fn sample_transcript() -> Transcript {
        let mut t = Transcript::new("Test Transcript", None, "base", TranscriptSource::Dictation);
        t.add_segment(Segment::new(0.0, 3.2, "Hello, this is a test."));
        t.add_segment(Segment::new(3.2, 6.1, "This is the second segment."));
        t
    }

    /// Build a transcript with three segments for broader coverage.
    fn three_segment_transcript() -> Transcript {
        let mut t = Transcript::new("Three Segments", None, "base", TranscriptSource::Dictation);
        t.add_segment(Segment::new(0.0, 1.5, "First."));
        t.add_segment(Segment::new(1.5, 4.0, "Second line here."));
        t.add_segment(Segment::new(4.0, 7.25, "Third and final."));
        t
    }

    fn empty_transcript() -> Transcript {
        Transcript::new("Empty", None, "base", TranscriptSource::Dictation)
    }

    // -- TXT tests --------------------------------------------------------

    #[test]
    fn txt_basic() {
        let t = sample_transcript();
        let result = export(&t, ExportFormat::Txt).unwrap();
        assert_eq!(
            result,
            "Hello, this is a test.\nThis is the second segment."
        );
    }

    #[test]
    fn txt_three_segments() {
        let t = three_segment_transcript();
        let result = export(&t, ExportFormat::Txt).unwrap();
        assert_eq!(result, "First.\nSecond line here.\nThird and final.");
    }

    #[test]
    fn txt_empty() {
        let t = empty_transcript();
        let result = export(&t, ExportFormat::Txt).unwrap();
        assert_eq!(result, "");
    }

    // -- SRT tests --------------------------------------------------------

    #[test]
    fn srt_basic() {
        let t = sample_transcript();
        let result = export(&t, ExportFormat::Srt).unwrap();
        let expected = "\
1
00:00:00,000 --> 00:00:03,200
Hello, this is a test.

2
00:00:03,200 --> 00:00:06,100
This is the second segment.
";
        assert_eq!(result, expected);
    }

    #[test]
    fn srt_numbering_three_segments() {
        let t = three_segment_transcript();
        let result = export(&t, ExportFormat::Srt).unwrap();
        // Verify that numbering is sequential 1, 2, 3
        assert!(result.starts_with("1\n"));
        assert!(result.contains("\n2\n"));
        assert!(result.contains("\n3\n"));
        // Should NOT contain a "4"
        assert!(!result.contains("\n4\n"));
    }

    #[test]
    fn srt_empty() {
        let t = empty_transcript();
        let result = export(&t, ExportFormat::Srt).unwrap();
        assert_eq!(result, "");
    }

    // -- VTT tests --------------------------------------------------------

    #[test]
    fn vtt_basic() {
        let t = sample_transcript();
        let result = export(&t, ExportFormat::Vtt).unwrap();
        let expected = "\
WEBVTT

00:00:00.000 --> 00:00:03.200
Hello, this is a test.

00:00:03.200 --> 00:00:06.100
This is the second segment.
";
        assert_eq!(result, expected);
    }

    #[test]
    fn vtt_starts_with_header() {
        let t = sample_transcript();
        let result = export(&t, ExportFormat::Vtt).unwrap();
        assert!(result.starts_with("WEBVTT\n"));
    }

    #[test]
    fn vtt_empty() {
        let t = empty_transcript();
        let result = export(&t, ExportFormat::Vtt).unwrap();
        assert_eq!(result, "WEBVTT\n");
    }

    // -- CSV tests --------------------------------------------------------

    #[test]
    fn csv_basic() {
        let t = sample_transcript();
        let result = export(&t, ExportFormat::Csv).unwrap();
        let expected = "\
start,end,text
0.000,3.200,\"Hello, this is a test.\"
3.200,6.100,\"This is the second segment.\"
";
        assert_eq!(result, expected);
    }

    #[test]
    fn csv_escaping_double_quotes() {
        let mut t = Transcript::new("Quotes", None, "base", TranscriptSource::Dictation);
        t.add_segment(Segment::new(0.0, 1.0, "She said \"hello\" to me."));
        let result = export(&t, ExportFormat::Csv).unwrap();
        assert_eq!(
            result,
            "start,end,text\n0.000,1.000,\"She said \"\"hello\"\" to me.\"\n"
        );
    }

    #[test]
    fn csv_empty() {
        let t = empty_transcript();
        let result = export(&t, ExportFormat::Csv).unwrap();
        assert_eq!(result, "start,end,text\n");
    }

    #[test]
    fn csv_multiple_quotes() {
        let mut t = Transcript::new("Multi-Quotes", None, "base", TranscriptSource::Dictation);
        t.add_segment(Segment::new(
            0.0,
            2.0,
            "He said \"hi\" and she said \"bye\"",
        ));
        let result = export(&t, ExportFormat::Csv).unwrap();
        assert!(result.contains("\"He said \"\"hi\"\" and she said \"\"bye\"\"\""));
    }

    // -- Timestamp formatting tests ---------------------------------------

    #[test]
    fn timestamp_srt_zero() {
        assert_eq!(format_timestamp_srt(0.0), "00:00:00,000");
    }

    #[test]
    fn timestamp_vtt_zero() {
        assert_eq!(format_timestamp_vtt(0.0), "00:00:00.000");
    }

    #[test]
    fn timestamp_srt_just_under_minute() {
        // 59.999 seconds
        assert_eq!(format_timestamp_srt(59.999), "00:00:59,999");
    }

    #[test]
    fn timestamp_vtt_just_under_minute() {
        assert_eq!(format_timestamp_vtt(59.999), "00:00:59.999");
    }

    #[test]
    fn timestamp_srt_over_one_hour() {
        // 3661.5 = 1 hour, 1 minute, 1.5 seconds
        assert_eq!(format_timestamp_srt(3661.5), "01:01:01,500");
    }

    #[test]
    fn timestamp_vtt_over_one_hour() {
        assert_eq!(format_timestamp_vtt(3661.5), "01:01:01.500");
    }

    #[test]
    fn timestamp_srt_exact_minute() {
        assert_eq!(format_timestamp_srt(60.0), "00:01:00,000");
    }

    #[test]
    fn timestamp_vtt_exact_minute() {
        assert_eq!(format_timestamp_vtt(60.0), "00:01:00.000");
    }

    #[test]
    fn timestamp_srt_large_value() {
        // 10 hours exactly
        assert_eq!(format_timestamp_srt(36000.0), "10:00:00,000");
    }

    #[test]
    fn timestamp_vtt_fractional_rounding() {
        // 1.9999 should round to 2.000
        assert_eq!(format_timestamp_vtt(1.9999), "00:00:02.000");
    }

    #[test]
    fn timestamp_srt_separator_is_comma() {
        let ts = format_timestamp_srt(1.234);
        assert!(ts.contains(','), "SRT timestamp must use comma: {}", ts);
        assert!(
            !ts.contains('.'),
            "SRT timestamp must not use period: {}",
            ts
        );
    }

    #[test]
    fn timestamp_vtt_separator_is_period() {
        let ts = format_timestamp_vtt(1.234);
        // VTT has periods in the fractional part and colons in the time part.
        // Make sure there is no comma.
        assert!(
            !ts.contains(','),
            "VTT timestamp must not use comma: {}",
            ts
        );
    }

    // -- ExportFormat enum tests ------------------------------------------

    #[test]
    fn export_format_debug() {
        // Ensure Debug derive works.
        let dbg = format!("{:?}", ExportFormat::Txt);
        assert_eq!(dbg, "Txt");
    }

    #[test]
    fn export_format_clone_copy_eq() {
        let a = ExportFormat::Srt;
        let b = a; // Copy
        let c = a.clone(); // Clone
        assert_eq!(a, b);
        assert_eq!(a, c);
        assert_ne!(a, ExportFormat::Vtt);
    }

    // -- ExportError tests ------------------------------------------------

    #[test]
    fn export_error_format_display() {
        let err = ExportError::FormatError("bad data".to_string());
        assert_eq!(err.to_string(), "Format error: bad data");
    }

    #[test]
    fn export_error_io_from() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let err: ExportError = io_err.into();
        assert!(err.to_string().contains("file missing"));
    }
}
