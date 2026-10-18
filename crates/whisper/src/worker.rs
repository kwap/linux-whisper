//! Worker thread for asynchronous whisper inference.
//!
//! [`WhisperWorker`] spawns a dedicated OS thread for running synchronous
//! whisper-rs inference and exposes an async interface via tokio channels.
//! This keeps the tokio runtime free from blocking compute work.

use std::ops::Range;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use linux_whisper_audio::capture::AudioBuffer;
use linux_whisper_core::model::{Segment, Transcript, TranscriptSource};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, error, info};
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
};

use crate::engine::{TranscribeError, TranscribeOptions};

// ---------------------------------------------------------------------------
// WorkerCommand
// ---------------------------------------------------------------------------

/// Commands sent from the async world to the worker thread.
pub enum WorkerCommand {
    /// Request transcription of an audio buffer.
    Transcribe {
        /// The audio to transcribe.
        audio: AudioBuffer,
        /// Transcription options (language, translate flag, etc.).
        options: TranscribeOptions,
        /// Channel to send the result back on.
        reply: oneshot::Sender<Result<Transcript, TranscribeError>>,
    },
    /// Load a whisper model from the given path.
    LoadModel {
        /// Path to the GGML model file.
        path: PathBuf,
        /// Channel to send the result back on.
        reply: oneshot::Sender<Result<(), TranscribeError>>,
    },
    /// Gracefully shut down the worker thread.
    Shutdown,
}

// ---------------------------------------------------------------------------
// WhisperWorker
// ---------------------------------------------------------------------------

/// Async handle to a background worker thread that performs whisper inference.
///
/// Commands are sent over an MPSC channel and processed sequentially on a
/// dedicated `std::thread`. Results are returned via oneshot channels.
#[derive(Clone)]
pub struct WhisperWorker {
    sender: mpsc::Sender<WorkerCommand>,
    /// Name of the model currently loaded on the worker thread.
    loaded_model: Arc<Mutex<Option<String>>>,
}

impl Default for WhisperWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl WhisperWorker {
    /// Spawns a new worker thread and returns a handle for communicating with it.
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel::<WorkerCommand>(32);
        let loaded_model = Arc::new(Mutex::new(None));
        let loaded_for_thread = Arc::clone(&loaded_model);

        std::thread::Builder::new()
            .name("whisper-worker".to_string())
            .spawn(move || {
                Self::worker_loop(rx, loaded_for_thread);
            })
            .expect("failed to spawn whisper worker thread");

        info!("Whisper worker thread spawned");

