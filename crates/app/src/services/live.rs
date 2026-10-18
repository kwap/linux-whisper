//! Live dictation: the text is typed while it is spoken and corrected as
//! recognition improves (see `linux_whisper_platform::live_typing`).

use std::collections::VecDeque;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use tracing::{info, warn};

use linux_whisper_audio::capture::AudioBuffer;
use linux_whisper_core::config::PasteShortcut;
use linux_whisper_core::format::{format_typed_segments, FormatOptions};
use linux_whisper_core::model::Segment;
use linux_whisper_platform::display::DisplayServer;
use linux_whisper_platform::hotkey::modifier_keys_held;
use linux_whisper_platform::live_typing::{dead_key_accent, KeyboardLayout, LiveTyping, Stroke};
use linux_whisper_whisper::engine::TranscribeOptions;
use linux_whisper_whisper::worker::{LiveContext, LiveText};

/// A dictation typed while it is spoken.
pub struct LiveDictation {
    pub id: u64,
    /// Sample rate of the recording.
    pub rate: u32,
    pub options: TranscribeOptions,
    pub format: FormatOptions,
    /// Where the audio without final text starts (in recorded samples).
    pub unfinished_from: usize,
    /// The final text, timed from the start of the recording.
    pub done: Vec<Segment>,
    /// The latest guess at the rest.
    pub guess: Vec<Segment>,
    pub context: LiveContext,
    /// An update is being worked out.
    pub busy: bool,
    /// How many samples had been recorded for the last update.
    pub heard: usize,
    /// The whole recording, once it ended while an update was being worked
    /// out: the final update follows that one.
    pub ended: Option<Vec<f32>>,
    /// The text last sent to be typed.
    pub shown: String,
}

impl LiveDictation {
    pub fn new(id: u64, rate: u32, options: TranscribeOptions, format: FormatOptions) -> Self {
        Self {
            id,
            rate,
            options,
            format,
            unfinished_from: 0,
            done: Vec::new(),
            guess: Vec::new(),
            context: LiveContext::default(),
            busy: false,
            heard: 0,
            ended: None,
            shown: String::new(),
        }
    }

    /// Take in an update for the audio from `unfinished_from` on.
    pub fn apply(&mut self, update: LiveText) {
        let offset = self.unfinished_from as f64 / f64::from(self.rate);
        let moved = |mut segments: Vec<Segment>| {
            for segment in &mut segments {
                segment.start += offset;
                segment.end += offset;
            }
            segments
        };
        self.done.extend(moved(update.done_segments));
        self.guess = moved(update.guess);
        // The update counts samples at 16 kHz.
        self.unfinished_from += (update.done as f64 * f64::from(self.rate)
            / f64::from(AudioBuffer::TARGET_SAMPLE_RATE)) as usize;
        self.context = update.context;
    }

    /// The text as it reads now.
    pub fn text(&self) -> String {
        let all: Vec<Segment> = self.done.iter().chain(&self.guess).cloned().collect();
        format_typed_segments(&all, &self.format)
    }
}

/// The user's keyboard layout as the desktop uses it, for typing on the
/// virtual keyboard. `None` if the desktop's keymap cannot be read.
pub fn keyboard_layout() -> Option<KeyboardLayout> {
    let display = gdk::Display::default()?;
    let keycodes: Vec<(u16, Vec<(gdk::KeymapKey, gdk::Key)>)> = (8..256)
        .filter_map(|code| Some((evdev_code(code)?, display.map_keycode(code)?)))
        .collect();
    let groups = keycodes
        .iter()
        .flat_map(|(_, keys)| keys.iter().map(|(k, _)| k.group()))
        .max()
        .map_or(1, |g| g + 1);
    let group = if groups > 1 { active_group() } else { 0 };

    let level3_key = display
        .map_keyval(gdk::Key::ISO_Level3_Shift)
        .unwrap_or_default()
        .into_iter()
        .filter(|k| k.group() == group)
        .min_by_key(|k| k.level())
        .and_then(|k| evdev_code(k.keycode()));
    let mut layout = KeyboardLayout::new(level3_key);

    // The simplest way to type a character wins: lower levels first.
    for level in 0..4 {
        for (code, keys) in &keycodes {
            for (key, keyval) in keys {
                if key.group() != group || key.level() != level {
                    continue;
                }
                let stroke = Stroke {
                    key: *code,
                    shift: level % 2 == 1,
                    level3: level >= 2,
                };
                let name = keyval.name();
                let name = name.as_deref().unwrap_or_default();
                if let Some(accent) = dead_key_accent(name) {
                    layout.add_dead_key(accent, stroke);
                } else if name.starts_with("KP_") {
                    // Keypad keys type differently in terminals.
                } else if let Some(ch) = keyval.to_unicode().filter(|c| !c.is_control()) {
                    layout.add_key(ch, stroke);
                }
            }
        }
    }
    info!(
        "Keyboard layout {group} of {groups}: {} characters, AltGr {}",
        layout.len(),
        if level3_key.is_some() { "yes" } else { "no" }
    );
    (!layout.is_empty()).then_some(layout)
}

