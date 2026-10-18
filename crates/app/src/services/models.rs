//! Model service — makes a whisper model (or the AI formatter) ready to use:
//! downloads it if needed, loads it into its worker, and reports progress to
//! the GTK main thread.

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::Arc;
use std::time::Duration;

use gtk::glib;
use tracing::{error, info};

use linux_whisper_core::config::AppConfig;
use linux_whisper_llm::model_manager::LlmModelManager;
use linux_whisper_llm::model_registry as llm_registry;
use linux_whisper_whisper::model_manager::ModelManager;
use linux_whisper_whisper::model_registry::WhisperModel;

use crate::context::{AppAction, AppContext};

/// Progress of making a model ready.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelEvent {
    /// Downloading; the fraction done (0.0–1.0).
    Downloading(f64),
    /// Downloaded; loading into memory.
    Loading,
    Ready,
    Failed(String),
}

thread_local! {
    /// Models being downloaded right now, so a second request for the same
    /// model does not start a second download into the same file.
    static IN_FLIGHT: RefCell<HashSet<&'static str>> = RefCell::new(HashSet::new());
}

/// Make `model` the active whisper model: save the choice, download the model
/// if it is not on disk yet, and load it. `on_event` runs on the GTK thread.
pub fn activate_model(
    ctx: &AppContext,
    model: &'static WhisperModel,
    on_event: impl Fn(ModelEvent) + 'static,
) {
    if let Err(e) = AppConfig::update(|c| c.model = model.name.to_string()) {
        error!("Failed to save model preference: {e}");
    }

    if !IN_FLIGHT.with(|f| f.borrow_mut().insert(model.name)) {
        on_event(ModelEvent::Failed(format!(
            "{} is already being downloaded",
            model.name
        )));
        return;
    }

    let (tx, rx) = std_mpsc::channel::<ModelEvent>();
    let worker = ctx.worker.clone();
    ctx.rt.spawn(async move {
        let mgr = ModelManager::new(AppConfig::models_dir());
        if !mgr.is_downloaded(model) {
            info!("Downloading model '{}'", model.name);
            let progress = Box::new(progress_reporter(tx.clone()));
            if let Err(e) = mgr.download(model, Some(progress)).await {
                error!("Model '{}' download failed: {e}", model.name);
                let _ = tx.send(ModelEvent::Failed(format!("Download failed: {e}")));
                return;
            }
        }
        let _ = tx.send(ModelEvent::Loading);
        let event = match worker.load_model(mgr.model_path(model)).await {
            Ok(()) => ModelEvent::Ready,
            Err(e) => ModelEvent::Failed(format!("Could not load the model: {e}")),
        };
        let _ = tx.send(event);
    });

    let ctx = ctx.clone();
    forward_events(rx, move |event| {
        if matches!(event, ModelEvent::Ready | ModelEvent::Failed(_)) {
            IN_FLIGHT.with(|f| f.borrow_mut().remove(model.name));
            ctx.send(AppAction::ModelChanged);
        }
        on_event(event);
    });
}

/// Make the AI formatter ready: download its model if needed and load it.
pub fn activate_formatter(ctx: &AppContext, on_event: impl Fn(ModelEvent) + 'static) {
    const NAME: &str = "ai-formatter";
    if !IN_FLIGHT.with(|f| f.borrow_mut().insert(NAME)) {
        on_event(ModelEvent::Failed(
            "The AI model is already being downloaded".into(),
        ));
        return;
    }

    let (tx, rx) = std_mpsc::channel::<ModelEvent>();
    let llm_worker = ctx.llm_worker.clone();
    ctx.rt.spawn(async move {
        let mgr = LlmModelManager::new(AppConfig::llm_models_dir());
        if !mgr.is_ready() {
            info!("Downloading the AI formatting model");
            let progress = Arc::new(progress_reporter(tx.clone()));
            if let Err(e) = mgr.download_all(Some(progress)).await {
                error!("AI formatting model download failed: {e}");
                let _ = tx.send(ModelEvent::Failed(format!("Download failed: {e}")));
                return;
            }
        }
        let _ = tx.send(ModelEvent::Loading);
        let model_path = mgr.file_path(llm_registry::model_file());
        let tokenizer_path = mgr.file_path(llm_registry::tokenizer_file());
        let event = match llm_worker.load_model(model_path, tokenizer_path).await {
            Ok(()) => ModelEvent::Ready,
            Err(e) => ModelEvent::Failed(format!("Could not load the AI model: {e}")),
        };
        let _ = tx.send(event);
    });

    forward_events(rx, move |event| {
        if matches!(event, ModelEvent::Ready | ModelEvent::Failed(_)) {
            IN_FLIGHT.with(|f| f.borrow_mut().remove(NAME));
        }
        on_event(event);
    });
}

/// Human-friendly progress text, e.g. "Downloading Base… 42%".
pub fn describe(event: &ModelEvent, what: &str) -> String {
    match event {
        ModelEvent::Downloading(frac) => {
            format!("Downloading {what}\u{2026} {:.0}%", frac * 100.0)
        }
        ModelEvent::Loading => format!("Loading {what}\u{2026}"),
        ModelEvent::Ready => format!("{what} is ready"),
        ModelEvent::Failed(e) => e.clone(),
    }
}

/// A download progress callback that sends at most one event per percent —
/// plenty for the UI.
fn progress_reporter(
    tx: std_mpsc::Sender<ModelEvent>,
) -> impl Fn(u64, u64) + Send + Sync + 'static {
    let last_percent = AtomicU32::new(u32::MAX);
    move |done, total| {
        let Some(percent) = (done * 100).checked_div(total) else {
            return;
        };
        if last_percent.swap(percent as u32, Ordering::Relaxed) != percent as u32 {
            let _ = tx.send(ModelEvent::Downloading(done as f64 / total as f64));
        }
    }
}

/// Deliver events from a background task to `on_event` on the GTK thread.
fn forward_events(rx: std_mpsc::Receiver<ModelEvent>, on_event: impl Fn(ModelEvent) + 'static) {
    glib::timeout_add_local(Duration::from_millis(100), move || loop {
        match rx.try_recv() {
            Ok(event) => {
                let finished = matches!(event, ModelEvent::Ready | ModelEvent::Failed(_));
                on_event(event);
                if finished {
                    return glib::ControlFlow::Break;
                }
            }
            Err(std_mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
            Err(std_mpsc::TryRecvError::Disconnected) => {
                on_event(ModelEvent::Failed(
                    "The download stopped unexpectedly".into(),
                ));
                return glib::ControlFlow::Break;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_progress() {
        assert_eq!(
            describe(&ModelEvent::Downloading(0.421), "Base"),
            "Downloading Base\u{2026} 42%"
        );
        assert_eq!(describe(&ModelEvent::Ready, "Base"), "Base is ready");
        assert_eq!(describe(&ModelEvent::Failed("boom".into()), "Base"), "boom");
    }
}
