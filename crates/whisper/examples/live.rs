//! Dictate an audio file as if it were being spoken: prints the text as live
//! dictation would type it after each update, how much of it each update
//! erased again, and how soon after the end the final text was there.
//!
//! ```text
//! cargo run --release -p linux-whisper-whisper --example live -- \
//!     ~/.local/share/linux-whisper/models/ggml-base.bin recording.m4a [language | en,pl,de]
//! ```
//!
//! The audio "arrives" as fast as it would from a microphone: while the
//! model works on one update, the next part of the file is being spoken.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use linux_whisper_audio::capture::AudioBuffer;
use linux_whisper_audio::decode::decode_to_mono_16khz;
use linux_whisper_core::format::{format_typed_segments, FormatOptions};
use linux_whisper_core::model::Segment;
use linux_whisper_whisper::engine::TranscribeOptions;
use linux_whisper_whisper::worker::{LiveContext, WhisperWorker};

/// The app asks for an update at most this often.
const UPDATE_INTERVAL: Duration = Duration::from_millis(100);

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("LW_DEBUG").is_some() {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .init();
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    let [model, file, rest @ ..] = args.as_slice() else {
        eprintln!("usage: live <model.bin> <audio file> [language | en,pl,de]");
        std::process::exit(2);
    };

    let worker = WhisperWorker::new();
    worker.load_model(PathBuf::from(model)).await?;

    let audio = decode_to_mono_16khz(&PathBuf::from(file))?;
    let options = match rest.first() {
        Some(list) if list.contains(',') => TranscribeOptions {
            languages: list.split(',').map(str::to_string).collect(),
            ..Default::default()
        },
        language => TranscribeOptions {
            language: language.cloned(),
            ..Default::default()
        },
    };

    let rate = AudioBuffer::TARGET_SAMPLE_RATE as usize;
    let total = audio.samples.len();
    let format = FormatOptions::default();
    let mut start = 0;
    let mut done: Vec<Segment> = Vec::new();
    let mut context = LiveContext::default();
    let mut shown = String::new();
    let mut erased_total = 0;
    let mut heard = UPDATE_INTERVAL.as_secs_f64();

    loop {
        let end = ((heard * rate as f64) as usize).min(total);
        let finish = end == total;
        let began = Instant::now();
        let live = worker
            .transcribe_live(
                AudioBuffer {
                    samples: audio.samples[start..end].to_vec(),
                    sample_rate: AudioBuffer::TARGET_SAMPLE_RATE,
                },
                options.clone(),
                context,
                finish,
            )
            .await?;
        let took = began.elapsed();

        let offset = start as f64 / rate as f64;
        let moved = |mut segments: Vec<Segment>| {
            for segment in &mut segments {
                segment.start += offset;
                segment.end += offset;
            }
            segments
        };
        done.extend(moved(live.done_segments));
        let guess = moved(live.guess);
        start += live.done;
        context = live.context;

        let all: Vec<Segment> = done.iter().chain(&guess).cloned().collect();
        let text = format_typed_segments(&all, &format);
        let same = shown
            .chars()
            .zip(text.chars())
            .take_while(|(a, b)| a == b)
            .count();
        let erased = shown.chars().count() - same;
        erased_total += erased;
        if text != shown || finish {
            println!(
                "{:6.2}s  took {:4.2}s  final to {:6.2}s  erase {erased:3}  | {text}",
                end as f64 / rate as f64,
                took.as_secs_f64(),
                start as f64 / rate as f64,
            );
        }
        shown = text;

        if finish {
            println!(
                "\nfinal text {:.2}s after the end, {erased_total} characters erased in all:\n{shown}",
                took.as_secs_f64()
            );
            for seg in &done {
                println!(
                    "[{:7.2} - {:7.2}] {:>2}  {}",
                    seg.start,
                    seg.end,
                    seg.language.as_deref().unwrap_or("-"),
                    seg.text
                );
            }
            return Ok(());
        }
        heard = (end as f64 / rate as f64) + took.max(UPDATE_INTERVAL).as_secs_f64();
    }
}