/// evdev key code of an XKB key code.
fn evdev_code(xkb: u32) -> Option<u16> {
    u16::try_from(xkb.checked_sub(8)?).ok()
}

/// Which of several layouts is in use, if the desktop says (KDE does); the
/// first otherwise.
fn active_group() -> i32 {
    kde_active_layout().unwrap_or(0)
}

fn kde_active_layout() -> Option<i32> {
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).ok()?;
    let reply = bus
        .call_sync(
            Some("org.kde.keyboard"),
            "/Layouts",
            "org.kde.KeyboardLayouts",
            "getLayout",
            None,
            Some(glib::VariantTy::new("(u)").ok()?),
            gio::DBusCallFlags::NONE,
            200,
            gio::Cancellable::NONE,
        )
        .ok()?;
    let (index,) = reply.get::<(u32,)>()?;
    i32::try_from(index).ok()
}

/// How long typing waits for the user to let go of Ctrl, Alt, Shift or
/// Super (e.g. still held from the dictation shortcut).
const MODIFIER_WAIT: Duration = Duration::from_millis(1500);
const FINAL_MODIFIER_WAIT: Duration = Duration::from_secs(10);

enum Job {
    Begin {
        dictation: u64,
        layout: KeyboardLayout,
        paste: PasteShortcut,
        display: DisplayServer,
    },
    /// The text so far; it may still change.
    Show { dictation: u64, text: String },
    /// The final text; `done` gets the outcome once it is typed.
    Finish {
        dictation: u64,
        text: String,
        done: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
}

impl Job {
    fn dictation(&self) -> u64 {
        match self {
            Job::Begin { dictation, .. }
            | Job::Show { dictation, .. }
            | Job::Finish { dictation, .. } => *dictation,
        }
    }
}

/// Types live dictations on a thread of its own, one dictation after the
/// other. Typing a correction takes a moment; meanwhile newer text replaces
/// what was waiting to be typed.
pub struct Typist {
    jobs: mpsc::Sender<Job>,
}

impl Typist {
    pub fn new() -> Self {
        let (jobs, rx) = mpsc::channel();
        thread::Builder::new()
            .name("live-typing".into())
            .spawn(move || type_jobs(rx))
            .expect("failed to spawn the typing thread");
        Self { jobs }
    }

    pub fn begin(
        &self,
        dictation: u64,
        layout: KeyboardLayout,
        paste: PasteShortcut,
        display: DisplayServer,
    ) {
        let _ = self.jobs.send(Job::Begin {
            dictation,
            layout,
            paste,
            display,
        });
    }

    pub fn show(&self, dictation: u64, text: String) {
        let _ = self.jobs.send(Job::Show { dictation, text });
    }

    /// Type the final text, not waiting to hear how it went.
    pub fn end(&self, dictation: u64, text: String) {
        drop(self.finish(dictation, text));
    }

    /// Type the final text; the receiver gets the outcome.
    pub fn finish(
        &self,
        dictation: u64,
        text: String,
    ) -> tokio::sync::oneshot::Receiver<Result<(), String>> {
        let (done, rx) = tokio::sync::oneshot::channel();
        let _ = self.jobs.send(Job::Finish {
            dictation,
            text,
            done,
        });
        rx
    }
}

fn type_jobs(rx: mpsc::Receiver<Job>) {
    let mut typing: Option<(u64, LiveTyping)> = None;
    let mut queue: VecDeque<Job> = VecDeque::new();
    loop {
        if queue.is_empty() {
            match rx.recv() {
                Ok(job) => queue.push_back(job),
                Err(_) => return,
            }
        }
        queue.extend(rx.try_iter());
        let Some(job) = queue.pop_front() else {
            continue;
        };
        // Newer text for the same dictation makes this one stale.
        let stale = matches!(job, Job::Show { .. })
            && queue.iter().any(|next| next.dictation() == job.dictation());
        if stale {
            continue;
        }

        match job {
            Job::Begin {
                dictation,
                layout,
                paste,
                display,
            } => typing = Some((dictation, LiveTyping::new(layout, paste, display))),
            Job::Show { dictation, text } => {
                let Some((current, live)) = typing.as_mut() else {
                    continue;
                };
                if *current != dictation || !modifiers_released(MODIFIER_WAIT) {
                    continue;
                }
                if let Err(e) = live.show(&text) {
                    warn!("Live typing failed: {e}");
                }
            }
            Job::Finish {
                dictation,
                text,
                done,
            } => {
                let result = match typing.take() {
                    Some((current, mut live)) if current == dictation => {
                        if modifiers_released(FINAL_MODIFIER_WAIT) {
                            live.show(&text).map_err(|e| e.to_string())
                        } else {
                            Err("a modifier key is held down".to_string())
                        }
                    }
                    other => {
                        typing = other;
                        Err("the dictation was not being typed".to_string())
                    }
                };
                let _ = done.send(result);
            }
        }
    }
}

/// Wait up to `wait` until no modifier key is held. False if one still is.
fn modifiers_released(wait: Duration) -> bool {
    let until = Instant::now() + wait;
    while modifier_keys_held() {
        if Instant::now() >= until {
            return false;
        }
        thread::sleep(Duration::from_millis(20));
    }
    true
}
