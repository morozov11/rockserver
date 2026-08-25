# RM-004 — Shared station catalog implementation plan

Status: **RM-004-B completed; later implementation remains planned**  
Created: 2026-08-21  
Implementation: **not started**

Execution state:

- RM-004-A: **approved** on 2026-08-21;
- RM-004-B: **completed** on 2026-08-21 at `C:\repos\rockcast-station-catalog`;
- RM-004-C through RM-004-I: **not started**.

## Objective

Create one versioned, canonical baseline station catalog shared by RockServer, RockCast, and
RockMobile without making either client depend on a running RockServer.

RockServer remains the primary online catalog and the source of AI, voice, search, enrichment, and
personalization features. RockCast and RockMobile bundle a released snapshot of the same baseline
catalog. If RockServer is unavailable, local radio playback continues from that snapshot.

RockMobile additionally receives a versioned extended offline snapshot exported from RockServer's
PostgreSQL catalog. The current target is the complete eligible catalog (approximately 16,000
stations), searchable locally by station name and genre/tags. This snapshot supplements rather than
replaces the small curated baseline: baseline playback remains available on first launch even if
RockServer and the extended snapshot update channel are unavailable.

## Target architecture

```text
station-catalog repository
  ├── catalog/stations.v1.json
  ├── schema/stations.v1.schema.json
  ├── tools/validate
  ├── tools/convert-legacy-txt
  └── versioned release + checksum
          │
          ├── RockServer SharedCatalogImportProvider → PostgreSQL / in-memory bootstrap
          ├── RockCast bundled snapshot → local fallback
          └── RockMobile bundled snapshot → local fallback

RockServer PostgreSQL (provider-owned dynamic catalog)
  └── deterministic sanitized export + version/checksum
          └── RockMobile bundled Room/SQLite snapshot
                  └── local name + genre/tag search
```

The separate repository is the authoring source of truth. Each consumer keeps a pinned, validated
snapshot so clean and offline builds never require a sibling checkout or network access.

The PostgreSQL export is not a second authoring source for the canonical baseline. It is a derived,
replaceable product artifact owned by RockServer. It contains only fields needed for offline
discovery and playback; operational health history, probe errors/timestamps, embeddings, import-run
metadata, credentials, and provider-private fields remain server-only. Logos remain URLs and are
cached on demand rather than embedded for every station.

## Canonical v1 data shape

The contract should support a stable station identity and one or more stream identities:

```json
{
  "schemaVersion": 1,
  "catalogVersion": "2026.08.1",
  "stations": [
    {
      "id": "rock-antenne-heavy-metal",
      "name": "Rock Antenne — Heavy Metal",
      "tags": ["metal", "heavy metal"],
      "countryCode": "DE",
      "language": "de",
      "homepageUrl": "https://www.rockantenne.de/",
      "faviconUrl": null,
      "streams": [
        {
          "id": "primary-mp3",
          "url": "https://stream.rockantenne.de/heavy-metal/stream/mp3",
          "codec": "mp3",
          "bitrateKbps": 128,
          "primary": true
        }
      ]
    }
  ]
}
```

Contract invariants:

- `station.id` is explicit, globally unique inside the baseline catalog, human-reviewable, and
  remains unchanged when a name, URL, codec, or provider changes;
- `stream.id` is stable inside its station and does not derive solely from its URL;
- each station contains at least one stream and exactly one primary stream;
- station and stream IDs use documented lowercase ASCII syntax;
- tags are normalized, trimmed, non-empty, sorted, and deduplicated;
- country uses ISO 3166-1 alpha-2 and language uses a documented ISO 639 representation;
- playable URLs are HTTP(S); optional URLs are either valid HTTP(S) values or `null`;
- unknown optional fields are tolerated by consumers for forward-compatible v1 evolution;
- changes that reinterpret an existing field require a new schema version.

## Model allocation

OpenAI's current model guidance describes GPT-5.6 Sol as the frontier option and GPT-5.6 Terra as
the balanced intelligence/cost option. This plan uses Sol where decisions are difficult to reverse
and Terra where the contract is already fixed and work is bounded.

