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

Use Graphify only when the user explicitly invokes `/graphify` or when a genuinely large,
cross-repository architecture investigation needs relationship traversal that `rg` and targeted file
reading cannot provide. For routine code search, roadmap/status questions, concrete bugs, and
single-repository work, use `rg` and read only the relevant files.

Do not run `graphify update .` after ordinary changes. Never use Graphify merely because
`graphify-out/` exists or is dirty.

After every Graphify invocation, including failure, timeout, or interruption, inspect Python
processes created by that invocation. Gracefully stop only processes positively identified as
Graphify-owned (for example by command line containing `graphify`, its working directory, or a
Graphify helper), verify they exited, and never terminate unrelated Python processes.
