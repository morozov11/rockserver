# Task log

## RS-000 — 2026-08-13 — Repository bootstrap

- Goal: establish the Rust crate, repository hygiene, contributor guidance, project scope, architecture outline, and near-term roadmap.
- Scope: Rust edition 2024 bootstrap binary and documentation only.
- Result: completed by commit `4117786` (`Document RockServer project setup`); the commit added the crate files, ignore rules, contributor instructions, README, TODO, and architecture outline.
- Checks: the commit and resulting files were inspected for this log entry; historical command output is not available in Git and is therefore not claimed.
- Status: complete.

## RS-001 — 2026-08-13 — HTTP service skeleton

- Goal: introduce a minimal, testable Axum HTTP service with operational health endpoints.
- Scope: library plus thin binary, stable JSON health models, request and application tracing, configurable local listener, Ctrl+C graceful shutdown, in-memory router tests, and project documentation. Search, OpenAPI, persistence, containers, external providers, and client work are excluded.
- Result: added the Axum library application and thin binary, liveness and readiness routes with a shared stable serde model, JSON application tracing and HTTP request spans, `ROCKSERVER_BIND_ADDR` configuration with the local-only `127.0.0.1:3000` default, an extensible graceful-serving boundary with Ctrl+C as the current signal, and in-memory router tests. Updated the contributor rules and current project documentation.
- Checks: `cargo fmt --check` passed; `cargo clippy --all-targets --all-features -- -D warnings` passed; `cargo test` passed with 2 tests; `git diff --check` passed.
- Status: complete.

## RS-002 — 2026-08-13 — Search API contract and service diagrams

- Goal: define the public search contract before implementation and provide a clear offline architecture reference.
- Scope: OpenAPI 3.1 contract for `POST /v1/search` and existing health routes; validation rules, defaults, response/error schemas, status semantics, and Russian and English examples; a structural contract test; autonomous Russian HTML diagrams; contributor and project documentation. Search routing, catalog/domain implementation, persistence, providers, ranking, authentication, and client changes are excluded.
- Result: added `api/openapi.yaml` with the required schemas and 200/400/422/429/500 outcomes, explicitly separating malformed JSON (400) from well-formed but invalid input (422). Added a `cargo test` integration check using test-only `serde_yaml`; it parses the YAML and guards the required OpenAPI version, paths, POST operation, and schemas. Added `docs/service-diagrams.html`, which distinguishes CURRENT, NEXT, and FUTURE architecture and works without external resources. The Axum router was intentionally unchanged, so `POST /v1/search` remained 404 at that stage. Added permanent Rustdoc/comment guidance and a diagram-accuracy rule to `AGENTS.md`.
- Checks: `cargo fmt --check` passed; `cargo clippy --all-targets --all-features -- -D warnings` passed; `cargo test` passed with 4 tests, including an explicit assertion that `POST /v1/search` remained 404; `git diff --check` passed. Python's standard-library `html.parser` accepted the complete HTML file and additional local checks confirmed one doctype, one `<html>` root, no external resource URLs, and balanced SVG elements.
- Status: complete.

## RS-003 — 2026-08-13 — Deterministic in-memory station search

- Goal: make the contract-defined `POST /v1/search` route usable without persistence, providers, embeddings, LLMs, or external network access.
- Scope: Axum search routing and DTO validation; contract-compliant 400/422 errors with request IDs; a small built-in in-memory catalog; separated search domain, `StationRepository` trait, normalization, filtering, deterministic ranking, and result mapping; HTTP and domain tests; current-state documentation and diagrams. PostgreSQL, imports, external providers, semantic ranking, rate limiting, client changes, and authentication are excluded.
- Result: registered `POST /v1/search` alongside the unchanged health routes. The route accepts all existing request fields, defaults locale and limit, rejects malformed JSON with 400 and syntactically valid invalid input with 422, and always returns the standard `ErrorResponse` fields on errors. The domain applies inferred language/country and exclusion constraints to a six-station catalog, scores matching metadata, and breaks equal scores by station ID ascending. No OpenAPI changes were needed because the existing schema already described this behavior.
- Checks: `cargo fmt --check` passed; `cargo clippy --all-targets --all-features -- -D warnings` passed; `cargo test --all-targets --all-features` passed with 13 tests; `git diff --check` passed. Search tests cover success, empty results, limit, stable ordering/tie-break, exclusions, malformed JSON 400, and semantic validation 422. The suite uses no network listener, external network, AI provider, or LLM. A standard-library HTML parser accepted the updated service diagrams.
- Status: complete.

## RS-004 — 2026-08-13 — PostgreSQL persistence foundation

- Goal: add real PostgreSQL catalog storage behind `StationRepository` without changing the public search contract or introducing import, pgvector, LLM, authentication, or rate limiting.
- Scope: versioned station/stream migrations, constraints and indexes, idempotent development seed, SQLx PostgreSQL repository, automatic startup migrations, `DATABASE_URL` selection with in-memory fallback, backend logging without DSN disclosure, dependency-aware readiness, local PostgreSQL Compose, deterministic unit tests, real opt-in PostgreSQL integration coverage, OpenAPI readiness alignment, and current-state documentation.
- Result: added separate `stations` and `station_streams` tables with timestamps and catalog invariants; seeded the same six development stations as the fallback; implemented parameterized SQL for hard filters, exclusions, deterministic metadata scoring, score-descending/station-ID-ascending order, and limit; made search repository operations asynchronous; added startup backend selection and migrations; kept liveness process-only and made PostgreSQL readiness return 503/`not_ready` after dependency loss. The responsibility boundary is explicit: the domain owns normalized inputs and rule meaning, while each repository executes those rules efficiently in its storage model.
- Checks: Docker Engine 29.7.2 and Compose 5.3.1 were available. A dedicated `rockserver-rs004-test` project ran PostgreSQL 17 and the opt-in integration test passed for migrations, seed, HTTP search, exclusions, limit, tie-break, and readiness loss; only that project's container, network, and volume were removed afterward. `cargo fmt --check` passed; `cargo clippy --all-targets --all-features -- -D warnings` passed; `cargo test --all-targets --all-features` passed with 15 regular tests and the opt-in PostgreSQL test ignored by default; the same PostgreSQL test passed when explicitly run with `TEST_DATABASE_URL`; `git diff --check` passed; the standard-library HTML parser accepted `docs/service-diagrams.html`.
- Status: complete.

## RS-005 — 2026-08-13 — Keep Graphify artifacts local

- Goal: prevent generated Graphify output from being included in commits or pushed to GitHub while preserving the local knowledge graph.
- Scope: root ignore rule and Git-index cleanup only; no Graphify files were deleted from disk.
- Result: normalized `.gitignore` to one `/graphify-out/` rule and removed all staged Graphify artifacts from the Git index. No `graphify-out` path existed in `HEAD`, so no committed repository content required deletion.
- Checks: `git ls-files -- graphify-out` returned no paths; `git check-ignore -v` matched both root and nested Graphify artifacts; the local directory remained present with 43 files; `git diff --check -- .gitignore docs/status.md docs/tasks.md` passed.
- Status: complete.
