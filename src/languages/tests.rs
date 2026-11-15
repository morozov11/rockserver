//! Unit tests for language modules, loader, registry, intersection, and resolution chain.

use std::{
    collections::{BTreeSet, HashSet},
    fs,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use super::{
    intersection::{FakeServiceLanguageSource, LanguageIntersectionService, ServiceLanguageStatus},
    loader::{DEFAULT_LANGUAGES_DIR, LanguageLoader},
    resolution::{extract_base_language, resolve_communication_language},
};

struct TestTempDir {
    path: PathBuf,
}

impl TestTempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "rockserver_lang_test_{}_{}",
            name,
            uuid::Uuid::new_v4()
        ));
        let _ = fs::create_dir_all(&path);
        Self { path }
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TestTempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn default_language_modules_load_and_validate_successfully() {
    let registry = LanguageLoader::load_from_directory(std::path::Path::new(DEFAULT_LANGUAGES_DIR))
        .expect("bundled languages must load successfully");

    assert!(registry.is_valid("en"));
    assert!(registry.is_valid("ru"));

    let en = registry.get("en").unwrap();
    assert_eq!(en.code, "en");
    assert_eq!(en.display_name, "English");
    assert!(!en.lexical.stop_words.is_empty());

    let ru = registry.get("ru").unwrap();
    assert_eq!(ru.code, "ru");
    assert_eq!(ru.display_name, "Русский");
    assert!(!ru.lexical.stop_words.is_empty());
}

#[test]
fn invalid_module_code_mismatch_is_rejected() {
    let temp_dir = TestTempDir::new("mismatch");
    let temp_path = temp_dir.path();

    // Copy valid en.yaml so invariant holds
    fs::copy("languages/en.yaml", temp_path.join("en.yaml")).unwrap();

    // Create a bad module where file is de.yaml but code is "fr"
    let bad_content = fs::read_to_string("languages/en.yaml")
        .unwrap()
        .replace("\"code\": \"en\"", "\"code\": \"fr\"");
    fs::write(temp_path.join("de.yaml"), bad_content).unwrap();

    let registry = LanguageLoader::load_from_directory(temp_path).unwrap();

    // en is valid, de was rejected because of code mismatch
    assert!(registry.is_valid("en"));
    assert!(!registry.is_valid("de"));
    assert!(!registry.is_valid("fr"));
}

#[test]
fn invalid_module_with_unknown_section_is_rejected() {
    let temp_dir = TestTempDir::new("unknown_section");
    let temp_path = temp_dir.path();

    fs::copy("languages/en.yaml", temp_path.join("en.yaml")).unwrap();

    let mut bad_content = fs::read_to_string("languages/en.yaml").unwrap();
    bad_content.push_str("\n\"unknown_section\": true\n");
    fs::write(temp_path.join("ru.yaml"), bad_content).unwrap();

    let registry = LanguageLoader::load_from_directory(temp_path).unwrap();

    assert!(registry.is_valid("en"));
    assert!(!registry.is_valid("ru"));
}

#[test]
fn missing_en_module_fails_startup() {
    let temp_dir = TestTempDir::new("missing_en");
    let temp_path = temp_dir.path();

    // Copy only ru.yaml
    fs::copy("languages/ru.yaml", temp_path.join("ru.yaml")).unwrap();

    let err = LanguageLoader::load_from_directory(temp_path).unwrap_err();
    assert!(
        err.message
            .contains("mandatory English ('en') language module is missing")
    );
}

#[test]
fn intersection_unconnected_stubs_preserve_rockserver_languages() {
    let registry = Arc::new(
        LanguageLoader::load_from_directory(std::path::Path::new(DEFAULT_LANGUAGES_DIR)).unwrap(),
    );
    let llm = Arc::new(FakeServiceLanguageSource::new(
        ServiceLanguageStatus::NotConnected,
    ));
    let stt = Arc::new(FakeServiceLanguageSource::new(
        ServiceLanguageStatus::NotConnected,
    ));

    let intersection = LanguageIntersectionService::new(registry, llm, stt);
    let published = intersection.compute_intersection(
        &ServiceLanguageStatus::NotConnected,
        &ServiceLanguageStatus::NotConnected,
    );

    let codes: Vec<_> = published.into_iter().map(|s| s.code).collect();
    assert_eq!(codes, vec!["en", "ru"]);
}

