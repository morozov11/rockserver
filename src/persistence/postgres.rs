//! PostgreSQL implementation of the station repository boundary.

use async_trait::async_trait;
use sqlx::{FromRow, PgPool, postgres::PgPoolOptions};

use crate::search::{
    Embedding, METADATA_WEIGHT, RankedStation, RepositoryError, SEMANTIC_WEIGHT, SearchConstraints,
    SearchQuery, Station, StationHealth, StationRepository,
};

use super::embedding_postgres::vector_literal;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();

// Hard filters and exclusions are applied in `candidates`, before scoring and the final limit.
// Metadata fallback remains exact; compatible embeddings add an exact cosine-derived score.
const SEARCH_SQL: &str = r#"
WITH candidates AS (
    SELECT
        s.id,
        s.name,
        primary_stream.stream_url,
        s.homepage_url,
        s.tags,
        s.language,
        s.country_code,
        primary_stream.codec,
        primary_stream.bitrate_kbps,
        primary_stream.health,
        station_embedding.embedding,
        (
            SELECT COUNT(DISTINCT query_term.term)
            FROM UNNEST($1::text[]) AS query_term(term)
            WHERE EXISTS (
                SELECT 1
                FROM (
                    SELECT token
                    FROM regexp_split_to_table(lower(s.name), '[^[:alnum:]]+') AS token
                    UNION
                    SELECT token
                    FROM UNNEST(s.tags) AS station_tag(tag)
                    CROSS JOIN LATERAL regexp_split_to_table(lower(station_tag.tag), '[^[:alnum:]]+') AS token
                ) AS searchable_terms
                WHERE searchable_terms.token <> ''
                  AND searchable_terms.token = query_term.term
            )
        ) + (
            SELECT COUNT(DISTINCT query_tag.tag)
            FROM UNNEST($2::text[]) AS query_tag(tag)
            WHERE query_tag.tag = ANY(s.tags)
        ) AS matched_count
    FROM stations AS s
    JOIN LATERAL (
        SELECT stream_url, codec, bitrate_kbps, health
        FROM station_streams
        WHERE station_id = s.id
        ORDER BY is_primary DESC, id ASC
        LIMIT 1
    ) AS primary_stream ON true
    LEFT JOIN station_embeddings AS station_embedding
      ON station_embedding.station_id = s.id
     AND station_embedding.model = $8
     AND station_embedding.version = $9
     AND station_embedding.dimension = $10
     AND $7::text IS NOT NULL
    WHERE ($3::text IS NULL OR s.language = $3)
      AND ($4::text IS NULL OR s.country_code = $4)
      AND NOT (s.id = ANY($5::text[]))
), scored AS (
    SELECT
        *,
        matched_count::float8
            / GREATEST(cardinality($1::text[]) + cardinality($2::text[]), 1)
            AS metadata_score,
        CASE
            WHEN embedding IS NULL OR $7::text IS NULL THEN NULL
            ELSE LEAST(1.0, GREATEST(0.0, 1.0 - (embedding <=> ($7::text)::vector) / 2.0))
        END AS semantic_score
    FROM candidates
)
SELECT
    id,
    name,
    stream_url,
    homepage_url,
    tags,
    language,
    country_code,
    codec,
    bitrate_kbps,
    health,
    metadata_score,
    semantic_score,
    CASE
        WHEN semantic_score IS NULL THEN metadata_score
        ELSE $11::float8 * metadata_score + $12::float8 * semantic_score
    END AS score
FROM scored
WHERE metadata_score > 0 OR semantic_score > 0
ORDER BY score DESC, id ASC
LIMIT $6
"#;

/// PostgreSQL-backed station catalog with startup migrations and seeded development data.
#[derive(Clone, Debug)]
pub struct PostgresStationRepository {
    pool: PgPool,
}

impl PostgresStationRepository {
    /// Connects to PostgreSQL and applies all pending versioned migrations.
    pub async fn connect(database_url: &str) -> Result<Self, RepositoryError> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect(database_url)
            .await
            .map_err(|error| RepositoryError::new("connection", error))?;