| Subplan | Model | Reasoning |
|---|---|---|
| RM-004-A — Contract and migration design | `gpt-5.6-sol` | `high` |
| RM-004-B — Catalog repository and validation tooling | `gpt-5.6-terra` | `high` |
| RM-004-C — Legacy data conversion and ID review | `gpt-5.6-terra` | `high` |
| RM-004-D — RockServer integration | `gpt-5.6-terra` | `high` |
| RM-004-E — RockMobile integration | `gpt-5.6-terra` | `medium` |
| RM-004-F — RockCast integration | `gpt-5.6-terra` | `medium` |
| RM-004-G — Release and synchronization automation | `gpt-5.6-terra` | `medium` |
| RM-004-H — Cross-project verification and architecture review | `gpt-5.6-sol` | `high` |
| RM-004-I — Cutover and legacy cleanup | `gpt-5.6-terra` | `medium` |

Each subplan should be a separate Codex task. A task must use only the model specified for that
subplan and must stop at its acceptance gate. Do not run implementation subplans in parallel until
RM-004-A is approved and RM-004-B has published the first immutable schema contract.

---

## RM-004-A — Contract and migration design

**Status:** approved 2026-08-21; design complete; no implementation performed.  
**Model:** `gpt-5.6-sol`  
**Reasoning:** `high`  
**Repository scope:** read all three existing repositories; write only design artifacts in the new
catalog repository or a temporary proposal location agreed before execution.

### Work

1. Inventory the current station fields, parsers, stable/derived identifiers, stream multiplicity,
   import ownership, API DTOs, persistence constraints, and fallback behavior in all three projects.
2. Produce the final JSON Schema, compatibility policy, ID policy, normalization policy, release
   policy, and migration mapping.
3. Decide how aliases and merges are represented when two legacy records refer to one station.
4. Decide whether `faviconUrl`, health, provider ownership, and discovered metadata are baseline
   authoring fields or RockServer-owned enrichment fields.
5. Identify every public/API/database behavior that must remain unchanged.

### Required outputs

- approved schema proposal;
- field mapping table for RockServer, RockCast, and RockMobile;
- ID stability and collision policy;
- compatibility and rollback strategy;
- explicit list of open decisions requiring human approval.

### Acceptance gate

No code implementation starts until schema v1, ID policy, ownership rules, and rollback strategy are
approved. A schema change after this gate must be treated as a reviewed contract revision.

---

## RM-004-B — Catalog repository and validation tooling

**Status:** completed 2026-08-21; acceptance gate passed.  
**Model:** `gpt-5.6-terra`  
**Reasoning:** `high`  
**Depends on:** RM-004-A approved.

### Work

1. Create the dedicated `station-catalog` repository with schema, canonical data, documentation,
   changelog, and ownership rules.
2. Implement deterministic validation and formatting tools.
3. Validate unique IDs, primary-stream rules, URL syntax, normalized metadata, and catalog version.
4. Add fixture-based tests for valid, invalid, and forward-compatible documents.
5. Produce a deterministic checksum and release artifact.

### Boundaries

- do not integrate consumers yet;
- do not invent station metadata not present in an authoritative source;
- do not depend on external network access for ordinary tests.

### Acceptance gate

A clean checkout can validate the catalog offline; repeated formatting produces byte-identical JSON;
invalid identities or ambiguous primary streams fail CI.

**Result:** the local `C:\repos\rockcast-station-catalog` repository contains the published final-name
v1 schema, a minimal non-provider sample catalog, strict authoring and forward-compatible-consumer
validation, deterministic formatter, SHA-256 manifest generator, documentation, ownership rules,
and fixture tests. No 41-station migration, consumer integration, network access, or derived
PostgreSQL catalog was introduced.

---

## RM-004-C — Legacy data conversion and stable-ID review

**Model:** `gpt-5.6-terra`  
**Reasoning:** `high`  
**Depends on:** RM-004-B.

### Work

1. Convert RockCast `stations.txt` into schema v1 without losing name, stream URL, tags, bitrate,
   codec, or country data.
2. Generate candidate station and stream IDs using a documented deterministic rule.
3. Produce a review report for collisions, duplicate URLs, duplicate brands, invalid values, and
   inferred country/language normalization.
4. Require a one-time human review of all generated IDs before declaring the first release.
5. Preserve an explicit legacy mapping so existing RockMobile URL-derived IDs can be migrated later
   without losing favourites/history when RM-007 is implemented.