#[test]
fn intersection_restricts_to_common_available_languages() {
    let registry = Arc::new(
        LanguageLoader::load_from_directory(std::path::Path::new(DEFAULT_LANGUAGES_DIR)).unwrap(),
    );
    let llm = Arc::new(FakeServiceLanguageSource::new(
        ServiceLanguageStatus::Available(BTreeSet::from(["en".into(), "ru".into(), "de".into()])),
    ));
    let stt = Arc::new(FakeServiceLanguageSource::new(
        ServiceLanguageStatus::Available(BTreeSet::from(["en".into(), "de".into()])),
    ));

    let intersection = LanguageIntersectionService::new(registry, llm, stt);
    let published = intersection.compute_intersection(
        &ServiceLanguageStatus::Available(BTreeSet::from(["en".into(), "ru".into(), "de".into()])),
        &ServiceLanguageStatus::Available(BTreeSet::from(["en".into(), "de".into()])),
    );

    // de is not in rockserver, ru is not in stt -> only en survives
    let codes: Vec<_> = published.into_iter().map(|s| s.code).collect();
    assert_eq!(codes, vec!["en"]);
}

#[test]
fn intersection_unknown_service_enforces_en_exception() {
    let registry = Arc::new(
        LanguageLoader::load_from_directory(std::path::Path::new(DEFAULT_LANGUAGES_DIR)).unwrap(),
    );
    let llm = Arc::new(FakeServiceLanguageSource::new(
        ServiceLanguageStatus::Available(BTreeSet::from(["en".into(), "ru".into()])),
    ));
    let stt = Arc::new(FakeServiceLanguageSource::new(
        ServiceLanguageStatus::Unknown,
    ));

    let intersection = LanguageIntersectionService::new(registry, llm, stt);
    let published = intersection.compute_intersection(
        &ServiceLanguageStatus::Available(BTreeSet::from(["en".into(), "ru".into()])),
        &ServiceLanguageStatus::Unknown,
    );

    // When STT is Unknown, only en is published (contract §2 en-exception)
    let codes: Vec<_> = published.into_iter().map(|s| s.code).collect();
    assert_eq!(codes, vec!["en"]);
}

#[tokio::test]
async fn polling_refreshes_unknown_status_with_retry_pause() {
    let registry = Arc::new(
        LanguageLoader::load_from_directory(std::path::Path::new(DEFAULT_LANGUAGES_DIR)).unwrap(),
    );
    let llm = Arc::new(FakeServiceLanguageSource::new(
        ServiceLanguageStatus::Unknown,
    ));
    let stt = Arc::new(FakeServiceLanguageSource::new(
        ServiceLanguageStatus::NotConnected,
    ));

    let intersection = LanguageIntersectionService::with_retry_pause(
        registry,
        llm.clone(),
        stt.clone(),
        Duration::from_millis(50),
    );

    intersection.refresh().await;
    let published = intersection.published_languages().await;
    assert_eq!(published.len(), 1);
    assert_eq!(published[0].code, "en");

    // Service recovers
    llm.set_status(ServiceLanguageStatus::Available(BTreeSet::from([
        "en".into(),
        "ru".into(),
    ])))
    .await;

    intersection.refresh().await;
    let published = intersection.published_languages().await;
    let codes: Vec<_> = published.into_iter().map(|s| s.code).collect();
    assert_eq!(codes, vec!["en", "ru"]);
}

#[test]
fn resolution_chain_follows_documented_priority() {
    let published: HashSet<String> = HashSet::from(["en".to_owned(), "ru".to_owned()]);

    // 1. Account language takes precedence
    assert_eq!(
        resolve_communication_language(Some("ru"), Some("en"), Some("en-US"), &published),
        "ru"
    );

    // 2. Inactive or unknown account language falls back to request language
    assert_eq!(
        resolve_communication_language(Some("de"), Some("ru"), Some("en-US"), &published),
        "ru"
    );

    // 3. No account, no request language -> locale base language
    assert_eq!(
        resolve_communication_language(None, None, Some("ru-RU"), &published),
        "ru"
    );
    assert_eq!(
        resolve_communication_language(None, None, Some("en_GB"), &published),
        "en"
    );

    // 4. Unsupported locale -> falls back to en
    assert_eq!(
        resolve_communication_language(None, None, Some("ja-JP"), &published),
        "en"
    );

    // 5. Everything missing -> en
    assert_eq!(
        resolve_communication_language(None, None, None, &published),
        "en"
    );
}

#[test]
fn extract_base_language_handles_various_locale_shapes() {
    assert_eq!(extract_base_language("ru-RU"), "ru");
    assert_eq!(extract_base_language("en_US"), "en");
    assert_eq!(extract_base_language("RU"), "ru");
    assert_eq!(extract_base_language("en-GB-oed"), "en");
    assert_eq!(extract_base_language(""), "en");
}