        Self {
            sender: tx,
            loaded_model,
        }
    }

    /// Name of the model that is loaded and ready (e.g. "base"), if any.
    pub fn loaded_model(&self) -> Option<String> {
        self.loaded_model.lock().ok().and_then(|m| m.clone())
    }

    /// Sends an audio buffer to the worker for transcription and awaits the
    /// result.
    pub async fn transcribe(
        &self,
        audio: AudioBuffer,
        options: TranscribeOptions,
    ) -> Result<Transcript, TranscribeError> {
        let (tx, rx) = oneshot::channel();

        self.sender
            .send(WorkerCommand::Transcribe {
                audio,
                options,
                reply: tx,
            })
            .await
            .map_err(|_| {
                TranscribeError::TranscriptionFailed("worker channel closed".to_string())
            })?;

        rx.await.map_err(|_| {
            TranscribeError::TranscriptionFailed("worker dropped reply channel".to_string())
        })?
    }

    /// Sends a model-load command to the worker and awaits the result.
    pub async fn load_model(&self, path: PathBuf) -> Result<(), TranscribeError> {
        let (tx, rx) = oneshot::channel();

        self.sender
            .send(WorkerCommand::LoadModel { path, reply: tx })
            .await
            .map_err(|_| {
                TranscribeError::TranscriptionFailed("worker channel closed".to_string())
            })?;

        rx.await.map_err(|_| {
            TranscribeError::TranscriptionFailed("worker dropped reply channel".to_string())
        })?
    }

    /// Sends a shutdown command to the worker thread.
    ///
    /// The worker will finish processing the current command (if any) and then
    /// exit. This method does not block waiting for the thread to terminate.
    pub async fn shutdown(&self) {
        let _ = self.sender.send(WorkerCommand::Shutdown).await;
        debug!("Shutdown command sent to whisper worker");
    }

    // -----------------------------------------------------------------------
    // Internal worker loop
    // -----------------------------------------------------------------------

    /// The synchronous event loop that runs on the dedicated worker thread.
    ///
    /// Receives commands from the async side and processes them one at a time.
    fn worker_loop(
        mut rx: mpsc::Receiver<WorkerCommand>,
        loaded_model: Arc<Mutex<Option<String>>>,
    ) {
        // We need a small tokio runtime *only* to drive the mpsc::Receiver
        // (which is async). An alternative would be std::sync::mpsc, but we
        // use tokio::sync::mpsc for consistency with the rest of the project.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build worker-local tokio runtime");

        let mut ctx: Option<WhisperContext> = None;
        let mut model_name: String = String::new();

        rt.block_on(async {
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    WorkerCommand::Transcribe {
                        audio,
                        options,
                        reply,
                    } => {
                        debug!(
                            "Worker: transcribe request ({} samples, lang={:?})",
                            audio.samples.len(),
                            options.language,
                        );

                        let Some(ref whisper_ctx) = ctx else {
                            let _ = reply.send(Err(TranscribeError::ModelNotLoaded));
                            continue;
                        };

                        let result = run_transcription(whisper_ctx, &audio, &options, &model_name);
                        let _ = reply.send(result);
                    }

                    WorkerCommand::LoadModel { path, reply } => {
                        info!("Worker: loading model from {}", path.display());

                        if !path.exists() {
                            let _ = reply.send(Err(TranscribeError::TranscriptionFailed(format!(
                                "model file not found: {}",
                                path.display()
                            ))));
                            continue;
                        }

                        let params = WhisperContextParameters::default();
                        match WhisperContext::new_with_params(
                            path.to_str().unwrap_or_default(),
                            params,
                        ) {
                            Ok(new_ctx) => {
                                // Extract model name from filename.
                                model_name = path
                                    .file_stem()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or("unknown")
                                    .strip_prefix("ggml-")
                                    .unwrap_or("unknown")
                                    .to_string();

                                info!("Model loaded: {} ({})", model_name, path.display());
                                ctx = Some(new_ctx);
                                if let Ok(mut loaded) = loaded_model.lock() {
                                    *loaded = Some(model_name.clone());
                                }
                                let _ = reply.send(Ok(()));
                            }
                            Err(e) => {
                                error!("Failed to load whisper model: {e}");
                                let _ = reply.send(Err(TranscribeError::TranscriptionFailed(
                                    format!("whisper-rs model load failed: {e}"),
                                )));
                            }
                        }
                    }

                    WorkerCommand::Shutdown => {
                        info!("Worker: shutting down");
                        break;
                    }
                }
            }
        });

        debug!("Whisper worker thread exiting");
    }
}

/// Pauses at least this long separate the parts whose language is
/// identified separately.
const MIN_PAUSE_MS: usize = 500;

/// Parts shorter than this are joined to their neighbour: too little speech
/// to identify a language reliably.
const MIN_PART_MS: usize = 1500;

/// Recordings longer than this (files, mostly) are split into parts of up to
/// `LONG_AUDIO_PART_SECS` so identifying languages stays cheap.
const LONG_AUDIO_SECS: usize = 120;
const LONG_AUDIO_PART_SECS: usize = 30;

/// One detection over a recording that fits in whisper's 30 s window: above
/// this probability the recording is taken to be in one language.
const WHOLE_CLIP_SURE_PROBABILITY: f32 = 0.9;
const WINDOW_SECS: usize = 30;

/// A part's detected language is always trusted above this probability.
const SURE_LANGUAGE_PROBABILITY: f32 = 0.6;

/// Below `SURE_LANGUAGE_PROBABILITY`, a part keeps the previous part's
/// language while that language is at least this likely for it...
const PLAUSIBLE_PREVIOUS_PROBABILITY: f32 = 0.15;

/// ...and otherwise switches to its own detected language if that is at least
/// this likely.
const MIN_SWITCH_PROBABILITY: f32 = 0.3;

