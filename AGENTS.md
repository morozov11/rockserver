# RockServer contributor instructions

## Purpose

RockServer is a Rust service that will turn natural-language radio requests into a ranked list of internet radio streams for the Windows RockCast client.

## Repository boundaries

- Keep this repository focused on the server API, search domain, persistence, provider abstractions, and operational concerns.
- Keep RockCast UI and playback changes in the RockCast repository.
- Do not combine server and client work in one change.
- Preserve RockCast's local station catalog as an offline fallback.

## Architecture rules

- `api/openapi.yaml` is the source of truth for the HTTP contract once it is introduced.
- Version public endpoints under `/v1`.
- Return errors with `code`, `message`, `request_id`, and `details`.
- Separate HTTP DTOs, domain models, persistence, and external providers.
- Use traits around query parsing and embeddings; tests must use deterministic fakes.
- An LLM may translate a request into structured filters, but must not scan the full catalog.
- Never commit secrets or local environment files.
- Unit tests must not call real LLMs or the external network.
- Keep changes small and aligned with one roadmap stage.
- Keep the crate on Rust edition 2024.

## Code comments

- All new or modified public functions, methods, types, and modules must have meaningful Rustdoc comments (`///` or `//!`).
- Complex private functions and methods must have a short comment explaining their purpose, invariants, non-obvious design choices, errors, or side effects.
- Do not add comments that merely restate an obvious line of code.
- When changing an existing method's behavior, verify that its comments remain accurate.
- Apply these rules to code created or substantially changed by the task; do not perform unrelated mass comment rewrites.

## Required checks

Run all of the following before handing off a change:

```text
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

Update the OpenAPI contract and relevant tests whenever a public HTTP behavior changes.

## Project documentation

- Every task that changes the project must update `docs/status.md` and `docs/tasks.md` before completion.
- `docs/status.md` records the current working state, current stage, known limitations, latest verification results, and next step.
- `docs/tasks.md` is a chronological task log recording each task's goal, scope, result, checks, and status.
- Documentation must describe verified, actual behavior rather than planned or assumed behavior.
- Do not create a separate document for every small task unless the task genuinely needs one.
- When architecture or the public API changes, check `docs/service-diagrams.html` for accuracy. Update it only to reflect verified current behavior or explicitly labeled planned state.

## graphify

This project has a knowledge graph at graphify-out/ with god nodes, community structure, and cross-file relationships.

When the user types `/graphify`, use the installed graphify skill or instructions before doing anything else.

Rules:
- For codebase questions, first run `graphify query "<question>"` when graphify-out/graph.json exists. Use `graphify path "<A>" "<B>"` for relationships and `graphify explain "<concept>"` for focused concepts. These return a scoped subgraph, usually much smaller than GRAPH_REPORT.md or raw grep output.
- Dirty graphify-out/ files are expected after hooks or incremental updates; dirty graph files are not a reason to skip graphify. Only skip graphify if the task is about stale or incorrect graph output, or the user explicitly says not to use it.
- If graphify-out/wiki/index.md exists, use it for broad navigation instead of raw source browsing.
- Read graphify-out/GRAPH_REPORT.md only for broad architecture review or when query/path/explain do not surface enough context.
- After modifying code, run `graphify update .` to keep the graph current (AST-only, no API cost).
