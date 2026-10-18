//! Transcribe an audio file from the command line — a quick way to check
//! decoding, language detection and model output without the GUI.
//!
//! ```text
//! cargo run --release -p linux-whisper-whisper --example transcribe -- \
//!     ~/.local/share/linux-whisper/models/ggml-base.bin recording.m4a [language]
//! ```
//!
//! Set `LW_DEBUG=1` to see how the audio was split and which language each
//! part was given.

use std::path::PathBuf;

use linux_whisper_audio::decode::decode_to_mono_16khz;
use linux_whisper_whisper::engine::TranscribeOptions;
use linux_whisper_whisper::worker::WhisperWorker;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("LW_DEBUG").is_some() {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .init();
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    let [model, file, rest @ ..] = args.as_slice() else {
        eprintln!("usage: transcribe <model.bin> <audio file> [language]");
        std::process::exit(2);
    };

    let worker = WhisperWorker::new();
    worker.load_model(PathBuf::from(model)).await?;

    let audio = decode_to_mono_16khz(&PathBuf::from(file))?;
    let options = TranscribeOptions {
        language: rest.first().cloned(),
        translate: false,
    };
    let transcript = worker.transcribe(audio, options).await?;

    println!(
        "language: {}",
        transcript.language.as_deref().unwrap_or("unknown")
    );
    for seg in &transcript.segments {
        let confidence = seg
            .confidence
            .map(|c| format!("{:.0}%", c * 100.0))
            .unwrap_or_else(|| "-".into());
        println!(
            "[{:7.2} - {:7.2}] {confidence:>4}  {}",
            seg.start, seg.end, seg.text
        );
    }
    Ok(())
}
