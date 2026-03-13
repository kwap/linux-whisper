use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Represents the source of a transcription.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum TranscriptSource {
    /// Live dictation via microphone.
    Dictation,
    /// Transcription from an audio file on disk.
    File {
        /// Path to the source audio file.
        path: String,
    },
}

/// A single segment of transcribed speech, typically corresponding to one
/// utterance or sentence detected by the whisper model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Segment {
    /// Unique identifier for this segment.
    pub id: Uuid,
    /// Start time in seconds from the beginning of the audio.
    pub start: f64,
    /// End time in seconds from the beginning of the audio.
    pub end: f64,
    /// The transcribed text for this segment.
    pub text: String,
    /// Optional confidence score in the range 0.0 to 1.0, where 1.0 indicates
    /// highest confidence. Not all models or configurations produce this value.
    pub confidence: Option<f32>,
}

impl Segment {
    /// Creates a new segment with a generated UUID and no confidence score.
    ///
    /// # Arguments
    ///
    /// * `start` - Start time in seconds.
    /// * `end` - End time in seconds.
    /// * `text` - The transcribed text content.
    pub fn new(start: f64, end: f64, text: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            start,
            end,
            text: text.into(),
            confidence: None,
        }
    }

    /// Returns the duration of this segment in seconds.
    pub fn duration(&self) -> f64 {
        self.end - self.start
    }
}

/// A complete transcription result, containing metadata and an ordered list of
/// segments produced by a whisper model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcript {
    /// Unique identifier for this transcript.
    pub id: Uuid,
    /// Human-readable title for the transcript (e.g. filename or session name).
    pub title: String,
    /// Ordered list of transcribed segments.
    pub segments: Vec<Segment>,
    /// Optional ISO 639-1 language code (e.g. "en", "de", "ja").
    pub language: Option<String>,
    /// Name of the whisper model used for transcription (e.g. "base", "large-v3").
    pub model_name: String,
    /// Timestamp when this transcript was created.
    pub created_at: DateTime<Utc>,
    /// Total duration of the source audio in seconds.
    pub duration: f64,
    /// Where the audio originated from.
    pub source: TranscriptSource,
}

