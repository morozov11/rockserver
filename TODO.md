# Near-term TODO

1. Repository hygiene and documentation
   - Acceptance: Rust edition remains 2024; local build, IDE, and environment artifacts are ignored; repository purpose and boundaries are documented; formatting, linting, and tests pass.

2. HTTP service skeleton
   - Add Axum routing, health endpoints, structured tracing, graceful shutdown, and router-level tests.
   - Acceptance: readiness and liveness endpoints return documented responses; shutdown is graceful; router tests require no network service; all required checks pass.

3. Search API contract
   - Add `api/openapi.yaml` with `POST /v1/search`, request/response schemas, examples, and the standard error shape.
   - Acceptance: the contract defines validation and status codes, includes `code`, `message`, `request_id`, and `details` for errors, and is covered by a contract validation check.

4. Deterministic in-memory search
   - Introduce separate domain models and HTTP DTOs, a small in-memory catalog, explicit filters, and stable ranking/tie-breaking.
   - Acceptance: identical inputs produce identical ordering; DTO mapping is tested; invalid filters return contract-compliant errors; tests use neither external network nor AI providers.