/// Run whisper-rs transcription synchronously. Called on the worker thread.
fn run_transcription(
    ctx: &WhisperContext,
    audio: &AudioBuffer,
    options: &TranscribeOptions,
    model_name: &str,
) -> Result<Transcript, TranscribeError> {
    // whisper.cpp defaults to at most 4 threads; more makes CPU-only
    // transcription noticeably faster on desktop machines.
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 8);
    let rate = audio.sample_rate as usize;

    // whisper.cpp ignores input shorter than one second, so a quick "yes"
    // would come back empty. Pad short clips with silence.
    let min_samples = rate * 11 / 10;
    let padded;
    let samples: &[f32] = if audio.samples.len() < min_samples {
        let mut buf = audio.samples.clone();
        buf.resize(min_samples, 0.0);
        padded = buf;
        &padded
    } else {
        &audio.samples
    };

    let mut state = ctx
        .create_state()
        .map_err(|e| TranscribeError::TranscriptionFailed(format!("create state: {e}")))?;

    // Decide which language to use for which part of the audio.
    let spans = match &options.language {
        Some(lang) => vec![(0..samples.len(), Some(lang.clone()))],
        None if ctx.is_multilingual() => language_spans(&mut state, samples, rate, threads),
        // English-only models: auto-detecting would only cost time and report
        // a meaningless language.
        None => vec![(0..samples.len(), Some("en".to_string()))],
    };

    let mut transcript = Transcript::new(
        "Transcription",
        options.language.clone(),
        model_name,
        TranscriptSource::Dictation,
    );
    transcript.duration = audio.samples.len() as f64 / rate as f64;

    // Segment text tokens have ids below end-of-text; timestamps and other
    // special tokens come after it.
    let eot = ctx.token_eot();
    let mut speech_per_language: Vec<(String, usize)> = Vec::new();

    for (range, language) in &spans {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(threads as i32);
        // `None` makes whisper.cpp detect the language itself.
        params.set_language(language.as_deref());
        params.set_translate(options.translate);
        params.set_suppress_nst(false);
        params.set_single_segment(false);
        params.set_no_context(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        state
            .full(params, &samples[range.clone()])
            .map_err(|e| TranscribeError::TranscriptionFailed(format!("inference failed: {e}")))?;

        let used_language = language.clone().or_else(|| {
            whisper_rs::get_lang_str(state.full_lang_id_from_state()).map(str::to_string)
        });
        if let Some(lang) = used_language {
            match speech_per_language.iter_mut().find(|(l, _)| *l == lang) {
                Some((_, len)) => *len += range.len(),
                None => speech_per_language.push((lang, range.len())),
            }
        }

        let offset = range.start as f64 / rate as f64;
        for i in 0..state.full_n_segments() {
            let Some(seg) = state.get_segment(i) else {
                continue;
            };

            let text = seg.to_str_lossy().map_err(|e| {
                TranscribeError::TranscriptionFailed(format!("failed to get segment {i} text: {e}"))
            })?;
            let cleaned = strip_non_speech(&text);
            if cleaned.is_empty() {
                continue;
            }

            // whisper.cpp timestamps are in centiseconds (10ms units).
            let start_sec = offset + seg.start_timestamp() as f64 / 100.0;
            let end_sec = offset + seg.end_timestamp() as f64 / 100.0;

            // Average probability of the segment's text tokens.
            let (sum, count) = (0..seg.n_tokens())
                .filter_map(|t| seg.get_token(t))
                .filter(|tok| tok.token_id() < eot)
                .fold((0.0f32, 0u32), |(sum, n), tok| {
                    (sum + tok.token_probability(), n + 1)
                });

            let mut segment = Segment::new(start_sec, end_sec, cleaned);
            segment.confidence = (count > 0).then(|| sum / count as f32);
            transcript.add_segment(segment);
        }
    }

    // Report the language spoken the most.
    if transcript.language.is_none() {
        transcript.language = speech_per_language
            .iter()
            .max_by_key(|(_, len)| *len)
            .map(|(lang, _)| lang.clone());
    }

    info!(
        "Whisper inference complete: {} segment(s), {:.1}s audio, language(s): {:?}",
        transcript.segment_count(),
        transcript.duration,
        speech_per_language
            .iter()
            .map(|(l, _)| l.as_str())
            .collect::<Vec<_>>(),
    );

    Ok(transcript)
}