        if let Err(error) = MIGRATOR.run(&pool).await {
            pool.close().await;
            return Err(RepositoryError::new("migration", error));
        }

        Ok(Self { pool })
    }

    /// Closes this repository's shared connection pool after in-flight work completes.
    pub async fn close(&self) {
        self.pool.close().await;
    }
}

#[async_trait]
impl StationRepository for PostgresStationRepository {
    async fn search(
        &self,
        query: &SearchQuery,
        constraints: &SearchConstraints,
        embedding: Option<&Embedding>,
    ) -> Result<Vec<RankedStation>, RepositoryError> {
        let parameters = PostgresSearchParameters::from_domain(query, constraints, embedding)
            .map_err(|error| RepositoryError::new("parameter conversion", error))?;
        let rows = sqlx::query_as::<_, StationRow>(SEARCH_SQL)
            .bind(&parameters.terms)
            .bind(&parameters.tags)
            .bind(&parameters.language)
            .bind(&parameters.country_code)
            .bind(&parameters.excluded_station_ids)
            .bind(parameters.limit)
            .bind(&parameters.embedding)
            .bind(&parameters.embedding_model)
            .bind(&parameters.embedding_version)
            .bind(parameters.embedding_dimension)
            .bind(parameters.metadata_weight)
            .bind(parameters.semantic_weight)
            .fetch_all(&self.pool)
            .await
            .map_err(|error| RepositoryError::new("search", error))?;

        rows.into_iter()
            .map(RankedStation::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| RepositoryError::new("row conversion", error))
    }

    async fn check_readiness(&self) -> Result<(), RepositoryError> {
        sqlx::query_scalar::<_, i32>("SELECT 1")
            .fetch_one(&self.pool)
            .await
            .map(|_| ())
            .map_err(|error| RepositoryError::new("readiness check", error))
    }
}

#[derive(Debug, FromRow)]
struct StationRow {
    id: String,
    name: String,
    stream_url: String,
    homepage_url: Option<String>,
    tags: Vec<String>,
    language: Option<String>,
    country_code: Option<String>,
    codec: Option<String>,
    bitrate_kbps: Option<i32>,
    health: String,
    metadata_score: f64,
    semantic_score: Option<f64>,
    score: f64,
}

impl TryFrom<StationRow> for RankedStation {
    type Error = RowConversionError;

    fn try_from(row: StationRow) -> Result<Self, Self::Error> {
        let bitrate_kbps = row
            .bitrate_kbps
            .map(u32::try_from)
            .transpose()
            .map_err(|_| RowConversionError::InvalidBitrate)?;
        let health = match row.health.as_str() {
            "healthy" => StationHealth::Healthy,
            "degraded" => StationHealth::Degraded,
            "unknown" => StationHealth::Unknown,
            _ => return Err(RowConversionError::InvalidHealth(row.health)),
        };

        let reason = row.semantic_score.map_or_else(
            || {
                format!(
                    "Matched catalog metadata with score {:.3}.",
                    row.metadata_score
                )
            },
            |semantic_score| {
                format!(
                    "Hybrid match: metadata {:.3}, semantic {:.3}.",
                    row.metadata_score, semantic_score
                )
            },
        );

        Ok(Self {
            reason,
            score: row.score,
            station: Station {
                id: row.id,
                name: row.name,
                stream_url: row.stream_url,
                homepage_url: row.homepage_url,
                tags: row.tags,
                language: row.language,
                country_code: row.country_code,
                codec: row.codec,
                bitrate_kbps,
                health,
            },
        })
    }
}

#[derive(Debug)]
enum RowConversionError {
    InvalidBitrate,
    InvalidHealth(String),
}

impl std::fmt::Display for RowConversionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBitrate => formatter.write_str("database bitrate cannot fit in u32"),
            Self::InvalidHealth(value) => {
                write!(formatter, "unknown database health value {value:?}")
            }
        }
    }
}

impl std::error::Error for RowConversionError {}

