//! Run the AI formatter from the command line: downloads the model into
//! `<dir>` if needed (verifying its SHA-256), then formats each argument.
//!
//! ```text
//! cargo run --release -p linux-whisper-llm --example format -- \
//!     ~/.local/share/linux-whisper/llm-models "so first buy milk second call mom"
//! ```

use std::path::PathBuf;
use std::time::Instant;

use linux_whisper_llm::model_manager::LlmModelManager;
use linux_whisper_llm::model_registry::{model_file, tokenizer_file};
use linux_whisper_llm::worker::LlmWorker;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [dir, texts @ ..] = args.as_slice() else {
        eprintln!("usage: format <model dir> <text>...");
        std::process::exit(2);
    };

    let mgr = LlmModelManager::new(PathBuf::from(dir));
    if !mgr.is_ready() {
        println!("downloading model files...");
        mgr.download_all(None).await?;
    }

    let worker = LlmWorker::new();
    worker
        .load_model(mgr.file_path(model_file()), mgr.file_path(tokenizer_file()))
        .await?;

    for text in texts {
        let started = Instant::now();
        let result = worker.format_text(text.clone()).await;
        println!("--- input:  {text}");
        match result {
            Ok(out) => println!(
                "--- output ({:.1}s):\n{out}",
                started.elapsed().as_secs_f64()
            ),
            Err(e) => println!("--- error: {e}"),
        }
    }
    Ok(())
}
