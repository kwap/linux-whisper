//! Internationalization support for Linux Whisper.
//!
//! This crate provides compile-time checked localization using
//! [i18n-embed](https://crates.io/crates/i18n-embed) and
//! [Project Fluent](https://projectfluent.org/).
//!
//! # Usage
//!
//! ```rust,ignore
//! use linux_whisper_i18n::{fl, LANGUAGE_LOADER};
//!
//! let label = fl!(LANGUAGE_LOADER, "record");
//! ```

use i18n_embed::{
    fluent::{fluent_language_loader, FluentLanguageLoader},
    DesktopLanguageRequester, LanguageLoader,
};
use once_cell::sync::Lazy;
use rust_embed::RustEmbed;

// Re-export for use by other crates.
pub use i18n_embed;
pub use i18n_embed_fl::fl;

/// Embedded Fluent translation files from the `i18n/` directory.
#[derive(RustEmbed)]
#[folder = "../../i18n/"]
struct Localizations;

/// Global language loader, lazily initialized.
///
/// On first access the loader reads the user's desktop language preferences
/// (via `LANGUAGE` / `LC_ALL` / etc.) and selects the best available locale.
pub static LANGUAGE_LOADER: Lazy<FluentLanguageLoader> = Lazy::new(|| {
    let loader: FluentLanguageLoader = fluent_language_loader!();
    let requested_languages = DesktopLanguageRequester::requested_languages();
    let _result = i18n_embed::select(&loader, &Localizations, &requested_languages);
    loader
});

/// Switch the active locale at runtime.
///
/// If `lang` cannot be parsed as a valid BCP-47 language identifier the
/// locale falls back to `en-US`.
///
/// # Examples
///
/// ```rust,ignore
/// linux_whisper_i18n::set_locale("es");
/// ```
pub fn set_locale(lang: &str) {
    let langid: unic_langid::LanguageIdentifier = lang
        .parse()
        .unwrap_or_else(|_| unic_langid::langid!("en-US"));
    let _ = i18n_embed::select(&*LANGUAGE_LOADER, &Localizations, &[langid]);
}

/// Return a list of all locales for which translation files are available.
///
/// The returned strings are BCP-47 language tags (e.g. `"en-US"`, `"es"`).
pub fn available_locales() -> Vec<String> {
    LANGUAGE_LOADER
        .available_languages(&Localizations)
        .unwrap_or_default()
        .iter()
        .map(|l| l.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loader_initializes_without_panic() {
        // Force the lazy static to initialize.
        let _ = &*LANGUAGE_LOADER;
    }

    #[test]
    fn available_locales_contains_en_us() {
        let locales = available_locales();
        assert!(
            locales.iter().any(|l| l == "en-US"),
            "expected en-US in available locales, got: {locales:?}"
        );
    }

    #[test]
    fn set_locale_does_not_panic() {
        set_locale("es");
        set_locale("en-US");
        // Invalid tag should fall back gracefully.
        set_locale("not-a-real-locale");
    }

    #[test]
    fn fl_macro_returns_locale_values() {
        // Tests run in parallel sharing the global LANGUAGE_LOADER, so we
        // test both locales in a single test to avoid race conditions.
        set_locale("en-US");
        assert_eq!(fl!(LANGUAGE_LOADER, "record"), "Record");

        set_locale("es");
        assert_eq!(fl!(LANGUAGE_LOADER, "record"), "Grabar");

        // Reset back to English.
        set_locale("en-US");
    }
}
