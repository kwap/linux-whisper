use crate::model::{Segment, Transcript};

/// A single search hit within a transcript segment.
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// The segment whose text matched the query (cloned).
    pub segment: Segment,
    /// The title of the transcript that contains the matching segment.
    pub transcript_title: String,
}

/// Search a single transcript for segments whose text contains `query`
/// (case-insensitive). Returns an empty `Vec` when `query` is empty.
pub fn search_transcript(transcript: &Transcript, query: &str) -> Vec<SearchResult> {
    if query.is_empty() {
        return Vec::new();
    }

    let query_lower = query.to_lowercase();

    transcript
        .segments
        .iter()
        .filter(|seg| seg.text.to_lowercase().contains(&query_lower))
        .map(|seg| SearchResult {
            segment: seg.clone(),
            transcript_title: transcript.title.clone(),
        })
        .collect()
}

/// Search across multiple transcripts, returning results from every transcript
/// that contains a matching segment.
pub fn search_transcripts(transcripts: &[Transcript], query: &str) -> Vec<SearchResult> {
    transcripts
        .iter()
        .flat_map(|t| search_transcript(t, query))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Segment, Transcript, TranscriptSource};

    /// Helper: build a segment with the given text and reasonable defaults.
    fn make_segment(text: &str) -> Segment {
        Segment::new(0.0, 1.0, text)
    }

    /// Helper: build a transcript with an explicit title and a list of segments.
    fn make_transcript(title: &str, segments: Vec<Segment>) -> Transcript {
        let mut t = Transcript::new(title, None, "base", TranscriptSource::Dictation);
        for seg in segments {
            t.add_segment(seg);
        }
        t
    }

    #[test]
    fn search_finds_matching_segments_case_insensitive() {
        let transcript = make_transcript(
            "Meeting Notes",
            vec![
                make_segment("Hello world"),
                make_segment("goodbye world"),
                make_segment("no match here"),
            ],
        );

        let results = search_transcript(&transcript, "HELLO");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].segment.text, "Hello world");
        assert_eq!(results[0].transcript_title, "Meeting Notes");
    }

    #[test]
    fn search_returns_empty_for_no_match() {
        let transcript = make_transcript("Demo", vec![make_segment("alpha"), make_segment("beta")]);

        let results = search_transcript(&transcript, "gamma");
        assert!(results.is_empty());
    }

    #[test]
    fn search_returns_empty_for_empty_query() {
        let transcript = make_transcript("Demo", vec![make_segment("some text")]);

        let results = search_transcript(&transcript, "");
        assert!(results.is_empty());
    }

    #[test]
    fn search_across_multiple_transcripts() {
        let t1 = make_transcript("First", vec![make_segment("Rust is great")]);
        let t2 = make_transcript("Second", vec![make_segment("Python is popular")]);
        let t3 = make_transcript("Third", vec![make_segment("Rust and C")]);

        let results = search_transcripts(&[t1, t2, t3], "rust");
        assert_eq!(results.len(), 2);

        let titles: Vec<&str> = results
            .iter()
            .map(|r| r.transcript_title.as_str())
            .collect();
        assert!(titles.contains(&"First"));
        assert!(titles.contains(&"Third"));
    }

    #[test]
    fn search_finds_partial_word_matches() {
        let transcript = make_transcript("Lecture", vec![make_segment("understanding compilers")]);

        let results = search_transcript(&transcript, "compil");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].segment.text, "understanding compilers");
    }

    #[test]
    fn multiple_segments_in_same_transcript_can_match() {
        let transcript = make_transcript(
            "Interview",
            vec![
                make_segment("I love Rust programming"),
                make_segment("Rust has great tooling"),
                make_segment("Java is also fine"),
            ],
        );

        let results = search_transcript(&transcript, "rust");
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.transcript_title == "Interview"));
    }
}