/// Split the audio at pauses and identify the language of each part, so a
/// speaker can switch language from one sentence to the next. Consecutive
/// parts in the same language are joined, so speech in one language is still
/// transcribed in a single pass.
fn language_spans(
    state: &mut WhisperState,
    samples: &[f32],
    rate: usize,
    threads: usize,
) -> Vec<(Range<usize>, Option<String>)> {
    let whole = vec![(0..samples.len(), None)];

    // Most recordings are in one language. One detection tells, and then the
    // whole recording is transcribed in that language in a single pass — the
    // same cost as letting whisper.cpp detect it. Only an unsure result
    // (typical when two languages are mixed) is worth splitting at pauses.
    if samples.len() <= WINDOW_SECS * rate {
        if let Some((id, probabilities)) = detect_language(state, samples, threads) {
            let top = usize::try_from(id)
                .ok()
                .and_then(|i| probabilities.get(i).copied())
                .unwrap_or(0.0);
            if top >= WHOLE_CLIP_SURE_PROBABILITY {
                return vec![(
                    0..samples.len(),
                    whisper_rs::get_lang_str(id).map(str::to_string),
                )];
            }
        }
    }

    let max_merge = if samples.len() > LONG_AUDIO_SECS * rate {
        LONG_AUDIO_PART_SECS * rate
    } else {
        0
    };
    let parts = split_at_pauses(samples, rate, max_merge);
    if parts.len() < 2 {
        return whole;
    }

    let detections: Vec<Option<(i32, Vec<f32>)>> = parts
        .iter()
        .map(|part| detect_language(state, &samples[part.clone()], threads))
        .collect();

    let probability = |probs: &[f32], id: i32| {
        usize::try_from(id)
            .ok()
            .and_then(|i| probs.get(i).copied())
            .unwrap_or(0.0)
    };

    // Decide each part's language; unclear parts follow the part before them.
    let mut languages: Vec<Option<i32>> = Vec::with_capacity(parts.len());
    let mut previous: Option<i32> = None;
    for detection in &detections {
        let chosen = detection.as_ref().and_then(|(id, probs)| {
            let top = probability(probs, *id);
            match previous {
                _ if top >= SURE_LANGUAGE_PROBABILITY => Some(*id),
                Some(prev) if probability(probs, prev) >= PLAUSIBLE_PREVIOUS_PROBABILITY => {
                    Some(prev)
                }
                _ if top >= MIN_SWITCH_PROBABILITY => Some(*id),
                _ => previous,
            }
        });
        if chosen.is_some() {
            previous = chosen;
        }
        languages.push(chosen);
    }

    // Unclear parts before the first clear one take its language.
    let Some(first_known) = languages.iter().flatten().next().copied() else {
        return whole;
    };
    let mut previous = first_known;
    for lang in languages.iter_mut() {
        match lang {
            Some(l) => previous = *l,
            None => *lang = Some(previous),
        }
    }

    let mut spans: Vec<(Range<usize>, i32)> = Vec::new();
    for (part, lang) in parts.into_iter().zip(languages.into_iter().flatten()) {
        match spans.last_mut() {
            Some((range, last)) if *last == lang => range.end = part.end,
            _ => spans.push((part, lang)),
        }
    }

    debug!(
        "Language per part: {:?}",
        spans
            .iter()
            .map(|(r, l)| (r.start / rate, whisper_rs::get_lang_str(*l)))
            .collect::<Vec<_>>()
    );

    spans
        .into_iter()
        .map(|(range, lang)| (range, whisper_rs::get_lang_str(lang).map(str::to_string)))
        .collect()
}

/// Identify the most likely language of `samples`, with the probabilities of
/// all languages.
fn detect_language(
    state: &mut WhisperState,
    samples: &[f32],
    threads: usize,
) -> Option<(i32, Vec<f32>)> {
    state.pcm_to_mel(samples, threads).ok()?;
    let (id, probabilities) = state.lang_detect(0, threads).ok()?;
    debug!(
        "Detected {:?} with probability {:.2}",
        whisper_rs::get_lang_str(id),
        probabilities.get(usize::try_from(id).ok()?).copied()?
    );
    Some((id, probabilities))
}

