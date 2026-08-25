# RockMobile extended catalog release package

## Current release status

The checked local PostgreSQL `rockserver` catalog passed the fixed release gate on 2026-08-21 with
**16,825** active, playable, uniquely-primary stations. The verified release package is:

- `release/mobile-catalog/rockmobile-extended-2026.08.2-mobile.1.sqlite`
- `release/mobile-catalog/rockmobile-extended-2026.08.2-mobile.1.manifest.json`
- `release/mobile-catalog/rockmobile-extended-2026.08.2-mobile.1.eligibility-report.json`

The manifest's exact-file SHA-256 is
`ad469d405f177d7e476cf9b3d9985497d0e2c6132ac0f3ce14485f4eab402073`.
No partial catalog is used: a source below the 16,000-station gate produces only a truthful gap
report and no SQLite/manifest release.

## Release artifacts

When the queried PostgreSQL catalog passes the strict gate, the `export_mobile_catalog` command
creates these immutable sibling files under `release/mobile-catalog/`:

| File | Purpose |
| --- | --- |
| `rockmobile-extended-<version>.sqlite` | Prebuilt SQLite database for Room's `createFromAsset`. |
| `rockmobile-extended-<version>.manifest.json` | Catalog version, SQLite schema version, exact-file SHA-256, and station count. It contains no wall-clock timestamp. |
| `rockmobile-extended-<version>.eligibility-report.json` | Machine-readable inclusion policy and explicit server-only exclusions. |

The manifest hashes the exact SQLite file bytes, with lowercase hexadecimal SHA-256. Existing full
release files are never overwritten. A below-threshold rerun replaces only its diagnostic
`*.gap-report.json`, because that report describes the current database rather than a release.

## SQLite schema (schema version 1)

`PRAGMA user_version` is `1`; SQLite uses 4 KiB pages and a fixed application ID. All ordinary
tables use SQLite `STRICT` typing.

### `catalog_metadata`

Exactly one row:

| Column | Type | Meaning |
| --- | --- | --- |
| `catalog_version` | `TEXT NOT NULL` | Explicit mobile release identifier. |
| `schema_version` | `INTEGER NOT NULL` | `1`, matching `PRAGMA user_version`. |
| `station_count` | `INTEGER NOT NULL` | Number of exported `stations` rows. |

### `stations`

One selected, playable stream per canonical RockServer station:

| Column | Type | Meaning |
| --- | --- | --- |
| `station_id` | `TEXT PRIMARY KEY` | Stable canonical `stations.id`; never a synthesized mobile ID. |
| `source` / `source_station_id` | `TEXT NOT NULL` | Provider-scoped identity; unique together. |
| `name` | `TEXT NOT NULL` | Display name. |
| `normalized_name` | `TEXT NOT NULL` | Lowercased, whitespace-collapsed name for deterministic local lookup. |
| `tags_json` | `TEXT NOT NULL` | JSON array of sorted, normalized tags. |
| `normalized_tags` | `TEXT NOT NULL` | Space-separated sorted normalized tags for filtering/search. |
| `country_code`, `language` | `TEXT` | Optional discovery metadata. |
| `homepage_url`, `favicon_url` | `TEXT` | Optional presentation metadata. `favicon_url` is currently `NULL`: the server persistence model does not yet carry it. |
| `stream_url` | `TEXT NOT NULL` | The station's selected active primary HTTP(S) stream. |
| `codec` | `TEXT` | Optional stream codec. |
| `bitrate_kbps` | `INTEGER` | Optional stream bitrate. |

Indexes and local search support:

| Name | Definition / use |
| --- | --- |
| `sqlite_autoindex_stations_1` | Primary-key lookup by stable `station_id`. |
| `stations_source_identity_idx` | Unique `(source, source_station_id)` provider identity. |
| `stations_normalized_name_idx` | B-tree `(normalized_name, station_id)` for prefix/exact normalized-name queries. |
| `stations_normalized_tags_idx` | B-tree `(normalized_tags, station_id)` for deterministic tag/genre filtering. |
| `station_search` | FTS5 (`unicode61`, diacritic removal) over `normalized_name` and `normalized_tags`; `station_id` is unindexed stored content. |

For a normalized prefix lookup, query `stations.normalized_name`; for free-text name/tag/genre
lookup, join FTS results back to `stations` by `station_id`. RockMobile should preserve its own
ranking/UI policy and use `station_id ASC` as its deterministic final tie-break where needed.

```sql
SELECT station_id, name, stream_url
FROM stations
WHERE normalized_name LIKE :normalized_prefix || '%'
ORDER BY normalized_name, station_id;

SELECT stations.station_id, stations.name, stations.stream_url
FROM station_search
JOIN stations USING (station_id)
WHERE station_search MATCH :fts_query
ORDER BY bm25(station_search), stations.station_id;
```

## Strict release eligibility and privacy boundary

The exporter selects a row only when all conditions hold:

1. The PostgreSQL station is active (`stations.retired_at IS NULL`).
2. It has exactly one active primary stream (`station_streams.is_primary` and
   `station_streams.retired_at IS NULL`).
3. That primary stream URL is non-empty and begins with `http://` or `https://`.
4. The result is ordered by stable `stations.id`. No source/name/URL cross-provider merge occurs;
   provider coexistence is preserved through `(source, source_station_id)`.

The export's one selected stream is precisely that primary stream. This is a release-specific
dedupe decision, not a change to server stream ownership or the public HTTP DTO.

The SQLite database and its reports intentionally exclude health and probe status/history/errors,
embeddings, import runs, `last_import_run_id`, creation/update timestamps, credentials, and
provider-private operational metadata.

Before release the code runs `PRAGMA integrity_check`, validates `PRAGMA user_version`, verifies
the metadata and `stations` counts, verifies the FTS row count, and hashes the exact SQLite bytes.

## Creating and consuming a release

Use a PostgreSQL instance already populated through the approved server import workflows; this
command does not fetch providers, alter PostgreSQL, or bypass the 16,000-station release gate.

```powershell
$env:DATABASE_URL='postgres://USER:PASSWORD@HOST:PORT/DATABASE'
$env:ROCKSERVER_MOBILE_EXPORT_VERSION='2026.08.2-mobile.1'
cargo run --bin export_mobile_catalog
```

Optionally set `ROCKSERVER_MOBILE_EXPORT_DIR` to another staging directory. The release command's
16,000-station gate is fixed and cannot be lowered with an environment variable.

After a successful release, verify before copying it to RockMobile:

```powershell
$base='release/mobile-catalog/rockmobile-extended-2026.08.2-mobile.1'
$manifest=Get-Content "$base.manifest.json" -Raw | ConvertFrom-Json
(Get-FileHash "$base.sqlite" -Algorithm SHA256).Hash.ToLowerInvariant() -eq $manifest.sha256
sqlite3 "$base.sqlite" 'PRAGMA integrity_check; PRAGMA user_version; SELECT station_count FROM catalog_metadata;'
```

The first command must return `True`; SQLite must print `ok`, `1`, and the same count as the
manifest. Copy the verified `.sqlite` file unchanged to the approved RockMobile asset path (for
example `src/main/assets/rockmobile-extended-<version>.sqlite`) and open it with Room's
`createFromAsset` flow. Do not unzip, rewrite, or migrate the bundled file before checksum
verification. RockMobile code remains intentionally out of scope here.

For the next release, populate a verified full catalog, choose a new explicit version, run the
same command into a clean staging directory, verify the manifest, then update the Android asset
reference in RockMobile. Retain the previous verified version to support rollback. If the source
catalog is below the gate, hand off the generated gap report instead; do not create a substitute
SQLite file.