#[derive(Debug, PartialEq)]
struct PostgresSearchParameters {
    terms: Vec<String>,
    tags: Vec<String>,
    language: Option<String>,
    country_code: Option<String>,
    excluded_station_ids: Vec<String>,
    limit: i64,
    embedding: Option<String>,
    embedding_model: Option<String>,
    embedding_version: Option<String>,
    embedding_dimension: Option<i32>,
    metadata_weight: f64,
    semantic_weight: f64,
}

impl PostgresSearchParameters {
    /// Copies normalized domain values into SQL-safe owned parameters without changing meaning.
    fn from_domain(
        query: &SearchQuery,
        constraints: &SearchConstraints,
        embedding: Option<&Embedding>,
    ) -> Result<Self, ParameterConversionError> {
        let embedding_dimension = embedding
            .map(|value| i32::try_from(value.provenance().dimension))
            .transpose()
            .map_err(|_| ParameterConversionError::EmbeddingDimensionTooLarge)?;
        Ok(Self {
            terms: query.terms.clone(),
            tags: query.tags.clone(),
            language: query.language.clone(),
            country_code: query.country_code.clone(),
            excluded_station_ids: constraints.excluded_station_ids.iter().cloned().collect(),
            limit: i64::try_from(constraints.limit)
                .map_err(|_| ParameterConversionError::LimitTooLarge)?,
            embedding: embedding.map(vector_literal),
            embedding_model: embedding.map(|value| value.provenance().model.clone()),
            embedding_version: embedding.map(|value| value.provenance().version.clone()),
            embedding_dimension,
            metadata_weight: METADATA_WEIGHT,
            semantic_weight: SEMANTIC_WEIGHT,
        })
    }
}

#[derive(Debug)]
enum ParameterConversionError {
    LimitTooLarge,
    EmbeddingDimensionTooLarge,
}

impl std::fmt::Display for ParameterConversionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LimitTooLarge => {
                formatter.write_str("search limit cannot fit in PostgreSQL bigint")
            }
            Self::EmbeddingDimensionTooLarge => {
                formatter.write_str("embedding dimension cannot fit in PostgreSQL integer")
            }
        }
    }
}

impl std::error::Error for ParameterConversionError {}

#[cfg(test)]
mod tests {
    use super::{PostgresSearchParameters, StationRow};
    use crate::search::{
        METADATA_WEIGHT, RankedStation, SEMANTIC_WEIGHT, SearchConstraints, StationHealth,
        normalize_query,
    };
    use std::collections::BTreeSet;

    #[test]
    fn database_row_conversion_maps_stream_fields_and_health() {
        let ranked = RankedStation::try_from(StationRow {
            id: "station-test".to_owned(),
            name: "Test Station".to_owned(),
            stream_url: "https://streams.example.com/test.mp3".to_owned(),
            homepage_url: None,
            tags: vec!["rock".to_owned()],
            language: Some("en".to_owned()),
            country_code: Some("US".to_owned()),
            codec: Some("MP3".to_owned()),
            bitrate_kbps: Some(192),
            health: "degraded".to_owned(),
            metadata_score: 0.5,
            semantic_score: None,
            score: 0.5,
        })
        .unwrap();

        assert_eq!(ranked.station.health, StationHealth::Degraded);
        assert_eq!(ranked.station.bitrate_kbps, Some(192));
        assert_eq!(ranked.score, 0.5);
    }

    #[test]
    fn domain_search_values_convert_to_stable_sql_parameters() {
        let query = normalize_query("british classic rock".to_owned(), "en-GB".to_owned());
        let constraints = SearchConstraints {
            limit: 7,
            excluded_station_ids: BTreeSet::from([
                "station-rock-002".to_owned(),
                "station-rock-001".to_owned(),
            ]),
        };

        let parameters = PostgresSearchParameters::from_domain(&query, &constraints, None).unwrap();

        assert_eq!(parameters.country_code.as_deref(), Some("GB"));
        assert_eq!(parameters.language.as_deref(), Some("en"));
        assert_eq!(parameters.limit, 7);
        assert_eq!(parameters.metadata_weight, METADATA_WEIGHT);
        assert_eq!(parameters.semantic_weight, SEMANTIC_WEIGHT);
        assert_eq!(
            parameters.excluded_station_ids,
            ["station-rock-001", "station-rock-002"]
        );
    }
}
