//! Language modules, localization rules, and communication language resolution.

mod intersection;
mod loader;
mod registry;
mod resolution;
mod schema;
pub mod translit;

#[cfg(test)]
mod tests;

pub use intersection::{
    FakeServiceLanguageSource, LanguageIntersectionService, LanguageSummary, ServiceLanguageSource,
    ServiceLanguageStatus, UnconnectedServiceLanguageSource,
};
pub use loader::{DEFAULT_LANGUAGES_DIR, LANGUAGES_DIR_ENV, LanguageLoadError, LanguageLoader};
pub use registry::LanguageRegistry;
pub use resolution::{extract_base_language, resolve_communication_language};
pub use schema::{LanguageModule, LanguageValidationError};
pub use translit::{
    is_cyrillic_char, is_cyrillic_token, is_latin_token, transliterate_lat_to_ru,
    transliterate_ru_to_lat,
};

static BUNDLED_REGISTRY: std::sync::LazyLock<LanguageRegistry> = std::sync::LazyLock::new(|| {
    LanguageLoader::load_from_env_or_default()
        .expect("bundled language modules must load successfully")
});

/// Returns a reference to the process-global bundled language registry.
pub fn bundled_registry() -> &'static LanguageRegistry {
    &BUNDLED_REGISTRY
}

/// Returns a clone wrapped in Arc of the process-global bundled language registry.
pub fn bundled_registry_arc() -> std::sync::Arc<LanguageRegistry> {
    std::sync::Arc::new(BUNDLED_REGISTRY.clone())
}
