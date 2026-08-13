# Project status

Last updated: 2026-08-13

## Current state

Stage 0, repository bootstrap and contributor documentation, is complete as recorded by commit `4117786`. The crate remains on Rust edition 2024 and repository hygiene, purpose, boundaries, and required checks are documented.

Stage 1, the HTTP service skeleton, is complete in the current working tree. RockServer is a library plus a thin binary, serves JSON liveness and readiness endpoints, emits structured tracing with HTTP request spans, reads its listener address from configuration, and supports graceful Ctrl+C shutdown. Router tests execute in memory and open no TCP port.

## Configuration and behavior

- Listener: `ROCKSERVER_BIND_ADDR`, default `127.0.0.1:3000`.
- Logging filter: `RUST_LOG`, default `info` when unset or invalid.
- `GET /health/live`: HTTP 200 with `{"status":"ok"}`.
- `GET /health/ready`: HTTP 200 with `{"status":"ok"}`.

## Known limitations

There is no OpenAPI contract, search endpoint, persistence, catalog, LLM or embedding integration, external provider integration, authentication, or RockCast client integration. Readiness currently has no downstream dependencies to inspect. Shutdown currently listens only for Ctrl+C, while the serving boundary accepts any future shutdown signal.

## Verification

On 2026-08-13, `cargo fmt --check`, strict Clippy for all targets and features, `cargo test`, and `git diff --check` all completed successfully. The test suite contains two router-level tests; both passed without opening a network listener. Detailed results are recorded in `docs/tasks.md`.

## Next step

Define and validate `api/openapi.yaml` for `POST /v1/search`, including request, response, examples, validation behavior, status codes, and the standard error shape. Do not add search implementation in that contract-only stage.
