//! Startup loader and validator for language definition modules.

use std::{
    collections::BTreeMap,
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use jsonschema::Validator;
use serde_json::Value;

use super::{
    registry::LanguageRegistry,
    schema::{LanguageModule, LanguageValidationError},
};

/// Environment variable configuring the directory containing language definition files.
pub const LANGUAGES_DIR_ENV: &str = "ROCKSERVER_LANGUAGES_DIR";

/// Default directory for language definitions when the environment variable is unset.
pub const DEFAULT_LANGUAGES_DIR: &str = "languages";

/// JSON Schema for language modules compiled into the binary.
const SCHEMA_BYTES: &[u8] = include_bytes!("../../languages/schema.json");

/// Error produced when loading language modules fails fatally at startup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LanguageLoadError {
    /// Human-readable explanation of why module loading failed.
    pub message: String,
}

impl LanguageLoadError {
    /// Creates a new loading failure.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for LanguageLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl Error for LanguageLoadError {}

/// Loads and validates language definition modules from disk at server startup.
pub struct LanguageLoader;

impl LanguageLoader {
    /// Loads all valid language modules from the configured or default directory.
    ///
    /// If an individual module fails schema or semantic validation, it is disabled with an
    /// error log while other languages continue to load. The English (`en`) module must be
    /// valid and present; otherwise loading fails fatally.
    pub fn load_from_env_or_default() -> Result<LanguageRegistry, LanguageLoadError> {
        let dir = std::env::var(LANGUAGES_DIR_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(DEFAULT_LANGUAGES_DIR));
        Self::load_from_directory(&dir)
    }

    /// Loads language modules from a specific directory path.
    pub fn load_from_directory(dir: &Path) -> Result<LanguageRegistry, LanguageLoadError> {
        let schema_json: Value = serde_json::from_slice(SCHEMA_BYTES).map_err(|err| {
            LanguageLoadError::new(format!("invalid embedded language schema: {err}"))
        })?;
        let validator = Validator::new(&schema_json).map_err(|err| {
            LanguageLoadError::new(format!("failed to compile language JSON schema: {err}"))
        })?;

        if !dir.exists() || !dir.is_dir() {
            return Err(LanguageLoadError::new(format!(
                "language directory does not exist or is not a directory: {:?}",
                dir
            )));
        }

        let entries = fs::read_dir(dir).map_err(|err| {
            LanguageLoadError::new(format!(
                "failed to read language directory {:?}: {err}",
                dir
            ))
        })?;

        let mut loaded = BTreeMap::new();

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(err) => {
                    tracing::error!("Failed to read directory entry in {:?}: {}", dir, err);
                    continue;
                }
            };
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("yaml") {
                continue;
            }

            let file_stem = match path.file_stem().and_then(|stem| stem.to_str()) {
                Some(stem) => stem.to_owned(),
                None => continue,
            };

            match Self::load_single_module(&path, &file_stem, &validator) {
                Ok(module) => {
                    tracing::info!(
                        "Loaded language module: code='{}', name='{}'",
                        module.code,
                        module.display_name
                    );
                    loaded.insert(module.code.clone(), Arc::new(module));
                }
                Err(err) => {
                    tracing::error!(
                        "Language module disabled due to validation failure: file={:?}, error={}",
                        path,
                        err
                    );
                }
            }
        }

        // Invariant: en must be valid and loaded
        if !loaded.contains_key("en") {
            return Err(LanguageLoadError::new(
                "mandatory English ('en') language module is missing or invalid",
            ));
        }

        Ok(LanguageRegistry::new(loaded))
    }

    /// Loads and validates a single language module file.
    pub fn load_single_module(
        path: &Path,
        expected_code: &str,
        validator: &Validator,
    ) -> Result<LanguageModule, LanguageValidationError> {
        let raw = fs::read_to_string(path).map_err(|err| {
            LanguageValidationError::new(format!("failed to read file {:?}: {err}", path))
        })?;

        // 1. Parse into generic serde_json::Value for JSON schema validation
        let value: Value = serde_yaml::from_str(&raw).map_err(|err| {
            LanguageValidationError::new(format!("YAML syntax error in {:?}: {err}", path))
        })?;

        // 2. Validate against JSON schema
        if let Err(error) = validator.validate(&value) {
            return Err(LanguageValidationError::new(format!(
                "JSON schema validation failed for {:?}: {}",
                path, error
            )));
        }

        // 3. Strongly typed deserialization
        let module: LanguageModule = serde_yaml::from_str(&raw).map_err(|err| {
            LanguageValidationError::new(format!(
                "deserialization failed for {:?} (unexpected fields): {err}",
                path
            ))
        })?;

        // 4. Semantic validation (code matches file stem, etc.)
        module.validate_semantics(expected_code)?;

        Ok(module)
    }
}