/// Cut the audio in the middle of pauses. The returned ranges cover all of
/// `samples` in order. Parts shorter than `MIN_PART_MS` are joined to their
/// neighbour, and neighbours are also joined while they fit in `max_merge`
/// samples (0 = only join short parts).
fn split_at_pauses(samples: &[f32], rate: usize, max_merge: usize) -> Vec<Range<usize>> {
    const FRAME_MS: usize = 20;

    let frame = (rate * FRAME_MS / 1000).max(1);
    let levels: Vec<f32> = samples
        .chunks(frame)
        .map(|f| (f.iter().map(|s| s * s).sum::<f32>() / f.len() as f32).sqrt())
        .collect();
    if levels.is_empty() {
        let whole = 0..samples.len();
        return vec![whole];
    }

    // Quiet = well below the level of the loud parts of this recording.
    let mut sorted = levels.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let loud = sorted[(sorted.len() * 95 / 100).min(sorted.len() - 1)];
    let threshold = (loud * 0.1).max(0.002);

    let min_pause_frames = MIN_PAUSE_MS / FRAME_MS;
    let mut cuts = Vec::new();
    let mut quiet_since: Option<usize> = None;
    for (i, &level) in levels.iter().enumerate() {
        if level < threshold {
            quiet_since.get_or_insert(i);
        } else if let Some(start) = quiet_since.take() {
            if start > 0 && i - start >= min_pause_frames {
                cuts.push((start + i) / 2 * frame);
            }
        }
    }

    let min_part = rate * MIN_PART_MS / 1000;
    let mut parts: Vec<Range<usize>> = Vec::new();
    let mut start = 0;
    for cut in cuts.into_iter().chain(std::iter::once(samples.len())) {
        if cut <= start {
            continue;
        }
        let piece = start..cut;
        start = cut;
        if let Some(last) = parts.last_mut() {
            let too_short = piece.len() < min_part || last.len() < min_part;
            if too_short || last.len() + piece.len() <= max_merge {
                last.end = piece.end;
                continue;
            }
        }
        parts.push(piece);
    }
    parts
}