### Acceptance gate

Station count and rejected-record count are explained; every canonical record passes validation;
IDs are approved and frozen; conversion is repeatable and produces no unexplained diff.

---

## RM-004-D — RockServer integration

**Model:** `gpt-5.6-terra`  
**Reasoning:** `high`  
**Repository:** RockServer  
**Depends on:** RM-004-C release candidate.

### Work

1. Add a provider-neutral shared-catalog adapter implementing the existing catalog import boundary.
2. Map `station.id` to `source_station_id` under source `rockcatalog`; map stream identity without
   deriving ownership solely from URL.
3. Upsert baseline stations and streams idempotently while preserving Radio Browser ownership,
   health, embeddings, probes, and import-run bookkeeping.
4. Replace the hard-coded development catalog with the pinned shared snapshot or an adapter over it.
5. Keep public HTTP/OpenAPI behavior compatible unless an independently approved contract change is
   required.
6. Add deterministic unit and PostgreSQL integration tests for repeat import, updates, collisions,
   multiple streams, rollback, and coexistence with provider-imported rows.
7. Add a deterministic export of all eligible active/playable PostgreSQL stations for RockMobile's
   extended offline catalog. Define deduplication, inclusion/exclusion, stable-ID, primary-stream,
   ordering, version, checksum, and rollback rules; exclude server-only operational fields.
8. Keep the extended export distinct from the curated canonical baseline and make generation an
   explicit release action, never an implicit client build dependency.

### Required checks