impl Transcript {
    /// Creates a new transcript with an empty segment list and the current UTC
    /// timestamp. The duration is initialised to zero and should be updated once
    /// the full audio length is known.
    ///
    /// # Arguments
    ///
    /// * `title` - Human-readable title for this transcript.
    /// * `language` - Optional ISO 639-1 language code.
    /// * `model_name` - Name of the whisper model used.
    /// * `source` - Where the audio came from.
    pub fn new(
        title: impl Into<String>,
        language: Option<String>,
        model_name: impl Into<String>,
        source: TranscriptSource,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            title: title.into(),
            segments: Vec::new(),
            language,
            model_name: model_name.into(),
            created_at: Utc::now(),
            duration: 0.0,
            source,
        }
    }

    /// Joins all segment texts with a single space to produce the complete
    /// transcription as a single string.
    pub fn full_text(&self) -> String {
        self.segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Returns the full text with basic formatting applied.
    pub fn formatted_text(&self, opts: &crate::format::FormatOptions) -> String {
        crate::format::basic_format_segments(&self.segments, opts)
    }

    /// Appends a segment to this transcript.
    pub fn add_segment(&mut self, segment: Segment) {
        self.segments.push(segment);
    }

    /// Returns the number of segments in this transcript.
    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ───────────────────────── Segment tests ─────────────────────────

    #[test]
    fn segment_new_creates_valid_segment() {
        let seg = Segment::new(1.0, 3.5, "hello world");

        assert!(!seg.id.is_nil());
        assert_eq!(seg.start, 1.0);
        assert_eq!(seg.end, 3.5);
        assert_eq!(seg.text, "hello world");
        assert_eq!(seg.confidence, None);
    }

    #[test]
    fn segment_duration_is_end_minus_start() {
        let seg = Segment::new(2.0, 5.5, "test");
        let dur = seg.duration();
        assert!((dur - 3.5).abs() < f64::EPSILON);
    }

    #[test]
    fn segment_duration_zero_length() {
        let seg = Segment::new(4.0, 4.0, "");
        assert!((seg.duration() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn segment_with_confidence() {
        let mut seg = Segment::new(0.0, 1.0, "confident");
        seg.confidence = Some(0.95);
        assert_eq!(seg.confidence, Some(0.95));
    }

    #[test]
    fn segment_clone_is_equal() {
        let seg = Segment::new(0.0, 1.0, "clone me");
        let cloned = seg.clone();
        assert_eq!(seg, cloned);
    }

    // ────────────────────── TranscriptSource tests ──────────────────────

    #[test]
    fn transcript_source_dictation_variant() {
        let src = TranscriptSource::Dictation;
        assert_eq!(src, TranscriptSource::Dictation);
    }

    #[test]
    fn transcript_source_file_variant() {
        let src = TranscriptSource::File {
            path: "/tmp/audio.wav".to_string(),
        };
        match &src {
            TranscriptSource::File { path } => assert_eq!(path, "/tmp/audio.wav"),
            _ => panic!("expected File variant"),
        }
    }

    #[test]
    fn transcript_source_file_equality() {
        let a = TranscriptSource::File {
            path: "/a.wav".to_string(),
        };
        let b = TranscriptSource::File {
            path: "/a.wav".to_string(),
        };
        let c = TranscriptSource::File {
            path: "/b.wav".to_string(),
        };
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn transcript_source_dictation_ne_file() {
        let dictation = TranscriptSource::Dictation;
        let file = TranscriptSource::File {
            path: "/x.wav".to_string(),
        };
        assert_ne!(dictation, file);
    }

    // ───────────────────── Transcript tests ─────────────────────

    #[test]
    fn transcript_new_has_empty_segments() {
        let t = Transcript::new(
            "Meeting",
            Some("en".into()),
            "base",
            TranscriptSource::Dictation,
        );

        assert!(!t.id.is_nil());
        assert_eq!(t.title, "Meeting");
        assert_eq!(t.language, Some("en".to_string()));
        assert_eq!(t.model_name, "base");
        assert!(t.segments.is_empty());
        assert_eq!(t.segment_count(), 0);
        assert_eq!(t.duration, 0.0);
        assert_eq!(t.source, TranscriptSource::Dictation);
    }

    #[test]
    fn transcript_new_without_language() {
        let t = Transcript::new("Notes", None, "large-v3", TranscriptSource::Dictation);
        assert_eq!(t.language, None);
    }

    #[test]
    fn transcript_new_with_file_source() {
        let src = TranscriptSource::File {
            path: "/home/user/recording.wav".to_string(),
        };
        let t = Transcript::new("Recording", Some("de".into()), "medium", src.clone());
        assert_eq!(t.source, src);
    }

    #[test]
    fn transcript_created_at_is_recent() {
        let before = Utc::now();
        let t = Transcript::new("Test", None, "tiny", TranscriptSource::Dictation);
        let after = Utc::now();

        assert!(t.created_at >= before);
        assert!(t.created_at <= after);
    }

    #[test]
    fn transcript_add_segment_increments_count() {
        let mut t = Transcript::new("Test", None, "base", TranscriptSource::Dictation);
        assert_eq!(t.segment_count(), 0);

        t.add_segment(Segment::new(0.0, 1.0, "first"));
        assert_eq!(t.segment_count(), 1);

        t.add_segment(Segment::new(1.0, 2.0, "second"));
        assert_eq!(t.segment_count(), 2);
    }

    #[test]
    fn transcript_full_text_empty() {
        let t = Transcript::new("Empty", None, "base", TranscriptSource::Dictation);
        assert_eq!(t.full_text(), "");
    }

    #[test]
    fn transcript_full_text_single_segment() {
        let mut t = Transcript::new("Single", None, "base", TranscriptSource::Dictation);
        t.add_segment(Segment::new(0.0, 1.0, "hello"));
        assert_eq!(t.full_text(), "hello");
    }

    #[test]
    fn transcript_full_text_multiple_segments() {
        let mut t = Transcript::new("Multi", None, "base", TranscriptSource::Dictation);
        t.add_segment(Segment::new(0.0, 1.0, "The quick"));
        t.add_segment(Segment::new(1.0, 2.0, "brown fox"));
        t.add_segment(Segment::new(2.0, 3.0, "jumps over the lazy dog."));
        assert_eq!(
            t.full_text(),
            "The quick brown fox jumps over the lazy dog."
        );
    }

    #[test]
    fn transcript_segment_count_matches_segments_len() {
        let mut t = Transcript::new("Count", None, "base", TranscriptSource::Dictation);
        for i in 0..5 {
            t.add_segment(Segment::new(i as f64, (i + 1) as f64, format!("seg {i}")));
        }
        assert_eq!(t.segment_count(), 5);
        assert_eq!(t.segment_count(), t.segments.len());
    }

    // ─────────────── Serialization round-trip tests ───────────────

    #[test]
    fn segment_serialize_deserialize_roundtrip() {
        let mut original = Segment::new(1.5, 4.25, "round trip");
        original.confidence = Some(0.87);

        let serialized = toml::to_string(&original).expect("failed to serialize segment");
        let deserialized: Segment =
            toml::from_str(&serialized).expect("failed to deserialize segment");

        assert_eq!(original, deserialized);
    }

    #[test]
    fn segment_serialize_without_confidence() {
        let original = Segment::new(0.0, 1.0, "no confidence");

        let serialized = toml::to_string(&original).expect("failed to serialize");
        let deserialized: Segment = toml::from_str(&serialized).expect("failed to deserialize");

        assert_eq!(original, deserialized);
        assert_eq!(deserialized.confidence, None);
    }

    #[test]
    fn transcript_source_dictation_roundtrip() {
        let original = TranscriptSource::Dictation;
        let serialized = toml::to_string(&original).expect("failed to serialize");
        let deserialized: TranscriptSource =
            toml::from_str(&serialized).expect("failed to deserialize");
        assert_eq!(original, deserialized);
    }

    #[test]
    fn transcript_source_file_roundtrip() {
        let original = TranscriptSource::File {
            path: "/data/meeting.ogg".to_string(),
        };
        let serialized = toml::to_string(&original).expect("failed to serialize");
        let deserialized: TranscriptSource =
            toml::from_str(&serialized).expect("failed to deserialize");
        assert_eq!(original, deserialized);
    }

    #[test]
    fn transcript_serialize_deserialize_roundtrip() {
        let mut t = Transcript::new(
            "Roundtrip Test",
            Some("en".into()),
            "small.en",
            TranscriptSource::File {
                path: "/audio/test.wav".to_string(),
            },
        );
        t.duration = 42.5;
        t.add_segment(Segment::new(0.0, 2.0, "Hello there."));
        t.add_segment(Segment::new(2.0, 4.0, "General Kenobi."));

        let serialized = toml::to_string(&t).expect("failed to serialize transcript");
        let deserialized: Transcript =
            toml::from_str(&serialized).expect("failed to deserialize transcript");

        assert_eq!(deserialized.id, t.id);
        assert_eq!(deserialized.title, t.title);
        assert_eq!(deserialized.language, t.language);
        assert_eq!(deserialized.model_name, t.model_name);
        assert_eq!(deserialized.created_at, t.created_at);
        assert!((deserialized.duration - t.duration).abs() < f64::EPSILON);
        assert_eq!(deserialized.source, t.source);
        assert_eq!(deserialized.segment_count(), 2);
        assert_eq!(deserialized.segments, t.segments);
    }

    #[test]
    fn transcript_full_text_after_roundtrip() {
        let mut t = Transcript::new("Text RT", None, "base", TranscriptSource::Dictation);
        t.add_segment(Segment::new(0.0, 1.0, "alpha"));
        t.add_segment(Segment::new(1.0, 2.0, "beta"));

        let serialized = toml::to_string(&t).expect("serialize");
        let deserialized: Transcript = toml::from_str(&serialized).expect("deserialize");

        assert_eq!(deserialized.full_text(), "alpha beta");
    }

    #[test]
    fn formatted_text_with_defaults() {
        let mut t = Transcript::new("Format", None, "base", TranscriptSource::Dictation);
        t.add_segment(Segment::new(0.0, 2.0, "hello. world."));
        let opts = crate::format::FormatOptions::default();
        let formatted = t.formatted_text(&opts);
        assert_eq!(formatted, "Hello. World.");
    }

    #[test]
    fn formatted_text_disabled_returns_raw() {
        let mut t = Transcript::new("Raw", None, "base", TranscriptSource::Dictation);
        t.add_segment(Segment::new(0.0, 1.0, "hello. world."));
        let opts = crate::format::FormatOptions {
            enabled: false,
            ..Default::default()
        };
        assert_eq!(t.formatted_text(&opts), t.full_text());
    }

    #[test]
    fn formatted_text_preserves_full_text() {
        let mut t = Transcript::new("Preserve", None, "base", TranscriptSource::Dictation);
        t.add_segment(Segment::new(0.0, 1.0, "hello"));
        t.add_segment(Segment::new(1.0, 2.0, "world"));
        let opts = crate::format::FormatOptions::default();
        let _ = t.formatted_text(&opts);
        assert_eq!(t.full_text(), "hello world");
    }

    #[test]
    fn formatted_text_uses_timestamp_gaps() {
        let mut t = Transcript::new("Gaps", None, "base", TranscriptSource::Dictation);
        t.add_segment(Segment::new(0.0, 1.0, "first part."));
        t.add_segment(Segment::new(3.0, 4.0, "second part."));
        let opts = crate::format::FormatOptions::default();
        let formatted = t.formatted_text(&opts);
        assert!(formatted.contains("\n\n"));
    }
}
