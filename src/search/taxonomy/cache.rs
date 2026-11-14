//! In-memory cached taxonomy provider with TTL and graceful degradation.

use std::env;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use super::{GenreTaxonomy, GenreTaxonomySource};

/// Default TTL for cached taxonomy snapshots (5 minutes).
pub const DEFAULT_TAXONOMY_TTL: Duration = Duration::from_secs(300);

/// Environment variable for configuring taxonomy cache TTL in seconds.
pub const TAXONOMY_TTL_ENV: &str = "ROCKSERVER_TAXONOMY_TTL_SECS";

/// Identifies the active taxonomy source for logs and metrics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaxonomySourceKind {
    /// Loaded from the database (PostgreSQL `genre_hierarchy`).
    Db,
    /// Fallback compiled-in defaults (`CANONICAL_TAGS`).
    Fallback,
}

impl TaxonomySourceKind {
    /// String label for logging and metrics.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Db => "db",
            Self::Fallback => "fallback",
        }
    }
}

#[derive(Clone, Debug)]
struct CachedState {
    taxonomy: GenreTaxonomy,
    source_kind: TaxonomySourceKind,
    loaded_at: Instant,
}

/// Thread-safe in-memory cache for the controlled genre taxonomy.
///
/// Prevents per-query database round-trips by caching the loaded taxonomy snapshot
/// with a configurable TTL, graceful degradation on transient database failures,
/// and fallback to compiled-in defaults when no database is available.
pub struct TaxonomyCache {
    source: Option<Arc<dyn GenreTaxonomySource>>,
    ttl: Duration,
    state: RwLock<Option<CachedState>>,
}

impl TaxonomyCache {
    /// Creates a new cache with an explicit source and TTL.
    pub fn new(source: Option<Arc<dyn GenreTaxonomySource>>, ttl: Duration) -> Self {
        Self {
            source,
            ttl,
            state: RwLock::new(None),
        }
    }

    /// Initializes a cache reading TTL from `ROCKSERVER_TAXONOMY_TTL_SECS`.
    pub fn from_env(source: Option<Arc<dyn GenreTaxonomySource>>) -> Self {
        let ttl = env::var(TAXONOMY_TTL_ENV)
            .ok()
            .and_then(|val| val.trim().parse::<u64>().ok())
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_TAXONOMY_TTL);
        Self::new(source, ttl)
    }

    /// Returns the underlying source, if configured.
    pub fn source(&self) -> Option<Arc<dyn GenreTaxonomySource>> {
        self.source.clone()
    }

    /// Returns the configured TTL.
    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// Returns the cached taxonomy or loads a fresh snapshot if expired or missing.
    pub async fn get_or_load(&self) -> GenreTaxonomy {
        // Fast path: read lock.
        {
            let guard = self.state.read().await;
            if let Some(ref state) = *guard
                && state.loaded_at.elapsed() < self.ttl
            {
                return state.taxonomy.clone();
            }
        }

        // Slow path: write lock with double-checked locking.
        let mut guard = self.state.write().await;
        if let Some(ref state) = *guard
            && state.loaded_at.elapsed() < self.ttl
        {
            return state.taxonomy.clone();
        }

        self.reload_under_lock(&mut guard).await
    }

    /// Forces a refresh of the cached taxonomy from the source.
    pub async fn refresh(&self) -> Result<GenreTaxonomy, String> {
        let mut guard = self.state.write().await;
        Ok(self.reload_under_lock(&mut guard).await)
    }

    /// Returns the active source kind of the cached taxonomy, if loaded.
    pub async fn source_kind(&self) -> Option<TaxonomySourceKind> {
        let guard = self.state.read().await;
        guard.as_ref().map(|s| s.source_kind)
    }

    async fn reload_under_lock(&self, guard: &mut Option<CachedState>) -> GenreTaxonomy {
        if let Some(ref source) = self.source {
            match source.load().await {
                Ok(taxonomy) if taxonomy.is_usable() => {
                    tracing::info!(
                        source = TaxonomySourceKind::Db.as_str(),
                        version = %taxonomy.version(),
                        tags = taxonomy.canonical_tags().len(),
                        prompt_tags = taxonomy.prompt_tags().len(),
                        "genre taxonomy active from database"
                    );
                    *guard = Some(CachedState {
                        taxonomy: taxonomy.clone(),
                        source_kind: TaxonomySourceKind::Db,
                        loaded_at: Instant::now(),
                    });
                    return taxonomy;
                }
                Ok(_) => {
                    tracing::warn!(
                        "genre taxonomy from database is unusable; using builtin fallback"
                    );
                }
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        "failed to load genre taxonomy from database; using builtin fallback"
                    );
                }
            }
        }

        // Fallback: use builtin defaults.
        let fallback = GenreTaxonomy::builtin();
        tracing::warn!(
            source = TaxonomySourceKind::Fallback.as_str(),
            version = %fallback.version(),
            tags = fallback.canonical_tags().len(),
            prompt_tags = fallback.prompt_tags().len(),
            "genre taxonomy active with builtin fallback"
        );
        *guard = Some(CachedState {
            taxonomy: fallback.clone(),
            source_kind: TaxonomySourceKind::Fallback,
            loaded_at: Instant::now(),
        });
        fallback
    }
}
