//! PostgreSQL implementation of the station repository boundary.

use async_trait::async_trait;
use sqlx::{FromRow, PgPool, postgres::PgPoolOptions};

use crate::search::{
    RankedStation, RepositoryError, SearchConstraints, SearchQuery, Station, StationHealth,
    StationRepository,
};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();

// The SQL mirrors the domain's exact-token matching, hard filters, score formula, stable tie-break,
// and post-ranking limit. Query normalization and the meaning of these rules remain domain-owned.
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
    WHERE ($3::text IS NULL OR s.language = $3)
      AND ($4::text IS NULL OR s.country_code = $4)
      AND NOT (s.id = ANY($5::text[]))
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
    matched_count::float8 / GREATEST(cardinality($1::text[]) + cardinality($2::text[]), 1) AS score
FROM candidates
WHERE matched_count > 0
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
    ) -> Result<Vec<RankedStation>, RepositoryError> {
        let parameters = PostgresSearchParameters::from_domain(query, constraints)
            .map_err(|error| RepositoryError::new("parameter conversion", error))?;
        let rows = sqlx::query_as::<_, StationRow>(SEARCH_SQL)
            .bind(&parameters.terms)
            .bind(&parameters.tags)
            .bind(&parameters.language)
            .bind(&parameters.country_code)
            .bind(&parameters.excluded_station_ids)
            .bind(parameters.limit)
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

        Ok(Self {
            reason: format!("Matched catalog metadata with score {:.3}.", row.score),
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

#[derive(Debug, Eq, PartialEq)]
struct PostgresSearchParameters {
    terms: Vec<String>,
    tags: Vec<String>,
    language: Option<String>,
    country_code: Option<String>,
    excluded_station_ids: Vec<String>,
    limit: i64,
}

impl PostgresSearchParameters {
    /// Copies normalized domain values into SQL-safe owned parameters without changing meaning.
    fn from_domain(
        query: &SearchQuery,
        constraints: &SearchConstraints,
    ) -> Result<Self, ParameterConversionError> {
        Ok(Self {
            terms: query.terms.clone(),
            tags: query.tags.clone(),
            language: query.language.clone(),
            country_code: query.country_code.clone(),
            excluded_station_ids: constraints.excluded_station_ids.iter().cloned().collect(),
            limit: i64::try_from(constraints.limit)
                .map_err(|_| ParameterConversionError::LimitTooLarge)?,
        })
    }
}

#[derive(Debug)]
enum ParameterConversionError {
    LimitTooLarge,
}

impl std::fmt::Display for ParameterConversionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("search limit cannot fit in PostgreSQL bigint")
    }
}

impl std::error::Error for ParameterConversionError {}

#[cfg(test)]
mod tests {
    use super::{PostgresSearchParameters, StationRow};
    use crate::search::{RankedStation, SearchConstraints, StationHealth, normalize_query};
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

        let parameters = PostgresSearchParameters::from_domain(&query, &constraints).unwrap();

        assert_eq!(parameters.country_code.as_deref(), Some("GB"));
        assert_eq!(parameters.language.as_deref(), Some("en"));
        assert_eq!(parameters.limit, 7);
        assert_eq!(
            parameters.excluded_station_ids,
            ["station-rock-001", "station-rock-002"]
        );
    }
}