/// Remove whisper's non-speech annotations — "[BLANK_AUDIO]", "(music)",
/// "[ Silence ]", "♪" — so they are never typed into the user's window.
fn strip_non_speech(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth_square = 0u32;
    let mut depth_round = 0u32;
    for ch in text.chars() {
        match ch {
            '[' => depth_square += 1,
            ']' if depth_square > 0 => depth_square -= 1,
            '(' => depth_round += 1,
            ')' if depth_round > 0 => depth_round -= 1,
            '♪' | '♫' => {}
            _ if depth_square == 0 && depth_round == 0 => out.push(ch),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_non_speech_removes_annotations() {
        assert_eq!(strip_non_speech(" [BLANK_AUDIO]"), "");
        assert_eq!(strip_non_speech("(upbeat music)"), "");
        assert_eq!(strip_non_speech("[ Silence ]"), "");
        assert_eq!(strip_non_speech("♪ ♪"), "");
        assert_eq!(
            strip_non_speech(" Hello [BLANK_AUDIO] world."),
            "Hello world."
        );
    }

    /// `secs` of a 300 Hz tone (speech stand-in) or silence at 16 kHz.
    fn tone(secs: f32) -> Vec<f32> {
        (0..(secs * 16_000.0) as usize)
            .map(|i| 0.3 * (i as f32 * 300.0 * std::f32::consts::TAU / 16_000.0).sin())
            .collect()
    }

    fn silence(secs: f32) -> Vec<f32> {
        vec![0.0; (secs * 16_000.0) as usize]
    }

    #[test]
    fn split_at_pauses_cuts_in_the_middle_of_a_pause() {
        let audio = [tone(2.0), silence(1.0), tone(2.0)].concat();
        let parts = split_at_pauses(&audio, 16_000, 0);
        assert_eq!(parts.len(), 2, "{parts:?}");
        assert_eq!(parts[0].start, 0);
        assert_eq!(parts[1].end, audio.len());
        assert_eq!(parts[0].end, parts[1].start);
        let cut_secs = parts[0].end as f32 / 16_000.0;
        assert!((cut_secs - 2.5).abs() < 0.05, "cut at {cut_secs}s");
    }

    #[test]
    fn split_at_pauses_ignores_short_pauses_and_short_parts() {
        // A 0.2 s gap is not a pause; a 0.5 s blip is too short to stand alone.
        let audio = [tone(2.0), silence(0.2), tone(2.0), silence(1.0), tone(0.5)].concat();
        let parts = split_at_pauses(&audio, 16_000, 0);
        assert_eq!(parts, vec![0..audio.len()]);
    }

    #[test]
    fn split_at_pauses_merges_up_to_max_for_long_audio() {
        let mut audio = Vec::new();
        for _ in 0..6 {
            audio.extend(tone(4.0));
            audio.extend(silence(1.0));
        }
        let unmerged = split_at_pauses(&audio, 16_000, 0);
        assert_eq!(unmerged.len(), 6);
        let merged = split_at_pauses(&audio, 16_000, 16_000 * 12);
        assert!(merged.len() < 6 && merged.len() > 1, "{merged:?}");
        assert!(merged.iter().all(|r| r.len() <= 16_000 * 12));
    }

    #[test]
    fn strip_non_speech_keeps_normal_text() {
        assert_eq!(
            strip_non_speech(" And so, my fellow Americans."),
            "And so, my fellow Americans."
        );
        assert_eq!(strip_non_speech("Tom & Jerry <b>"), "Tom & Jerry <b>");
    }

    #[tokio::test]
    async fn worker_creation_and_shutdown() {
        let worker = WhisperWorker::new();
        worker.shutdown().await;
        // If we get here without hanging, the test passes.
    }

    #[tokio::test]
    async fn worker_transcribe_without_model_returns_error() {
        let worker = WhisperWorker::new();

        let audio = AudioBuffer {
            samples: vec![0.0; 16_000],
            sample_rate: 16_000,
        };
        let opts = TranscribeOptions::default();
        let result = worker.transcribe(audio, opts).await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TranscribeError::ModelNotLoaded
        ));

        worker.shutdown().await;
    }

    #[tokio::test]
    async fn worker_load_model_nonexistent_path() {
        let worker = WhisperWorker::new();

        let result = worker
            .load_model(PathBuf::from("/nonexistent/path/model.bin"))
            .await;
        assert!(result.is_err());

        worker.shutdown().await;
    }

    #[tokio::test]
    async fn worker_load_model_and_transcribe() {
        // This test requires a real model file. Use the tiny model if available.
        let model_path = tiny_model_path();
        if !model_path.exists() {
            // Skip test silently if model not present.
            return;
        }

        let worker = WhisperWorker::new();

        let load_result = worker.load_model(model_path).await;
        assert!(
            load_result.is_ok(),
            "load_model failed: {:?}",
            load_result.err()
        );
        assert_eq!(worker.loaded_model().as_deref(), Some("tiny"));

        // Transcribe 1 second of silence.
        let audio = AudioBuffer {
            samples: vec![0.0; 16_000],
            sample_rate: 16_000,
        };
        let opts = TranscribeOptions::default();
        let result = worker.transcribe(audio, opts).await;
        assert!(result.is_ok(), "transcribe failed: {:?}", result.err());

        let transcript = result.unwrap();
        // Silence must not turn into "[BLANK_AUDIO]" text.
        assert!(transcript.segments.iter().all(|s| !s.text.contains('[')));
        // Silence may produce 0 or more segments depending on model.
        assert!(transcript.duration > 0.0);

        worker.shutdown().await;
    }

    /// Returns the path to the downloaded tiny model, if present.
    fn tiny_model_path() -> PathBuf {
        let data_dir = std::env::var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").expect("HOME not set");
                PathBuf::from(home).join(".local/share")
            });
        data_dir.join("linux-whisper/models/ggml-tiny.bin")
    }

    #[tokio::test]
    async fn worker_integration_real_model() {
        let model_path = tiny_model_path();
        if !model_path.exists() {
            // Skip test silently if model not present.
            return;
        }

        let worker = WhisperWorker::new();

        let load_result = worker.load_model(model_path).await;
        assert!(
            load_result.is_ok(),
            "load_model failed: {:?}",
            load_result.err()
        );

        // Transcribe 1 second of silence.
        let audio = AudioBuffer {
            samples: vec![0.0; 16_000],
            sample_rate: 16_000,
        };
        let opts = TranscribeOptions::default();
        let result = worker.transcribe(audio, opts).await;
        assert!(result.is_ok(), "transcribe failed: {:?}", result.err());

        worker.shutdown().await;
    }
}
