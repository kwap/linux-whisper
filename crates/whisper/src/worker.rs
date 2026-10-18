//! Worker thread for asynchronous whisper inference.
//!
//! [`WhisperWorker`] spawns a dedicated OS thread for running synchronous
//! whisper-rs inference and exposes an async interface via tokio channels.
//! This keeps the tokio runtime free from blocking compute work.

use std::borrow::Cow;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use linux_whisper_audio::capture::AudioBuffer;
use linux_whisper_core::model::{languages_spoken, Segment, Transcript, TranscriptSource};
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
    /// Transcribe the unfinished part of a recording that is still going on.
    Live {
        /// The audio since the end of the final text so far.
        audio: AudioBuffer,
        /// Transcription options (language, translate flag, etc.).
        options: TranscribeOptions,
        /// What is known about the recording before `audio`.
        context: LiveContext,
        /// The recording has ended: make all of `audio` final.
        finish: bool,
        /// Channel to send the result back on.
        reply: oneshot::Sender<Result<LiveText, TranscribeError>>,
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

    /// Transcribes the unfinished part of a live recording: what was said
    /// since the end of its final text (see [`LiveText`]). Call again with
    /// more audio as the recording goes on, and with `finish` once it ends.
    pub async fn transcribe_live(
        &self,
        audio: AudioBuffer,
        options: TranscribeOptions,
        context: LiveContext,
        finish: bool,
    ) -> Result<LiveText, TranscribeError> {
        let (tx, rx) = oneshot::channel();

        self.sender
            .send(WorkerCommand::Live {
                audio,
                options,
                context,
                finish,
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
        // Kept through a live recording: a new state per update would
        // allocate the model's buffers several times a second.
        let mut live_state: Option<WhisperState> = None;

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

                    WorkerCommand::Live {
                        audio,
                        options,
                        context,
                        finish,
                        reply,
                    } => {
                        let Some(ref whisper_ctx) = ctx else {
                            let _ = reply.send(Err(TranscribeError::ModelNotLoaded));
                            continue;
                        };
                        if live_state.is_none() {
                            match whisper_ctx.create_state() {
                                Ok(state) => live_state = Some(state),
                                Err(e) => {
                                    let _ = reply.send(Err(TranscribeError::TranscriptionFailed(
                                        format!("create state: {e}"),
                                    )));
                                    continue;
                                }
                            }
                        }
                        let Some(ref mut state) = live_state else {
                            continue;
                        };
                        let result =
                            run_live(whisper_ctx, state, &audio, &options, context, finish);
                        if finish {
                            live_state = None;
                        }
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
                                live_state = None;
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

/// A part with less sound than this above the quiet level holds no speech
/// (only a key click or a breath) and is not transcribed: Whisper makes up
/// text for such noise, often in a language nobody spoke.
const MIN_SPEECH_MS: usize = 250;

/// Recordings longer than this (files, mostly) are split into parts of up to
/// `LONG_AUDIO_PART_SECS` so identifying languages stays cheap.
const LONG_AUDIO_SECS: usize = 120;
const LONG_AUDIO_PART_SECS: usize = 30;

/// One detection over a recording that fits in whisper's 30 s window: above
/// this probability the recording is taken to be in one language.
const WHOLE_CLIP_SURE_PROBABILITY: f32 = 0.99;
const WINDOW_SECS: usize = 30;

/// A part's detected language is taken as it is above this probability.
const SURE_LANGUAGE_PROBABILITY: f32 = 0.8;

/// Below it, up to this many of the likeliest languages are tried on the
/// part, and the one the model transcribes it best in wins. An accent makes
/// detection unsure (a Polish speaker's English can come out as Latin or
/// Polish), but transcribing speech in the wrong language goes visibly worse.
const MAX_CANDIDATES: usize = 3;
const MIN_CANDIDATE_PROBABILITY: f32 = 0.02;
const DETECTION_WEIGHT: f32 = 0.05;

/// Trying languages costs a full decode each. Checks stop once they have
/// taken as long as the recording (at least this long): after that, parts
/// get the detected language. On a GPU the limit is never reached.
const MIN_CHECK_SECS: f64 = 5.0;

/// Live dictation: unfinished speech longer than this is made final at its
/// last pause, even one that does not end a sentence. A long guess is slow
/// to redo, and any change near its start means erasing all that follows.
const LIVE_LONG_GUESS_SECS: usize = 6;

/// Live dictation: unfinished speech longer than this (no pause in it) is
/// made final up to its last sentence.
const LIVE_MAX_UNFINISHED_SECS: usize = 20;

/// Whisper's own sign of made-up text: it thought there was no speech, and
/// it is unsure of the words (the thresholds OpenAI's Whisper uses).
const NO_SPEECH_PROBABILITY: f32 = 0.6;
const LOW_AVERAGE_LOG_PROBABILITY: f32 = -1.0;

/// Run whisper-rs transcription synchronously. Called on the worker thread.
fn run_transcription(
    ctx: &WhisperContext,
    audio: &AudioBuffer,
    options: &TranscribeOptions,
    model_name: &str,
) -> Result<Transcript, TranscribeError> {
    let threads = decode_threads();
    let rate = audio.sample_rate as usize;
    let samples = padded(&audio.samples, rate);

    let mut state = ctx
        .create_state()
        .map_err(|e| TranscribeError::TranscriptionFailed(format!("create state: {e}")))?;

    // Decide which language to use for which part of the audio.
    let spans = choose_spans(ctx, &mut state, &samples, rate, threads, options, 0.0);
    let segments = decode_spans(
        &mut state,
        &samples,
        rate,
        threads,
        ctx.token_eot(),
        &spans,
        options,
        false,
    )?;

    let mut transcript = Transcript::new(
        "Transcription",
        options.language.clone(),
        model_name,
        TranscriptSource::Dictation,
    );
    transcript.duration = audio.samples.len() as f64 / rate as f64;
    for segment in segments {
        transcript.add_segment(segment);
    }

    // Report the language spoken the most.
    let languages = languages_spoken(&transcript.segments);
    if transcript.language.is_none() {
        transcript.language = languages.first().cloned();
    }

    info!(
        "Whisper inference complete: {} segment(s), {:.1}s audio, language(s): {:?}",
        transcript.segment_count(),
        transcript.duration,
        languages,
    );

    Ok(transcript)
}

/// whisper.cpp defaults to at most 4 threads; more makes CPU-only
/// transcription noticeably faster on desktop machines.
fn decode_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 8)
}

/// whisper.cpp ignores input shorter than one second, so a quick "yes"
/// would come back empty. Short clips are padded with silence.
fn padded(samples: &[f32], rate: usize) -> Cow<'_, [f32]> {
    let min_samples = rate * 11 / 10;
    if samples.len() < min_samples {
        let mut buf = samples.to_vec();
        buf.resize(min_samples, 0.0);
        Cow::Owned(buf)
    } else {
        Cow::Borrowed(samples)
    }
}

/// Which language to use for which part of `samples`.
fn choose_spans(
    ctx: &WhisperContext,
    state: &mut WhisperState,
    samples: &[f32],
    rate: usize,
    threads: usize,
    options: &TranscribeOptions,
    loud: f32,
) -> Vec<(Range<usize>, Option<String>)> {
    match &options.language {
        Some(lang) => vec![(0..samples.len(), Some(lang.clone()))],
        None if ctx.is_multilingual() => {
            let allowed = language_ids(&options.languages);
            language_spans(
                state,
                samples,
                rate,
                threads,
                ctx.token_eot(),
                &allowed,
                loud,
            )
        }
        // English-only models: auto-detecting would only cost time and report
        // a meaningless language.
        None => vec![(0..samples.len(), Some("en".to_string()))],
    }
}

/// Transcribe each span of `samples` in its language (`None`: whisper
/// detects it). A `quick` reading (a live guess) takes whisper's first
/// reading even when it is unsure of it: retrying at higher temperatures can
/// take seconds, and the guess is read again moments later anyway.
#[allow(clippy::too_many_arguments)]
fn decode_spans(
    state: &mut WhisperState,
    samples: &[f32],
    rate: usize,
    threads: usize,
    eot: i32,
    spans: &[(Range<usize>, Option<String>)],
    options: &TranscribeOptions,
    quick: bool,
) -> Result<Vec<Segment>, TranscribeError> {
    let mut segments = Vec::new();
    for (range, language) in spans {
        // `None` makes whisper.cpp detect the language itself.
        let mut params = decode_params(threads, language.as_deref(), options.translate);
        if quick {
            params.set_temperature_inc(0.0);
        }
        state
            .full(params, &samples[range.clone()])
            .map_err(|e| TranscribeError::TranscriptionFailed(format!("inference failed: {e}")))?;

        let used_language = language.clone().or_else(|| {
            whisper_rs::get_lang_str(state.full_lang_id_from_state()).map(str::to_string)
        });

        let offset = range.start as f64 / rate as f64;
        for i in 0..state.full_n_segments() {
            let Some(seg) = state.get_segment(i) else {
                continue;
            };

            let raw = seg.to_str_lossy().map_err(|e| {
                TranscribeError::TranscriptionFailed(format!("failed to get segment {i} text: {e}"))
            })?;
            let cleaned = strip_non_speech(&raw);
            if cleaned.is_empty() {
                continue;
            }

            // Probabilities of the segment's text tokens.
            let probabilities: Vec<f32> = (0..seg.n_tokens())
                .filter_map(|t| seg.get_token(t))
                .filter(|tok| tok.token_id() < eot)
                .map(|tok| tok.token_probability())
                .collect();
            let count = probabilities.len().max(1) as f32;
            let average_log_probability =
                probabilities.iter().map(|p| p.max(1e-6).ln()).sum::<f32>() / count;
            if seg.no_speech_probability() > NO_SPEECH_PROBABILITY
                && average_log_probability < LOW_AVERAGE_LOG_PROBABILITY
            {
                debug!("Dropped as not speech: {cleaned:?}");
                continue;
            }

            // whisper.cpp timestamps are in centiseconds (10ms units).
            let start_sec = offset + seg.start_timestamp() as f64 / 100.0;
            let end_sec = offset + seg.end_timestamp() as f64 / 100.0;

            let mut segment = Segment::new(start_sec, end_sec, cleaned);
            segment.confidence =
                (!probabilities.is_empty()).then(|| probabilities.iter().sum::<f32>() / count);
            segment.language = used_language.clone();
            segments.push(segment);
        }
    }

    Ok(segments)
}

/// Decoding parameters: greedy, each call independent of the previous one.
fn decode_params(threads: usize, language: Option<&str>, translate: bool) -> FullParams<'_, '_> {
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_n_threads(threads as i32);
    params.set_language(language);
    params.set_translate(translate);
    params.set_suppress_nst(false);
    params.set_single_segment(false);
    params.set_no_context(true);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params
}

// ---------------------------------------------------------------------------
// Live transcription
// ---------------------------------------------------------------------------

/// What is known about a live recording before the audio sent next.
#[derive(Debug, Clone, Default)]
pub struct LiveContext {
    /// The language the speaker was last heard in.
    pub language: Option<String>,
    /// The loudness of the speech so far (see `Loudness`), so that the quiet
    /// after it is not taken for speech.
    pub loud: f32,
}

impl LiveContext {
    /// This context after `segments` were heard.
    fn followed_by(&self, segments: &[Segment]) -> Self {
        let mut next = self.clone();
        if let Some(language) = segments.iter().rev().find_map(|s| s.language.clone()) {
            next.language = Some(language);
        }
        next
    }
}

/// The text of a live recording's unfinished audio.
#[derive(Debug, Clone, Default)]
pub struct LiveText {
    /// How many samples at the start of the audio now have their final text.
    /// The next request starts after them.
    pub done: usize,
    /// The final text of those samples, timed from the start of the audio.
    pub done_segments: Vec<Segment>,
    /// The best guess at the rest, which may change as more is said.
    pub guess: Vec<Segment>,
    /// The context for the next request.
    pub context: LiveContext,
}

/// Transcribe a live recording's unfinished audio. All of it is read as a
/// guess, which the next update reads again with more audio. A pause where
/// the guess ends a sentence makes everything before it final: that part is
/// transcribed again as a recording of its own (languages and all) and not
/// looked at again.
fn run_live(
    ctx: &WhisperContext,
    state: &mut WhisperState,
    audio: &AudioBuffer,
    options: &TranscribeOptions,
    before: LiveContext,
    finish: bool,
) -> Result<LiveText, TranscribeError> {
    let threads = decode_threads();
    let rate = audio.sample_rate as usize;
    let samples = &audio.samples[..];
    let len = samples.len();
    let eot = ctx.token_eot();
    let loudness = Loudness::new(samples, rate, before.loud);
    let before = LiveContext {
        loud: loudness.loud,
        ..before
    };
    let loud = before.loud;
    let finished = |state: &mut WhisperState, end: usize| {
        let part = padded(&samples[..end], rate);
        let spans = choose_spans(ctx, state, &part, rate, threads, options, loud);
        decode_spans(state, &part, rate, threads, eot, &spans, options, false)
    };

    let has_speech = loudness.speech_ms(&(0..len)) >= MIN_SPEECH_MS;
    if finish {
        let done_segments = if has_speech {
            finished(state, len)?
        } else {
            Vec::new()
        };
        return Ok(LiveText {
            done: len,
            context: before.followed_by(&done_segments),
            done_segments,
            guess: Vec::new(),
        });
    }
    if !has_speech {
        // Quiet so far: no need to send it again, except its very end, where
        // speech may be starting.
        return Ok(LiveText {
            done: if len > rate { len - rate / 2 } else { 0 },
            context: before,
            ..LiveText::default()
        });
    }

    let unfinished = padded(samples, rate);
    let language = guess_language(ctx, state, &unfinished, rate, threads, options, &before);
    let spans = [(0..unfinished.len(), language)];
    let mut guess = decode_spans(
        state,
        &unfinished,
        rate,
        threads,
        eot,
        &spans,
        options,
        true,
    )?;
    // A quick reading can get stuck repeating itself; that would be typed.
    if runs_away(&guess, len as f64 / rate as f64) {
        debug!("Quick reading ran away; reading again");
        guess = decode_spans(
            state,
            &unfinished,
            rate,
            threads,
            eot,
            &spans,
            options,
            false,
        )?;
    }
    let heard = before.followed_by(&guess);

    // A pause after the end of a sentence finishes it. Once the guess is
    // long, so does any pause; speech without pauses is finished up to its
    // last sentence. A long guess is slow to redo, and a change near its
    // start means erasing all that follows.
    let pauses = phrase_ends(&loudness, len);
    let too_long = len > LIVE_MAX_UNFINISHED_SECS * rate;
    let sentence_end = pauses
        .iter()
        .rev()
        .find(|pause| ends_sentence(&guess, pause, rate))
        .or(pauses.last().filter(|_| len > LIVE_LONG_GUESS_SECS * rate));
    // While the speaker is still talking, the full stop whisper puts at the
    // end of the audio is not the end of a sentence; typing it would only
    // mean erasing it again.
    let talking = loudness.speech_ms(&(len.saturating_sub(rate * 3 / 10)..len)) >= 100;
    if talking {
        if let Some(last) = guess.last_mut() {
            let kept = last.text.trim_end_matches(['.', ',', '!', '?', '…']).len();
            last.text.truncate(kept);
        }
    }

    if let Some(pause) = sentence_end {
        let done = (pause.start + pause.end) / 2;
        let done_segments = finished(state, done)?;
        guess.retain(|s| s.start * rate as f64 >= pause.start as f64);
        return Ok(LiveText {
            done,
            context: heard.followed_by(&done_segments).followed_by(&guess),
            done_segments,
            guess,
        });
    }
    if too_long && guess.len() > 1 {
        let last = guess.split_off(guess.len() - 1);
        return Ok(LiveText {
            done: ((last[0].start * rate as f64) as usize).min(len),
            done_segments: guess,
            guess: last,
            context: heard,
        });
    }

    Ok(LiveText {
        done: 0,
        done_segments: Vec::new(),
        guess,
        context: heard,
    })
}

/// More text than anyone says in this time: whisper is repeating itself.
fn runs_away(segments: &[Segment], secs: f64) -> bool {
    let chars: usize = segments.iter().map(|s| s.text.chars().count()).sum();
    chars as f64 > 30.0 + 30.0 * secs
}

/// How far whisper's timestamps may be from the pause they belong to.
const TIMESTAMP_SLACK_SECS: f64 = 0.6;

/// Whether the guess ends a sentence at `pause` (samples).
fn ends_sentence(guess: &[Segment], pause: &Range<usize>, rate: usize) -> bool {
    let (start, end) = (
        pause.start as f64 / rate as f64 - TIMESTAMP_SLACK_SECS,
        pause.end as f64 / rate as f64 + TIMESTAMP_SLACK_SECS,
    );
    guess.iter().any(|segment| {
        (start..=end).contains(&segment.end)
            && segment
                .text
                .trim_end()
                .ends_with(['.', '!', '?', '…', '。', '！', '？'])
    })
}

/// The pauses in live audio that may end a phrase (in samples): long enough,
/// and after enough speech to tell its language. A pause still going on is
/// not one yet: where its middle is, and so where the audio is cut, depends
/// on how long it turns out to be.
fn phrase_ends(loudness: &Loudness, len: usize) -> Vec<Range<usize>> {
    let min_pause = MIN_PAUSE_MS / Loudness::FRAME_MS;
    let min_phrase = MIN_PART_MS / Loudness::FRAME_MS;
    let min_speech = MIN_SPEECH_MS / Loudness::FRAME_MS;
    let samples = |frame: usize| (frame * loudness.frame).min(len);

    let mut pauses = Vec::new();
    let mut first_speech: Option<usize> = None;
    let mut speech_frames = 0;
    let mut quiet_since: Option<usize> = None;
    let mut consider = |start: usize, stop: usize, speech: usize, first: Option<usize>| {
        if first.is_some_and(|first| start - first >= min_phrase)
            && stop - start >= min_pause
            && speech >= min_speech
        {
            pauses.push(samples(start)..samples(stop));
        }
    };
    for (i, &level) in loudness.levels.iter().enumerate() {
        if level < loudness.quiet {
            quiet_since.get_or_insert(i);
        } else {
            if let Some(start) = quiet_since.take() {
                consider(start, i, speech_frames, first_speech);
            }
            first_speech.get_or_insert(i);
            speech_frames += 1;
        }
    }
    pauses
}

/// The language to read unfinished live speech in. The language of the text
/// before is kept unless detection is sure of another one, so the guess does
/// not jump between languages mid-sentence (an accent makes detection
/// waver); the final reading at the next pause tries the likely languages
/// properly.
fn guess_language(
    ctx: &WhisperContext,
    state: &mut WhisperState,
    samples: &[f32],
    rate: usize,
    threads: usize,
    options: &TranscribeOptions,
    before: &LiveContext,
) -> Option<String> {
    if options.language.is_some() {
        return options.language.clone();
    }
    if !ctx.is_multilingual() {
        return Some("en".to_string());
    }
    let name = |id: i32| whisper_rs::get_lang_str(id).map(str::to_string);
    let allowed = language_ids(&options.languages);
    if let [only] = allowed[..] {
        return name(only);
    }

    let previous = before.language.as_deref().and_then(whisper_rs::get_lang_id);
    // Too little to tell yet.
    if previous.is_some() && samples.len() < rate * MIN_PART_MS / 1000 {
        return before.language.clone();
    }
    let probabilities = detect_language(state, samples, threads)?;
    let ranked = rank(&probabilities, &allowed);
    let &(top, top_probability) = ranked.first()?;
    match previous {
        Some(previous)
            if top_probability < SURE_LANGUAGE_PROBABILITY
                && ranked
                    .iter()
                    .any(|&(id, p)| id == previous && p >= MIN_CANDIDATE_PROBABILITY) =>
        {
            name(previous)
        }
        _ => name(top),
    }
}

/// Whisper's ids of the given language codes; unknown codes are left out.
fn language_ids(codes: &[String]) -> Vec<i32> {
    codes
        .iter()
        .filter_map(|code| whisper_rs::get_lang_id(code))
        .collect()
}

/// Decide which language each part of the audio is in, so a speaker can
/// switch language from one sentence to the next. Detection chooses among
/// `allowed` (any language if empty). Parts without speech are left out, and
/// consecutive parts in the same language are joined, so speech in one
/// language is still transcribed in a single pass.
fn language_spans(
    state: &mut WhisperState,
    samples: &[f32],
    rate: usize,
    threads: usize,
    eot: i32,
    allowed: &[i32],
    loud: f32,
) -> Vec<(Range<usize>, Option<String>)> {
    let name = |id: i32| whisper_rs::get_lang_str(id).map(str::to_string);
    if let [only] = allowed {
        return vec![(0..samples.len(), name(*only))];
    }

    let checks_until = Instant::now()
        + Duration::from_secs_f64((samples.len() as f64 / rate as f64).max(MIN_CHECK_SECS));
    let loudness = Loudness::new(samples, rate, loud);
    let max_merge = if samples.len() > LONG_AUDIO_SECS * rate {
        LONG_AUDIO_PART_SECS * rate
    } else {
        0
    };
    let parts: Vec<Range<usize>> = split_at_pauses(&loudness, samples.len(), max_merge)
        .into_iter()
        .filter(|part| loudness.speech_ms(part) >= MIN_SPEECH_MS)
        .collect();
    if parts.is_empty() {
        debug!("No speech in the recording");
        return Vec::new();
    }

    // Most recordings are in one language. One detection tells, and then the
    // speech is transcribed in that language in a single pass. Only an unsure
    // result (typical when two languages are mixed) is worth going part by
    // part.
    let speech = parts[0].start..parts[parts.len() - 1].end;
    if parts.len() == 1 || speech.len() <= WINDOW_SECS * rate {
        let sure = detect_language(state, &samples[speech.clone()], threads)
            .and_then(|probabilities| rank(&probabilities, allowed).first().copied())
            .filter(|&(_, p)| parts.len() == 1 || p >= WHOLE_CLIP_SURE_PROBABILITY);
        if let Some((id, _)) = sure.filter(|&(_, p)| p >= SURE_LANGUAGE_PROBABILITY) {
            return vec![(speech, name(id))];
        }
        if parts.len() == 1 {
            let id = choose_language(
                state,
                &samples[speech.clone()],
                threads,
                eot,
                allowed,
                checks_until,
            );
            return vec![(speech, id.and_then(name))];
        }
    }

    let languages: Vec<Option<i32>> = parts
        .iter()
        .map(|part| {
            choose_language(
                state,
                &samples[part.clone()],
                threads,
                eot,
                allowed,
                checks_until,
            )
        })
        .collect();

    // A part whose language could not be told takes its neighbour's.
    let known: Vec<i32> = languages.iter().flatten().copied().collect();
    let Some(&first_known) = known.first() else {
        return vec![(speech, None)];
    };
    let mut previous = first_known;
    let mut spans: Vec<(Range<usize>, i32)> = Vec::new();
    for (part, lang) in parts.into_iter().zip(languages) {
        let lang = lang.unwrap_or(previous);
        previous = lang;
        match spans.last_mut() {
            // Joined across a pause without speech, which whisper handles.
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
        .map(|(range, lang)| (range, name(lang)))
        .collect()
}

/// The language of `samples`: the detected one if detection is sure, else
/// the likeliest languages are tried and the one whisper reads the speech
/// best in wins.
fn choose_language(
    state: &mut WhisperState,
    samples: &[f32],
    threads: usize,
    eot: i32,
    allowed: &[i32],
    checks_until: Instant,
) -> Option<i32> {
    let probabilities = detect_language(state, samples, threads)?;
    let ranked = rank(&probabilities, allowed);
    let &(top, top_probability) = ranked.first()?;
    if top_probability >= SURE_LANGUAGE_PROBABILITY {
        return Some(top);
    }
    if Instant::now() >= checks_until {
        debug!("No time left to try languages; taking the detected one");
        return Some(top);
    }
    let candidates: Vec<(i32, f32)> = ranked
        .iter()
        .take(MAX_CANDIDATES)
        .filter(|&&(_, p)| p >= MIN_CANDIDATE_PROBABILITY)
        .copied()
        .collect();
    if candidates.len() < 2 {
        return Some(top);
    }

    candidates
        .into_iter()
        .filter_map(|(id, p)| {
            let reading = read_as(state, samples, id, threads, eot)?;
            debug!(
                "Candidate {:?} (detected {p:.2}): {:.2} per token, {} tokens: {:?}",
                whisper_rs::get_lang_str(id),
                reading.average_log_probability,
                reading.tokens,
                reading.text
            );
            let secs = samples.len() as f32 / AudioBuffer::TARGET_SAMPLE_RATE as f32;
            // Detection breaks near ties.
            Some((
                id,
                reading.score(secs) + DETECTION_WEIGHT * p.max(1e-6).ln(),
            ))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id)
        .or(Some(top))
}

/// How well whisper transcribes some audio in one language.
struct Reading {
    average_log_probability: f32,
    tokens: usize,
    text: String,
}

impl Reading {
    /// Higher is better. Speech read in the wrong language comes out as
    /// unlikely words, or as very little text for the time spoken.
    fn score(&self, secs: f32) -> f32 {
        let too_little = (1.0 - self.tokens as f32 / secs.max(1.0)).max(0.0);
        self.average_log_probability - too_little
    }
}

/// Transcribe `samples` as language `id` and measure how sure whisper is of
/// the words.
fn read_as(
    state: &mut WhisperState,
    samples: &[f32],
    id: i32,
    threads: usize,
    eot: i32,
) -> Option<Reading> {
    let language = whisper_rs::get_lang_str(id)?;
    state
        .full(decode_params(threads, Some(language), false), samples)
        .ok()?;
    let mut sum = 0.0f32;
    let mut tokens = 0usize;
    let mut text = String::new();
    for i in 0..state.full_n_segments() {
        let Some(seg) = state.get_segment(i) else {
            continue;
        };
        if let Ok(t) = seg.to_str_lossy() {
            text.push_str(&t);
        }
        for tok in (0..seg.n_tokens()).filter_map(|t| seg.get_token(t)) {
            if tok.token_id() < eot {
                sum += tok.token_probability().max(1e-6).ln();
                tokens += 1;
            }
        }
    }
    Some(Reading {
        average_log_probability: if tokens == 0 {
            -10.0
        } else {
            sum / tokens as f32
        },
        tokens,
        text: strip_non_speech(&text),
    })
}

/// Languages from most to least likely, only those in `allowed` (all if empty).
fn rank(probabilities: &[f32], allowed: &[i32]) -> Vec<(i32, f32)> {
    let mut ranked: Vec<(i32, f32)> = probabilities
        .iter()
        .enumerate()
        .map(|(id, &p)| (id as i32, p))
        .filter(|(id, _)| allowed.is_empty() || allowed.contains(id))
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    ranked
}

/// The probability of every language for `samples`.
fn detect_language(state: &mut WhisperState, samples: &[f32], threads: usize) -> Option<Vec<f32>> {
    state.pcm_to_mel(samples, threads).ok()?;
    let (id, probabilities) = state.lang_detect(0, threads).ok()?;
    debug!(
        "Detected {:?} with probability {:.2}",
        whisper_rs::get_lang_str(id),
        probabilities.get(usize::try_from(id).ok()?).copied()?
    );
    Some(probabilities)
}

/// Loudness of the audio in 20 ms frames, and the level below which a frame
/// counts as quiet: well below the loud parts of this recording.
struct Loudness {
    levels: Vec<f32>,
    /// The level of the loud parts.
    loud: f32,
    quiet: f32,
    frame: usize,
}

impl Loudness {
    const FRAME_MS: usize = 20;

    /// `at_least` is the level of loud parts heard before, for audio that
    /// continues a recording: without it, the quiet after speech would be
    /// measured against itself and taken for speech.
    fn new(samples: &[f32], rate: usize, at_least: f32) -> Self {
        let frame = (rate * Self::FRAME_MS / 1000).max(1);
        let levels: Vec<f32> = samples
            .chunks(frame)
            .map(|f| (f.iter().map(|s| s * s).sum::<f32>() / f.len() as f32).sqrt())
            .collect();
        let mut sorted = levels.clone();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let loud = sorted
            .get((sorted.len() * 95 / 100).min(sorted.len().saturating_sub(1)))
            .copied()
            .unwrap_or(0.0)
            .max(at_least);
        Self {
            levels,
            loud,
            // About -60 dBFS: quieter than any microphone carrying speech.
            quiet: (loud * 0.1).max(0.001),
            frame,
        }
    }

    /// Milliseconds of sound above the quiet level in `range` (in samples).
    fn speech_ms(&self, range: &Range<usize>) -> usize {
        let first = range.start / self.frame;
        let last = range.end.div_ceil(self.frame).min(self.levels.len());
        self.levels
            .get(first..last)
            .unwrap_or_default()
            .iter()
            .filter(|&&level| level >= self.quiet)
            .count()
            * Self::FRAME_MS
    }
}

/// Cut the audio (`len` samples) in the middle of pauses. The returned ranges
/// cover all of it in order. Parts shorter than `MIN_PART_MS` are joined to
/// their neighbour, and neighbours are also joined while they fit in
/// `max_merge` samples (0 = only join short parts).
fn split_at_pauses(loudness: &Loudness, len: usize, max_merge: usize) -> Vec<Range<usize>> {
    let frame = loudness.frame;
    let rate = frame * 1000 / Loudness::FRAME_MS;
    if loudness.levels.is_empty() {
        let whole = 0..len;
        return vec![whole];
    }

    let min_pause_frames = MIN_PAUSE_MS / Loudness::FRAME_MS;
    let mut cuts = Vec::new();
    let mut quiet_since: Option<usize> = None;
    for (i, &level) in loudness.levels.iter().enumerate() {
        if level < loudness.quiet {
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
    for cut in cuts.into_iter().chain(std::iter::once(len)) {
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

    fn split(audio: &[f32], max_merge: usize) -> Vec<Range<usize>> {
        split_at_pauses(&Loudness::new(audio, 16_000, 0.0), audio.len(), max_merge)
    }

    #[test]
    fn split_at_pauses_cuts_in_the_middle_of_a_pause() {
        let audio = [tone(2.0), silence(1.0), tone(2.0)].concat();
        let parts = split(&audio, 0);
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
        let parts = split(&audio, 0);
        assert_eq!(parts, vec![0..audio.len()]);
    }

    #[test]
    fn split_at_pauses_merges_up_to_max_for_long_audio() {
        let mut audio = Vec::new();
        for _ in 0..6 {
            audio.extend(tone(4.0));
            audio.extend(silence(1.0));
        }
        let unmerged = split(&audio, 0);
        assert_eq!(unmerged.len(), 6);
        let merged = split(&audio, 16_000 * 12);
        assert!(merged.len() < 6 && merged.len() > 1, "{merged:?}");
        assert!(merged.iter().all(|r| r.len() <= 16_000 * 12));
    }

    #[test]
    fn a_key_click_is_not_speech() {
        let click: Vec<f32> = (0..192)
            .map(|i| if i % 2 == 0 { 0.5 } else { -0.5 })
            .collect();
        let audio = [silence(1.0), click, silence(1.0), tone(2.0)].concat();
        let loudness = Loudness::new(&audio, 16_000, 0.0);
        assert!(loudness.speech_ms(&(0..32_000)) < MIN_SPEECH_MS);
        assert!(loudness.speech_ms(&(32_000..audio.len())) >= 1900);
    }

    #[test]
    fn silence_is_not_speech() {
        let audio = silence(3.0);
        let loudness = Loudness::new(&audio, 16_000, 0.0);
        assert_eq!(loudness.speech_ms(&(0..audio.len())), 0);
    }

    fn pauses_secs(audio: &[f32]) -> Vec<(f32, f32)> {
        phrase_ends(&Loudness::new(audio, 16_000, 0.0), audio.len())
            .iter()
            .map(|p| (p.start as f32 / 16_000.0, p.end as f32 / 16_000.0))
            .collect()
    }

    #[test]
    fn a_pause_after_a_phrase_may_end_it() {
        let audio = [tone(2.0), silence(0.8), tone(1.0)].concat();
        assert_eq!(pauses_secs(&audio), vec![(2.0, 2.8)]);
        // Not while it is still going on.
        assert!(pauses_secs(&[tone(2.0), silence(0.6)].concat()).is_empty());
        let audio = [tone(2.0), silence(0.6), tone(2.0), silence(0.6), tone(0.5)].concat();
        assert_eq!(pauses_secs(&audio).len(), 2);
    }

    #[test]
    fn short_gaps_and_short_phrases_end_nothing() {
        assert!(pauses_secs(&tone(5.0)).is_empty());
        assert!(pauses_secs(&[tone(2.0), silence(0.3), tone(1.0)].concat()).is_empty());
        // Quiet before any speech.
        assert!(pauses_secs(&[silence(2.0), tone(1.0)].concat()).is_empty());
        // Too little to tell the language of.
        assert!(pauses_secs(&[tone(0.8), silence(1.4), tone(1.0)].concat()).is_empty());
    }

    #[test]
    fn a_sentence_ends_where_whisper_put_a_full_stop_by_the_pause() {
        let pause = 32_000..44_800;
        let said = |end: f64, text: &str| {
            vec![
                Segment::new(0.0, end, text),
                Segment::new(2.9, 4.0, "and more"),
            ]
        };
        assert!(ends_sentence(&said(2.1, "Hello there."), &pause, 16_000));
        assert!(ends_sentence(&said(3.2, "Is it?"), &pause, 16_000));
        assert!(!ends_sentence(
            &said(2.1, "I would like to"),
            &pause,
            16_000
        ));
        assert!(!ends_sentence(&said(1.0, "Hello."), &pause, 16_000));
    }

    #[test]
    fn a_reading_far_longer_than_speech_runs_away() {
        let said = |text: &str| vec![Segment::new(0.0, 2.0, text)];
        assert!(!runs_away(
            &said("Hello, I am testing the new dictation app."),
            2.0
        ));
        assert!(runs_away(&said(&"the ".repeat(40)), 2.0));
    }

    #[test]
    fn quiet_after_speech_is_not_speech() {
        let noise: Vec<f32> = (0..16_000)
            .map(|i| if i % 2 == 0 { 0.01 } else { -0.01 })
            .collect();
        let alone = Loudness::new(&noise, 16_000, 0.0);
        assert!(alone.speech_ms(&(0..noise.len())) > 900);
        let after_speech = Loudness::new(&noise, 16_000, 0.3);
        assert_eq!(after_speech.speech_ms(&(0..noise.len())), 0);
    }

    fn segment_in(start: f64, end: f64, text: &str, language: &str) -> Segment {
        let mut segment = Segment::new(start, end, text);
        segment.language = Some(language.to_string());
        segment
    }

    #[test]
    fn live_context_follows_the_last_language_heard() {
        let context = LiveContext::default().followed_by(&[
            segment_in(0.0, 1.0, "Hello.", "en"),
            segment_in(1.0, 2.0, "Cześć.", "pl"),
        ]);
        assert_eq!(context.language.as_deref(), Some("pl"));
        assert_eq!(context.followed_by(&[]).language.as_deref(), Some("pl"));
    }

    #[test]
    fn rank_keeps_only_allowed_languages() {
        let en = whisper_rs::get_lang_id("en").unwrap();
        let de = whisper_rs::get_lang_id("de").unwrap();
        let ja = whisper_rs::get_lang_id("ja").unwrap();
        let mut probabilities = vec![0.0; 100];
        probabilities[ja as usize] = 0.5;
        probabilities[en as usize] = 0.3;
        probabilities[de as usize] = 0.2;
        assert_eq!(rank(&probabilities, &[]).first(), Some(&(ja, 0.5)));
        assert_eq!(rank(&probabilities, &[de, en]), vec![(en, 0.3), (de, 0.2)]);
        assert_eq!(language_ids(&["pl".into(), "xx".into()]).len(), 1);
    }

    #[test]
    fn reading_too_little_text_scores_lower() {
        let fluent = Reading {
            average_log_probability: -0.4,
            tokens: 16,
            text: String::new(),
        };
        let sparse = Reading {
            average_log_probability: -0.2,
            tokens: 1,
            text: String::new(),
        };
        assert!(fluent.score(4.0) > sparse.score(4.0));
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