```text
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

### Acceptance gate

Repeated imports are idempotent; baseline and Radio Browser records coexist; search results expose
stable IDs and playable primary streams; the extended mobile export is deterministic and contains
no server-only operational data; server absence still has no effect on client snapshots.

---

## RM-004-E — RockMobile integration

**Model:** `gpt-5.6-terra`  
**Reasoning:** `medium`  
**Repository:** RockMobile  
**Depends on:** RM-004-C release candidate.

### Work

1. Replace the bundled legacy TXT parser with a schema-v1 JSON loader.
2. Consume explicit station IDs instead of hashing stream URLs.
3. Select the primary stream while keeping the domain/UI/playback interfaces as stable as practical.
4. Preserve RockServer-first loading and automatic bundled fallback for timeout, network, HTTP,
   malformed response, and empty remote result cases.
5. Add tests for schema parsing, missing optional fields, multiple streams, stable IDs, duplicate
   rejection, and full offline fallback.
6. Bundle the released extended catalog as a prebuilt Room/SQLite database so the full eligible
   PostgreSQL catalog (currently approximately 16,000 stations) is available on first launch without
   RockServer.
7. Query the database locally by normalized station name and genre/tags; add suitable indexes and
   verify that startup does not parse or retain the complete catalog as an in-memory JSON list.
8. Preserve the curated baseline as an independent last-resort fallback if the extended database is
   missing, corrupt, incompatible, or rejected by checksum/schema validation.
9. Store logo URLs only and cache images on demand. Do not bundle all station image bytes.

### Acceptance gate

RockMobile builds and plays the pinned snapshot with RockServer completely stopped; remote success
still uses RockServer; changing a stream URL does not change station identity; the full extended
snapshot can be searched locally by name and genre/tags with no network connection.

---

## RM-004-F — RockCast integration

**Model:** `gpt-5.6-terra`  
**Reasoning:** `medium`  
**Repository:** RockCast  
**Depends on:** RM-004-C release candidate.

### Work

1. Embed and parse the schema-v1 JSON snapshot.
2. Extend the local station model with stable ID and required optional metadata.
3. Preserve the environment/executable/app-data override behavior using the new JSON format.
4. Keep the legacy TXT parser only as a clearly marked transition adapter for one release cycle.
5. Preserve ordering, deduplication, playback, relay, Cast, and offline startup behavior.
6. Add tests for JSON parsing, overrides, multiple streams, compatibility conversion, and offline
   startup.

### Acceptance gate

RockCast uses the same catalog version and station IDs as RockMobile, runs offline, and preserves
current playback behavior. Legacy TXT support has a documented removal date.

---

## RM-004-G — Release and synchronization automation

**Model:** `gpt-5.6-terra`  
**Reasoning:** `medium`  
**Depends on:** RM-004-D, RM-004-E, and RM-004-F.

### Work

1. Publish immutable catalog releases containing JSON, schema, version, and checksum.
2. Add an explicit consumer update command that downloads or copies a selected release and verifies
   its checksum before replacing the vendored snapshot.
3. Ensure consumer builds never fetch the catalog implicitly.
4. Add CI drift checks comparing the vendored snapshot version/checksum with project metadata.
5. Document release, rollback, emergency stream replacement, and consumer update procedures.
6. Publish and checksum the extended RockMobile SQLite snapshot separately from the canonical
   baseline release, with an explicit reproducible command for rebuilding it from RockServer.

### Acceptance gate

Updating each consumer is one deterministic command plus a reviewed diff; ordinary offline builds
remain reproducible; a corrupt or mismatched artifact is rejected.

---

## RM-004-H — Cross-project verification and architecture review

**Model:** `gpt-5.6-sol`  
**Reasoning:** `high`  
**Depends on:** RM-004-G.

### Work

1. Review all three integrations against the approved schema and ID policy.
2. Compare the same sample stations across canonical JSON, RockServer API/database, RockCast, and
   RockMobile.
3. Test URL replacement, added secondary stream, metadata-only update, deleted station, duplicate
   provider record, server outage, rollback, and old-client compatibility.
4. Verify extended-snapshot record counts, deterministic bytes, local name/genre search, corrupt or
   incompatible snapshot fallback, first-launch offline behavior, and representative query latency.
5. Look specifically for identity churn, silent field loss, divergent normalization, accidental
   build-time network dependencies, and broken fallback behavior.
6. Produce severity-ranked findings; do not mix cleanup implementation into the review task.

### Acceptance gate

No unresolved high-severity compatibility or data-loss finding remains. Any accepted limitation has
an owner and follow-up task.

---

## RM-004-I — Cutover and legacy cleanup

**Model:** `gpt-5.6-terra`  
**Reasoning:** `medium`  
**Depends on:** RM-004-H approval.

### Work

1. Resolve approved review findings.
2. Remove obsolete duplicated snapshots, hard-coded stations, and transition converters only after
   their supported migration window ends.
3. Update project documentation and diagrams to identify the canonical catalog and snapshot flow.
4. Run full project checks and manual offline/online smoke scenarios.
5. Record final catalog version and checksum in all three projects.

### Acceptance gate

There is one authoring source, every consumer pins the same approved release, no manual catalog copy
remains, and both clients demonstrably play baseline radio without RockServer.

## Global execution rules

- Run subplans sequentially unless this document explicitly permits parallel work.
- Never let parallel client tasks redefine the schema independently.
- Each task begins by reading this plan and the target repository's `AGENTS.md`.
- Each task changes only its declared repository scope.
- Do not commit secrets, local database credentials, generated build outputs, or private provider
  responses.
- Do not report external/live checks as passed unless they were actually executed.
- Preserve the existing RockServer API and client fallback contracts unless a reviewed subplan says
  otherwise.
- Every implementation task must leave the target repository's tests and required static checks
  passing before handoff.

## Rollback strategy

- Catalog releases are immutable and consumers pin an explicit version plus checksum.
- A bad release is rolled back by pinning the prior release; published data is never silently
  rewritten.
- RockServer imports retain provider ownership and import-run history so a failed shared-catalog run
  can be identified without deleting unrelated provider data.
- RockCast and RockMobile keep the prior known-good snapshot available through version control.
- Legacy TXT support is removed only after the JSON path has shipped and been verified in both
  clients.

## Definition of RM-004 complete

RM-004 is complete only when:

1. one canonical versioned catalog and schema exist;
2. RockServer, RockCast, and RockMobile consume the same released baseline data and stable IDs;
3. consumer snapshot updates are automated and checksum-verified;
4. RockServer enrichment remains separate from baseline authoring data;
5. RockCast and RockMobile work offline with RockServer stopped;
6. cross-project compatibility tests and final Sol review have passed;
7. obsolete manual duplication has been removed or has an explicit, time-bounded migration reason.
8. RockMobile ships a checksum-verified extended offline snapshot of all eligible RockServer
   stations and supports local indexed search by name and genre/tags without RockServer.
