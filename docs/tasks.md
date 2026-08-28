# Task log

## REFACTOR-001 — 2026-08-28 — HTTP transport module extraction

- Goal: separate the mixed HTTP endpoint module into understandable domain and transport layers
  without changing public behavior.
- Scope: route composition, auth, account/device management, pairing, catalog, search, voice,
  health/admin, shared HTTP transport, admission state, and boundary-local unit tests. No OpenAPI,
  migration, persistence, web, authorization, or business-logic changes.
- Result: `src/http/endpoints.rs` decreased from 3,704 to 392 lines and now only builds the shared
  router and preserves the public router constructors. The extracted modules have one clear
  responsibility each; common request IDs, error bodies, JSON parsing, proxy/origin trust,
  credential hashing, and response DTOs are in `transport.rs`, while rate admission remains in
  `state.rs`. Security, pairing-preview, and device-name regressions moved next to their modules.
- Checks: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test` (100 passed; five PostgreSQL tests ignored without `TEST_DATABASE_URL`), OpenAPI
  contract tests included in the full suite, and `git diff --check` passed. `cargo check
  --all-targets --all-features` also passed.
- Status: **implemented, committed as `2afcae0`, pushed to `origin/master`, and deployed through
  the OPS-001-D staging rollout; readiness passed.**

## HTTP transport architecture guardrail — 2026-08-28

- Goal: prevent further growth of the legacy mixed HTTP endpoint module.
- Result: added always-applied `.cursor/rules/http-module-boundaries.mdc`, requiring domain
  transport modules, thin route composition, and a final refactoring review for every HTTP change.
- Checks: pending final repository checks.
- Status: **guardrail complete; behavior-preserving legacy-module extraction remains a separate task.**

## RM-011-G2 — 2026-08-28 — browser account and pairing UX

- Goal: make a secure pairing link explain one pending device and prevent accidental account
  creation or loss of pairing context.
- Scope: current Preact/Vite page, tab-local browser-session CSRF refresh endpoint, OpenAPI and
  contract/security regressions. No native client, secret file, dependency, account, credential or
  deployment change was made.
- Result: `/` is an authenticated/anonymous account landing page without a generic pairing form.
  A link with the existing pairing code and approval secret renders only that request's product,
  suggested device name, phrase, code and expiry. Anonymous users see distinct passkey sign-in and
  clearly warned account-creation actions; successful ceremonies retain the same URL. A live
  session gets a rotating in-memory CSRF proof and sees a named account/device confirmation with
  connect/cancel. No UUID, credential ID, HTTP detail or native token is rendered.
- Checks: `cargo fmt --check`, strict all-target/all-feature Clippy, `cargo test` (99 regular
  tests), web regression tests, TypeScript typecheck/lint and Vite production build passed. Clean
  browser automation verified the anonymous landing has zero inputs, then a local deterministic
  API harness rendered both the named RockMobile target/anonymous choices and the named signed-in
  confirmation. No passkey, account or staging pairing request was created. PostgreSQL tests remain
  opt-in without `TEST_DATABASE_URL`.
- Deployment: commit `efc405a` completed through the standard detached staging worker with
  `readiness=passed`. A fresh read-only browser session displays the new anonymous landing without
  a UUID or general pairing-code field; no account or passkey was created.
- Status: **deployed to staging.**

## RM-011-G1 — 2026-08-28 — account and pairing contract

- Goal: make the server contract express one user account with many understandable devices, without
  weakening the deployed username-less passkey or native-token boundaries.
- Scope: migration `0016`, account/device/pairing persistence and DTOs, runtime OpenAPI, safe browser
  preview, explicit registration naming, contract/security tests, and current-state documentation.
  No G2 browser UX or native-client workflow was implemented.
- Result: existing account rows receive `Rock account`; legacy device and pairing values are preserved
  through column renames. Browser pairing preview has display/type, short code, phrase, expiry and
  `pending` status, with account display name only for the current browser session. It serializes no
  native proof, credential ID, access token, or refresh token. Approval does not create users;
  strict input DTOs still reject client-supplied owner identifiers.
- Checks: local Rust format, strict Clippy, and 98 tests passed; disposable PostgreSQL tests remain
  ignored without `TEST_DATABASE_URL`. Web typecheck, lint, and production build passed with the
  bundled runtime. The normal staging deploy was attempted and refused its clean-worktree gate;
  no bypass is permitted because the requested changes must remain uncommitted.
- Status: **implemented locally; deployment blocked by the immutable clean-worktree safety gate.**

## RM-011-F P0 blocker fix — 2026-08-27 — username-less discoverable passkey login (deployed)

- Goal: remove the unusable UUID prerequisite from existing-account browser login while keeping
  WebAuthn and pairing security boundaries unchanged.
- Scope: discoverable WebAuthn begin/verify flow, server-side credential owner resolution from the
  assertion `userHandle`, neutral negative handling, frontend login UI/statuses, OpenAPI, and
  focused tests/documentation. No new frontend, migration, account lookup, secret, token, or
  pairing-model change.
- Result: authentication options now create a challenge with an empty `allowCredentials` list and
  no account input. Verify requires a valid UUID-shaped `userHandle`, derives the owner from it,
  checks legacy in-flight challenge bindings when present, and performs owner-scoped credential
  lookup. Client `user_id` spoofing is rejected by the strict DTO. The UI no longer renders or
  submits an account identifier and maps browser no-key/cancel and server failures to safe
  user-facing statuses. Existing registered credentials remain in the same credential table and
  use the same registration user-handle bytes for owner-scoped lookup.
- Checks: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test`, and web `pnpm typecheck`, `pnpm lint`, `pnpm build` passed. Read-only browser
  inspection of both the local and deployed staging page showed no UUID field. The four
  PostgreSQL integration tests remain explicitly ignored without a disposable `TEST_DATABASE_URL`.
  The standard deploy reported commit `7b4f306635e95297ac3cd8b2d99063500d90bc0f`, image
  `sha256:531f1d5051260a3e008f36c96c2dfd3ce84bcc8158d8ccc5c7d09fa7f7f63de8`, and
  `readiness=passed`.
- Status: **F1 implementation deployed; manual physical-device passkey smoke test remains.**

## RM-011-F — 2026-08-27 — staging authorization security acceptance (in progress)

- Goal: perform the final integration/security acceptance of RM-011 browser/native authorization
  on staging without committing or pushing.
- Scope: reviewed RockServer, RockCast, and RockMobile authorization boundaries; ran local
  server/client checks; inspected local Docker state; prepared the standard staging deploy.
- Result: found and minimally fixed P1 QR approval-secret referrer leakage: Caddy now sends
  `Referrer-Policy: no-referrer` in both local and production configurations. Added the
  `deploy_security` regression test and validated both Caddyfiles in Caddy 2.10 with placeholder
  values only. The deploy script correctly refused the dirty worktree, preserving its immutable
  commit-to-image guarantee; no staging deployment was performed after the fix. The existing
  staging site passed HTTPS readiness and rejected direct port `3000`, but its root and negative
  RM-011 route probes returned `404`, so it predates the required auth router.
- Follow-up deployment diagnosis: the immutable image and Caddyfile safely reached the VPS, but
  Compose stopped before database backup because `ROCKSERVER_TRUSTED_PROXY_TOKEN` was absent from
  root-only `release.env`. Bootstrap now provisions that generated proof with the other protected
  runtime values; no secret was inspected or printed. The matching regression test prevents a
  future bootstrap from omitting the required proxy boundary.
- Second deployment diagnosis: Compose also required an immutable Caddy image, while the VPS had
  only the upstream base Caddy container. The rollout now transfers a commit-labelled Caddy web
  bundle alongside RockServer and verifies both archive hashes and revision labels before starting
  either service. This prevents a healthy server image from being paired with a stale/no-UI proxy.
- Caddy image build follow-up: the pnpm workspace retained its placeholder `esbuild` build-policy
  value, so a clean container correctly refused the script. It now explicitly allowlists only
  locked `esbuild`; no package, lockfile, or broader lifecycle-script permission was added.
- Container build follow-up: pnpm requested an interactive module-directory confirmation after the
  source copy. `Dockerfile.caddy` now sets `CI=true` only for that build command, preserving a
  reproducible non-interactive release build.
- Post-deploy probing found the SPA fallback rewrote `/health/*` and `/v1/*` before proxying,
  producing `405` instead of API responses. Both Caddyfiles now use an exclusive API `handle`
  before the static fallback; the regression test covers the required routing boundary.
- Checks: RockServer `cargo fmt --check`, strict Clippy, and `cargo test` passed; RockCast format
  and 85 library tests passed; RockMobile debug unit tests and `assembleDebug` passed; both
  Caddyfiles validated. Physical-device passkey/Keystore evidence remains required.
- Status: **blocked on an approved immutable commit for deployment, then physical-device evidence**.

### Deployment completion follow-up

- Result: after three minimal deployment fixes, staging commit
  `f960e323a2b9e06e0281bf144bb368dc244e54c9` completed successfully. The fixes provision the
  root-only trusted Caddy proof during bootstrap, deploy a matching checksum/label-verified Caddy
  web image, and proxy API/health routes before the SPA fallback.
- Staging checks: `/health/ready` returned JSON `200`; Caddy returned CSP, `no-referrer`,
  `nosniff`, `DENY`, and Permissions-Policy headers; TCP `3000` was unreachable externally;
  browser passkey options without first-party Origin returned `403`; malformed native completion
  returned `422`; a short-lived pairing create returned `201`, and completion with client-supplied
  `user_id` returned `422`. No response proof or identifier was logged.
- Status: **blocked only on manual physical-device passkey/Keystore evidence**.

## MVP-001-C follow-up — 2026-08-26 — local launcher credential fallback

- Goal: allow the documented local launcher to start for public `/v1` testing
  when `.env` contains the required database configuration but no Bearer token.
- Scope: `run-rockserver-local.ps1` and current-state documentation only.
- Result: the launcher now creates a random process-local, non-production
  credential when `ROCKSERVER_API_BEARER_TOKEN` is blank or absent. No value is
  written to `.env` or printed. Legacy `/api/v1` and local admin routes remain
  protected; release/production startup policy is unchanged.
- Checks: PowerShell parser accepted `run-rockserver-local.ps1`; `cargo fmt --check`,
  `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test`
  passed. The five real-provider and two disposable-PostgreSQL tests remained
  explicitly ignored; no server, external provider, or network call was made.
- Status: **complete / passed**.

## OPS-001-A — 2026-08-25 — Production deployment design

- Goal: define the safe single-VPS deployment boundary without creating infrastructure or a runnable
  deployment stack.
- Scope: `deploy/README.md` and current-state documentation only. The design fixes a safe
  non-routable placeholder domain, 80/443 Caddy ingress, Compose-internal RockServer/PostgreSQL
  ports, volumes, environment contract, ownership, health/readiness, firewall matrix, backup/
  restore rehearsal and rollback. Dockerfile, Compose production files, Caddyfile, real secrets,
  registry publishing and deployment are explicitly deferred to OPS-001-B/C/D.
- Result: the design records a private `database` network, Caddy-to-RockServer `edge` network,
  immutable-image release/rollback sequence, and a recovery path that restores a verified encrypted
  logical backup before an incompatible-schema application rollback. It also identifies two verified
  preconditions for OPS-001-B: production must reject the current hard-coded development bearer
  token and must require PostgreSQL rather than falling back to six in-memory stations.
- Checks: documentation/source consistency review against `src/config.rs`, `src/main.rs`, health
  routes and existing development `compose.yaml`; `git diff --check`, `cargo fmt --check`,
  `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test` passed. The regular
  suite passed 81 Rust unit tests plus HTTP/contract suites; two disposable-PostgreSQL, four
  billable Yandex LLM, and one credential/audio SpeechKit integration tests remained explicitly
  ignored. No Docker/VPS/network/secret operation was performed.
- Status: **complete / passed**. The user confirmed the design on 2026-08-25. The placeholder
  domain remains intentionally non-routable; real domain/DNS, deployment ownership, secret
  injection, backup policy, SSH allowlist, Caddy ACME policy and restore authority are manual
  pre-launch inputs and were not changed or committed.

## RM-007-A — 2026-08-25 — Common local personal-data model and ID migration contract

- Goal: define one portable offline-first personal-data contract for future RockMobile and RockCast
  favourites/history without implementing either feature, sync, authentication, server endpoints, or
  catalog changes.
- Scope: RockServer documentation only: versioned `LocalProfile`, `Favourite`, and
  `PlaybackHistoryEntry` model; deterministic dedupe/order/retention; RM-004 ID/lifecycle handling;
  local-first migration/rollback; verified client field mapping; privacy boundary; and human
  approval decisions.
- Result: [`rm-007-a-local-personal-data-contract.md`](rm-007-a-local-personal-data-contract.md)
  distinguishes verified current client behavior from proposed RM-007 design. It preserves IDs for
  URL/primary-stream/rename changes, follows only merged tombstones, and quarantines split, removed,
  missing, and unmapped legacy references. The present RockMobile unavailable-voice migration and
  RockCast URL-derived transitional IDs are not represented as nonexistent favourites/history.
- Checks: documentation/source consistency review against RM-004 contract, current
  RockServer/RockCast/RockMobile status, and `git diff --check` passed.
- Status: ready for human approval; implementation is intentionally deferred to RM-007-B/C.

## RM-004-D — 2026-08-21 — RockServer shared station catalog integration

- Goal: integrate the approved immutable RockCatalog release candidate without adding a network or
  build-time dependency.
- Scope: vendor the v1 schema/catalog/manifest for version `2026.08.2`; add a checksum- and
  invariant-validated provider-neutral adapter; replace the runtime in-memory development catalog;
  atomically activate provider-owned PostgreSQL rows; support stable multi-stream ownership,
  provider-scoped retirement, and preservation/invalidation of operational and derived fields.
- Result: canonical station IDs are both RockServer IDs and `source_station_id` under `rockcatalog`;
  streams use `<station-id>:<stream-id>`. Activation does not cross-provider merge or mutate Radio
  Browser rows, retains absent baseline records through soft retirement, keeps health/probe data for
  an unchanged URL, resets only a changed stream URL, and invalidates station embeddings on metadata
  changes. No public HTTP/OpenAPI fields or DTO shape changed. The extended RockMobile export is not
  part of this stage.
- Checks: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and
  `cargo test` passed. The opt-in PostgreSQL integration test also passed against a disposable local
  PostgreSQL/pgvector container on port 15432. Catalog validation, formatting, and all nine offline
  catalog-tool tests passed.
- Status: complete at the RM-004-D acceptance gate.

## RM-004-B — 2026-08-21 — Catalog repository and validation tooling

- Goal: establish the approved shared-catalog v1 authoring infrastructure without consumer integration or legacy data migration.
- Scope: create the local `C:\repos\rockcast-station-catalog` git repository, final-name schema, minimal canonical catalog, offline standard-library tools, fixtures, release manifest, and ownership/release documentation; record its location here.
- Result: author validation rejects unknown fields and contract/semantic violations; consumer mode tolerates unknown optional v1 fields; formatting and SHA-256 output are deterministic. No RockServer code/API/database changes, network access, actual provider data, 41-station conversion, or extended PostgreSQL snapshot occurred.
- Checks: catalog validation, formatting check, eight `unittest` fixture tests, and SHA-256 manifest generation passed offline. `graphify update .` follows below for the RockServer documentation reference.
- Status: complete.

## RS-037 — 2026-08-20 — Reliable local launcher endpoint output

- Goal: make the local launcher print the actual RockServer bind address and reachable admin URLs.
- Scope: derive the bind port from `ROCKSERVER_BIND_ADDR`, detect active LAN IPv4 addresses with a restricted-shell fallback, and remove stale hardcoded localhost output.
- Result: `run-rockserver-local.ps1` reports the configured listener, localhost preview, and one preview URL per detected LAN address; it no longer claims that `127.0.0.1` is the only endpoint when the server binds all interfaces.
- Checks: PowerShell syntax validation, gateway-backed LAN-address detection (`192.168.31.133`), `git diff --check`, and `graphify update .` passed.
- Status: complete.

## RS-038 — 2026-08-20 — Enforce the fixed local bootstrap token

- Goal: ensure the running RockServer cannot diverge from the token compiled into RockMobile because of stale environment values or an old custom launcher argument.
- Scope: make Rust startup and the local PowerShell launcher use the fixed bootstrap token; improve admin-preview error wording so only HTTP 401 is reported as token rejection; update current-state documentation.
- Result: `rockserver-dev-bootstrap-7f4b9a2c1e6d8a40` is now authoritative for local startup. Legacy `ROCKSERVER_API_BEARER_TOKEN` values are ignored until real user/client credentials replace this temporary mechanism.
- Checks: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, sequential `cargo test`, PowerShell syntax validation, `git diff --check`, and `graphify update .` passed. The regular suite passed 72 library tests plus HTTP/contract tests; external database/provider tests remained ignored.
- Status: complete.

## RS-036 — 2026-08-20 — Stable RockMobile bootstrap credential

- Goal: keep the local RockServer/RockMobile connection working with one credential until user accounts and revocable client tokens exist.
- Scope: add the stable development bootstrap credential to RockServer configuration and the local launcher; use the same default in RockMobile; retain environment/settings overrides; update current-state and API documentation.
- Result: RockServer and `run-rockserver-local.ps1` use `rockserver-dev-bootstrap-7f4b9a2c1e6d8a40` when no override is configured, and RockMobile sends the same value by default. Configured `ROCKSERVER_API_BEARER_TOKEN` values remain validated and take precedence.
- Checks: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and sequential `cargo test` passed (72 library tests plus HTTP/contract tests; external database/provider tests remained ignored). PowerShell syntax validation, `git diff --check`, and `graphify update .` passed. RockMobile `:app:lintDebug` passed; its unit suite still has six unrelated existing failures involving unmocked `android.util.Log` and coroutine timing.
- Status: complete.

## RS-035 - 2026-08-20 - Bind HTTP listener on all interfaces

- Goal: make the default RockServer listener reachable from other hosts on the configured network.
- Scope: change the default socket address from `127.0.0.1:3000` to `0.0.0.0:3000`; keep `ROCKSERVER_BIND_ADDR` as the explicit override; update user-facing documentation.
- Result: when `ROCKSERVER_BIND_ADDR` is unset, `Config::from_env` now configures `0.0.0.0:3000`. Localhost-only operation remains available by setting `ROCKSERVER_BIND_ADDR=127.0.0.1:3000`.
- Checks: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`, and `graphify update .` passed. The regular suite ran 70 library tests plus the HTTP/contract suites; database and credential-dependent live tests remained explicitly ignored.
- Status: complete.

## RS-034 — 2026-08-20 — Buffered microphone-capture regression diagnosis

- Goal: identify and correct the loss of recognition quality reported in both selectable voice modes.
- Scope: a billable opt-in Yandex TTS-to-PCM-to-STT diagnostic, focused provider verification, and a RockCast buffered-capture correction. No production API or protocol change.
- Result: a 1,841 ms Yandex-generated PCM16 command was recognized exactly by SpeechKit v1 in 925 ms, isolating the provider from the regression. RockCast buffered capture no longer routes audio through the bounded `try_send` queue introduced for live streaming, so it cannot silently discard callback chunks when that consumer falls behind. The streaming path remains separately queued for its next backpressure-focused iteration.
- Checks: `cargo check --bin diagnose_speechkit_pcm`, 70 RockServer library tests, and 41 RockCast library tests (including the new buffered-frame preservation test) passed. `graphify update .` refreshed the local RockServer code graph; it reported the existing `hooks.json` zero-node warning and stale community labels.
- Status: complete.

## RS-033 — 2026-08-20 — Selectable SpeechKit streaming recognition

- Goal: add upstream streaming recognition without removing the existing bounded, buffered voice path.
- Scope: SpeechKit v3 gRPC adapter, per-session `recognizer_mode` selection in the existing WebSocket start event, backwards-compatible `buffered_v1` default, OpenAPI/docs, deterministic request/response tests, and RockCast settings integration. No live SpeechKit call is made by ordinary tests.
- Result: `streaming_v3` sends PCM chunks to SpeechKit while RockCast records and can forward partial/final transcript events. `buffered_v1` remains the default and continues to submit the bounded recording after `commit`.
- Checks: changed-file formatting, `cargo check`, 70 library tests, the streaming WebSocket integration test, and strict all-target/all-feature Clippy passed. `graphify update .` refreshed the ignored local graph.
- Status: complete.

## RS-031 — 2026-08-20 — Explicit world-country parsing

- Goal: make explicit country requests such as `немецкий рок` consistently set the ISO country hard filter without treating locale or an LLM guess as country intent.
- Scope: replace the three-country mapping with ISO 3166-1 alpha-2 aliases in Russian and English, including common demonyms and multi-word country names; add regression tests. Public HTTP/OpenAPI behavior and ranking are unchanged.
- Result: `немецкий` and the natural request form `рок из Германии` now produce `country_code=DE`; explicit names and demonyms across the supported country set produce their alpha-2 code. Ambiguous matches intentionally produce no country filter.
- Checks: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test` passed (61 unit tests, integration/contract tests, with opt-in live/database tests ignored as intended).
- Status: complete.

## RS-030 — 2026-08-20 — Indexed candidate-branch PostgreSQL search

- Goal: remove timeout-prone broad PostgreSQL station searches without changing query-to-station matching or ranking behavior.
- Scope: replace the correlated `OR` prefilter with separate tag, full-text, and trigram candidate-ID branches; union/de-duplicate IDs before the existing bounded prefilter and unchanged final scoring; construct FTS input once, precompute FTS queries per expanded term, and normalize/tokenize each candidate station name once; add stage-duration debug logs and a structural regression test for the query shape. Public HTTP/OpenAPI behavior is unchanged.
- Result: broad candidate selection can use the existing GIN indexes independently before expensive scoring, while expanded voice terms no longer repeat FTS parsing, name normalization, or regex tokenization. The original hard filters, exclusions, candidate limit, score calculations, and `score DESC, id ASC` ordering remain in place.
- Checks: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test` passed. The opt-in PostgreSQL integration test remains ignored because no disposable `TEST_DATABASE_URL` was available. Live voice-command checks against the configured PostgreSQL database returned 30 stations in 1,387 ms for `Включи немецкий рок или хэви метал` and in 1,388 ms for the observed STT transcription `Включи немецкий рок или хейли метал`; PostgreSQL took 233 ms for the latter.
- Status: complete.

## RS-019 — 2026-08-19 — Genre hierarchy and progressive search fallback

- Goal: make subgenre queries (e.g. "heavy metal", "black metal", "smooth jazz") find stations tagged with parent genres when no exact match exists, and prefer stations matching the requested language.
- Scope: expanded canonical tag vocabulary with subgenres from the real Radio Browser catalog; a static genre-parent hierarchy (`black metal` → `metal` → `rock`); ancestor-aware `station_matches_requested_genre`; progressive fallback in `SearchService::search` (exact → parent → drop genre); a new builtin catalog station for metal; deterministic tests including the `"english heavy"` use case; and documentation updates. No public HTTP API, OpenAPI, PostgreSQL SQL, migration, or embedding changes.
- Result: `CANONICAL_TAGS` grew from 27 to 40 entries with metal subgenres (`black metal`, `death metal`, `doom metal`, `power metal`, `symphonic metal`, `thrash metal`), rock subgenres (`alternative rock`, `classic rock`, `indie rock`, `pop rock`, `progressive rock`, `psychedelic rock`, `soft rock`), and jazz subgenres (`acid jazz`, `latin jazz`, `smooth jazz`). `genre_parent` and `genre_ancestors` expose the hierarchy. `station_matches_requested_genre` accepts stations whose tags match any ancestor. `SearchService::search` progressively relaxes the genre constraint: exact match → parent genres → no genre filter, always gated by `MIN_RELEVANCE_SCORE`.
- Checks: `cargo fmt --check` passed; `cargo clippy --all-targets --all-features -- -D warnings` passed; `cargo test` passed with 67 regular tests (48 unit + 19 integration), 6 ignored live/database tests.
- Status: complete.

## RS-016 — 2026-08-17 — Local ONNX multilingual semantic search

- Goal: add an opt-in CPU/local ONNX embedding provider suitable for Russian and English station discovery.
- Result: selected `intfloat/multilingual-e5-small` (384 dimensions), added local ONNX Runtime/tokenizer inference, E5 query/document prefixes, normalized station text, importer maintenance, backfill support, and a provenance-specific pgvector HNSW cosine index.
- Verification: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test --all-features` pass with 50 regular tests. The local PostgreSQL database was migrated, 999 real Radio Browser stations were imported, all 1,005 catalog rows received E5 embeddings, and three Russian live queries ran through the HTTP search path. Jazz and 80s-rock ranked relevant stations first; the classical/instrumental query exposed a catalog/locale-filter quality limitation.
- Status: complete; local model assets remain intentionally outside Git.

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

## RS-006 — 2026-08-13 — Controlled Radio Browser import

- Goal: add a controlled, repeatable Radio Browser import into PostgreSQL outside the HTTP request path and server startup, without pgvector, embeddings, LLMs, authentication, rate limiting, RockCast work, or stream probing.
- Scope: provider-neutral import models and orchestration; an import-provider trait independent of `StationRepository`; a bounded Radio Browser HTTP client; deterministic DTO validation/normalization; an import-only PostgreSQL persistence trait; provider ownership and stream identity; durable `import_runs`; a one-shot CLI; unit, local mock HTTP, and opt-in PostgreSQL integration coverage; current-state documentation and diagrams. The public OpenAPI/search contract, liveness, and readiness semantics remain unchanged.
- Result: added migration `0003_add_catalog_import.sql`, which preserves the six seed stations as `builtin`, adds source identities for station/stream idempotency, removes global stream-URL uniqueness in favor of provider stream identity, and adds terminal import-run status/count/timestamp/error storage. Added `CatalogImporter`, `CatalogImportProvider`, and `CatalogImportStore` boundaries. Added `RadioBrowserClient` with explicit User-Agent, official DNS-balanced default root, configurable safe root, 1–60 second timeout, 1–500 page size, 1–100 maximum pages, 8 MiB response cap, deterministic per-page UUID order, and sanitized failures. Added `PostgresImportStore` transactional upserts and `import_radio_browser`; `DATABASE_URL` is mandatory for that binary and no DSN, stream URL, or response body is logged.
- Mapping rules: consumes official `stationuuid`, `name`, `url_resolved`, `homepage`, `tags`, `languagecodes`, `countrycode`, `codec`, `bitrate`, and `lastcheckok`. Rows are skipped unless the upstream check is good, UUID/name are valid, and resolved stream URL is credential-free HTTP(S). Optional homepage is validated; tags are lowercase/sorted/deduplicated and bounded; language prefers the first valid two-letter code across the full list and falls back to the first valid three-letter code only when necessary; valid alpha-2 country, bounded codec, and 1–2000 kbps bitrate are retained. Repeat runs update `(radio_browser, stationuuid)` and its provider-owned primary stream without duplicates. Missing upstream rows are retained.
- Sources: behavior and fields were verified against the official Radio Browser [API usage](https://docs.radio-browser.info/#using-the-api), [Station](https://docs.radio-browser.info/#station), [advanced search](https://docs.radio-browser.info/#advanced-station-search), and [mirror discovery](https://docs.radio-browser.info/#server-mirrors) documentation and the official [API repository](https://gitlab.com/radiobrowser/radiobrowser-api-rust).
- Checks: `cargo fmt --check` passed; `cargo clippy --all-targets --all-features -- -D warnings` passed; `cargo test --all-targets --all-features` passed with 25 regular tests and one opt-in PostgreSQL test ignored by default; `git diff --check` passed. Unit tests cover mapping, language preference/fallback, normalization, skip rules, bounds, pagination, partial failure accounting, source mismatch rejection before upsert, terminal failure counts, and deterministic/idempotent input. Local loopback mock HTTP tests cover request headers/query, parsing, and sanitized HTTP errors with no real external call. `docker compose config --quiet` passed. Python's standard-library `html.parser` accepted `docs/service-diagrams.html`; checks also confirmed one doctype/root, balanced SVG elements, and no external resource URL.
- Real integration: Docker Engine 29.7.2 and Compose 5.3.1 ran a dedicated `rockserver-rs006-test` PostgreSQL 17 project on host port 55436. The ignored integration test passed for migrations, first import, repeat update without station/stream duplicates, six-seed preservation, completed/failed run counts/status/timestamps/error summary, search over imported metadata, and readiness. Only that project's container, network, and volume were removed afterward.
- Smoke: a read-only official API request with `limit=1`, a descriptive `RockServer-Smoke/0.1.0` User-Agent, and a 15-second timeout succeeded and returned one checked station with a non-empty UUID. It wrote no database and is not in the ordinary suite.
- Known limitations: upstream name-ordered offset pagination is not a transactional snapshot; configured bounds intentionally produce a safe slice, not a full mirror. No-delete ownership prevents partial runs from removing data. A hard process/host failure can leave `started` stale because reconciliation is not yet implemented. `lastcheckok` is trusted without local probing, and three-letter-only language metadata still cannot match a strict locale-derived two-letter filter without a future ISO 639 mapping.
- Status: complete.

## RS-006 review hardening — 2026-08-13 — Language and source invariants

- Goal: resolve merge-blocking compatibility between imported language codes and locale-derived search filters, and prevent provider records from crossing import-run source ownership.
- Scope: deterministic language selection, orchestration ownership validation, run-bound PostgreSQL source enforcement, focused tests, and documentation. No migration or public HTTP/OpenAPI change.
- Result: language normalization now scans the complete list and prefers the first valid two-letter code regardless of position; only a three-letter-only list falls back to its first valid three-letter code. `CatalogImporter` rejects a mismatched-source page before upsert, marks the normalized page failed, and records a generic terminal error. `PostgresImportStore` verifies the explicit source against the still-started run and uses that value for writes rather than record fields.
- Checks: focused tests cover `yue,zh -> zh`, `EN,eng -> en`, deterministic three-letter-only fallback, no-upsert ownership rejection, sanitized error text, and terminal failed counts/status. Final formatting, strict Clippy, all-target/all-feature tests, and diff checks passed.
- Real integration: because runtime SQL changed, the opt-in PostgreSQL test passed again in isolated Compose project `rockserver-rs006-review-test` on port 55437; only that project's container, network, and volume were removed afterward.
- Status: complete.

## RS-007 — 2026-08-14 — Semantic ranking foundation

- Goal: add a minimal working semantic-search foundation behind domain and repository boundaries without adding a production LLM/model provider, changing the public HTTP schemas, or moving catalog/provider network work into `POST /v1/search`.
- Scope: provider-neutral `QueryParser` and `EmbeddingProvider`; deterministic parser/failure fallback and deterministic fakes; explicit non-production development embedder; pgvector migration and provenance-aware station embedding store; controlled one-shot backfill/update CLI; PostgreSQL exact hybrid search; in-memory metadata fallback; unit, HTTP, and opt-in real PostgreSQL coverage; Compose, OpenAPI description, architecture/status/roadmap/diagram updates. Authentication, rate limiting, RockCast, voice input, stream probing, scheduler, production providers, and ANN tuning are excluded.
- Result: `SearchService` now interprets validated request-only input and obtains an optional query embedding before crossing `StationRepository`. Providers cannot receive a station collection through their interfaces. Parser errors or invalid structured filters use the deterministic parser; embedding errors omit semantic input. The in-memory repository continues to use the original deterministic metadata ranker. PostgreSQL applies language/country filters and exclusions before score/limit, joins only matching model/version/dimension provenance, performs exact cosine similarity, and orders score descending with station ID ascending as the last tie-break.
- Ranking decision: domain constants own weights. Compatible embeddings use normalized cosine `1 - cosine_distance / 2` and `0.70 * metadata_score + 0.30 * semantic_score`. Missing compatible station embeddings retain full metadata score; missing/failed query embeddings use metadata-only inclusion and scoring. SQL receives the domain weights as bind parameters rather than duplicating literals.
- Persistence decision: migration `0004_add_station_embeddings.sql` enables pgvector and creates a separate table keyed by station/model/version with dimension, unbounded `vector`, and timestamps. Dimension and non-zero norm are CHECK-enforced. The schema is not locked to the test/dev dimension; no HNSW/IVFFlat index is added before a production model and scale measurements exist. SQLx queries remain runtime-checked and require no live database at ordinary build time.
- Validation decision: an `Embedding` can be constructed only through its validating constructor; its provenance and vector values are private and exposed read-only so a provider cannot bypass model, dimension, finite-value, or non-zero-norm checks before the repository boundary.
- Workflow decision: `backfill_embeddings` requires PostgreSQL and explicit `ROCKSERVER_SEMANTIC_PROVIDER=deterministic-dev`, pages by stable station ID, embeds one station document at a time, and idempotently upserts provenance-specific rows. It runs outside HTTP startup/search and does not alter Radio Browser ownership/import behavior. The deterministic hash embedder is documented as a development fixture, not a production semantic model.
- Checks: `cargo fmt --check` passed; `cargo clippy --all-targets --all-features -- -D warnings` passed; `cargo test --all-targets --all-features` passed with 35 regular tests and one opt-in PostgreSQL test ignored by default; `git diff --check` passed. `docker compose config --quiet` passed with the pgvector PostgreSQL 17 image. Python's standard-library HTML parser accepted `docs/service-diagrams.html`; checks confirmed one doctype/root, balanced SVG elements, and no external resource URL. Unit coverage includes parser boundaries/normalization, invalid intent fallback, embedding model/dimension/finite/non-zero validation, deterministic fake embeddings, provider-failure metadata fallback, controlled backfill, fixed hybrid weights, exclusions, limit, and stable tie-break.
- Real integration: Docker Engine 29.7.2 and Compose 5.3.1 ran isolated pgvector projects on ports 55438 and, after final runtime-SQL weight binding, 55439. The final ignored integration test passed for extension/migrations, embedding insert/repeat update, provenance/dimension storage, exact cosine similarity, hard filters, exclusions, final limit, semantic station-ID tie-break, metadata fallback, HTTP search, and repository-only readiness. The real deterministic backfill also completed twice and produced seven unique development-provenance rows. Only the created projects' containers, networks, and volumes were removed.
- Graphify: after deleting only the verified task-created `C:\repos\rockserver\target-rs007`, `graphify update .` completed. The permitted `tree-sitter-sql` extra was installed in Graphify's existing isolated `uv tool` environment; the final code update rebuilt the ignored local graph to 517 nodes, 1066 edges, and 30 communities without SQL-parser warnings. Remaining tool warnings are a skill/package version mismatch (`0.9.35` instructions versus `0.9.40` package), `hooks.json` producing no nodes, and stale community labels; optional LLM relabeling or semantic extraction of changed documentation was not run. Generated `graphify-out/` files remain untracked.
- Known limitations: exact vector scan has no ANN index; deterministic-dev has no production semantic quality; backfill recomputes all station documents and has no durable resume/unchanged-input skip; no model registry enforces one dimension globally per model/version; optional providers have no production authentication/retry/circuit breaker. Existing RS-006 upstream/import limitations remain.
- Status: complete.

## Roadmap decision — 2026-08-14 — Windows-first voice production path

- Goal: preserve the agreed delivery order in the repository so implementation follows the Windows RockCast production workflow rather than beginning with ESP32.
- Scope: planning documentation only. No server API, database schema, provider, RockCast code, or runtime behavior changed.
- Result: added `docs/windows-production-roadmap.md`; the plan verifies RS-007 on disposable local PostgreSQL first, integrates RockCast text search with local fallback, adds Windows microphone capture, adds a RockServer voice/STT path that reuses `SearchService`, introduces production providers, completes Windows end-to-end and operational hardening, and leaves ESP32 in future backlog. Updated the near-term TODO and current status to point to the same ordered plan. Database credentials remain environment-only and are not committed.
- Checks: documentation paths and Git diff were inspected; `git diff --check` passed. Rust checks were not required because no Rust, OpenAPI, dependency, migration, or runtime behavior changed.
- Status: planned.

## RS-007 verification — 2026-08-14 — Local PostgreSQL and HTTP smoke

- Goal: re-verify the completed semantic-ranking foundation on the user-designated disposable local `rockserver` database before beginning RockCast integration.
- Scope: database reset, clean migrations, the existing ignored PostgreSQL integration test, deterministic embedding backfill idempotency, database inspection, and a local HTTP smoke check. No source code, OpenAPI, migration, or runtime behavior changed.
- Result: connected to PostgreSQL 18.1 with pgvector 0.8.6, reset only the `public` schema of database `rockserver`, and successfully applied migrations 1–4. The exact ignored integration test passed. Two consecutive `deterministic-dev` backfills produced seven unique 32-dimensional embeddings for seven stations and zero duplicate station/model/version keys. The test's four 3-dimensional `integration-model` fixtures also remained valid. A temporary server on loopback reported `live=ok` and `ready=ok`; `POST /v1/search` for `jazz` returned hybrid-ranked results headed by `station-jazz-002` with reason `Hybrid match: metadata 1.000, semantic 0.878.` The temporary server was stopped after the smoke check.
- Security: the database password and connection URL were supplied only through process environment variables and were not written to repository files.
- Checks: `cargo test --test postgres_integration --all-features -- --ignored --exact postgres_migrations_seed_search_and_readiness --nocapture` passed with one test and no failures; migration, row-count, provenance, dimension, and duplicate-key queries passed; `git diff --check` passed.
- Status: complete.

## RS-008 — 2026-08-14 — Windows voice-command API contract

- Goal: stabilize the JSON contract used by the Windows voice-command flow after RS-007 without adding a second search implementation, audio upload, STT, or provider credentials.
- Scope: canonical OpenAPI/Axum `POST /api/v1/voice/command`, deprecated `/v1/voice/command` compatibility alias, transcript DTOs, selected-station response semantics, request IDs, validation, request-size and service-time boundaries, contract tests, and current-state documentation. Existing `POST /v1/search` request/response fields remain compatible.
- Result: the canonical route accepts only a bounded, already-recognized `transcript` plus locale, limit, and station exclusions, then calls the existing `SearchService`. It returns the trimmed transcript, normalized query, deterministic `stations`, and `selected_station` equal to the first result or `null`. Valid `X-Request-Id` values are echoed in the response body and header; otherwise the server generates one. JSON bodies are limited to 65,536 bytes; malformed input returns 400, oversized input 413, validation 422, unexpected search failure 500, and the five-second interpretation/search deadline returns structured 504. No audio, STT, LLM, or provider credential is accepted or simulated.
- Compatibility decision: `/api/v1/voice/command` is canonical for new Windows clients. `/v1/voice/command` maps to the same handler and DTOs but is documented as deprecated so the repository rule for `/v1` public versioning and the Windows `/api/v1` contract can coexist. Existing `/v1/search` remains registered with its existing body schemas; it now also uses the shared bounded JSON/error/request-ID transport behavior.
- Tests: `cargo fmt --check` passed; `cargo clippy --all-targets --all-features -- -D warnings` passed; `cargo test` passed with 40 regular tests and one opt-in PostgreSQL test ignored by default. Contract coverage verifies the canonical route, compatibility alias, request-ID propagation, selected/no-result behavior, transcript validation, structured 413 size failure, and deterministic 504 timeout.
- Graphify: `graphify update .` rebuilt the ignored local code graph to 559 nodes, 1,165 edges, and 32 communities. The tool reported only its pre-existing `hooks.json` zero-node warning and stale community labels; generated graph artifacts remain untracked.
- Documentation: `api/openapi.yaml` is the source of truth; `docs/status.md`, `docs/architecture.md`, `docs/windows-production-roadmap.md`, `README.md`, and `docs/service-diagrams.html` now distinguish the implemented JSON boundary from still-future microphone, audio-upload, and STT work.
- Status: complete.

## RS-009 — 2026-08-14 — Provider-neutral streaming voice API

- Goal: add a stable bidirectional audio contract without coupling RockCast to Yandex or OpenAI protocols and without changing the RS-008 JSON endpoint.
- Scope: canonical WebSocket `GET /api/v1/voice/stream`, deprecated `/v1/voice/stream` alias, PCM16 mono start/audio/commit protocol, incremental and final transcript events, final `SearchService` resolution, request IDs, size/time/error boundaries, provider traits, OpenAPI, deterministic tests, and current-state documentation. Real provider credentials and external-network tests are excluded.
- Result: added `StreamingSpeechRecognizer` and isolated per-client `SpeechStreamSession` boundaries. The WebSocket accepts 8/16/24/48 kHz PCM signed 16-bit little-endian mono chunks, limits each chunk to 65,536 bytes and the session to 10 MiB, emits `ready`, `transcript`, and terminal `result`/`error` events, and applies ten-second provider-operation and five-second search boundaries. The final transcript reuses the existing query interpretation, ranking, exclusions, and selected-station behavior. Default startup fails closed with `speech_provider_unavailable` until a concrete provider is configured.
- Tests: a real loopback WebSocket upgrade test injects a deterministic fake provider, verifies request-ID propagation, binary audio delivery, partial/final transcripts, and final station selection. OpenAPI structural coverage verifies both routes and all client/server event schemas. `cargo fmt --check`, strict all-target/all-feature Clippy, and `cargo test` passed with 41 regular tests and one opt-in PostgreSQL test ignored. Diff whitespace and the standalone HTML parser check passed. No external provider or internet call occurs.
- Graphify: `graphify update .` rebuilt the local ignored graph to 620 nodes, 1,339 edges, and 33 communities. The existing `hooks.json` zero-node warning remains, and labels are stale after the new community was introduced.
- Known limitations: this task stabilizes the service protocol and provider seam; it does not yet decode production audio because neither Yandex SpeechKit nor OpenAI Realtime is configured in `main`.
- Status: complete.

## RS-010 — 2026-08-14 — Yandex AI Studio structured intent parser

- Goal: add a production Yandex AI Studio adapter that turns text and already-recognized voice commands into the existing `QueryIntent`, without creating a second search path or changing public HTTP schemas.
- Scope: provider-neutral `LlmProvider` and `LlmQueryParser`; synchronous Yandex completion adapter; environment-only configuration and optional ignored local `.env`; strict JSON Schema plus local response validation; bounded HTTP timeout/body/token handling; deterministic failure fallback; loopback-only provider tests; and current-state documentation.
- Result: startup selects `YandexLlmProvider` only when both `YANDEX_AI_API_KEY` and `YANDEX_FOLDER_ID` are present. With neither value, it preserves `DeterministicQueryParser`; partial settings are a safe configuration error naming only variables. The Yandex request uses documented `Api-Key` authorization and `gpt://<folder>/<model>/latest`, fixed system rules, a JSON-encoded `{command, locale}` user message, and `jsonSchema` for the existing terms/tags/language/country intent. No station data crosses the provider boundary. Non-2xx, timeout, malformed/oversized response, malformed intent JSON, and invalid hard filters all keep the existing deterministic fallback. This parser is unrelated to SpeechKit/STT and is used by both text search and already-recognized voice commands through `SearchService`.
- Configuration: `.env.example` contains empty secret placeholders only. `dotenvy` may load an ignored local `.env`; production still reads process environment. Optional `YANDEX_LLM_MODEL` defaults to `yandexgpt`; `YANDEX_LLM_TIMEOUT_MS` defaults to 3000 and permits 100–10,000.
- Tests and checks: `cargo fmt --check`, strict all-target/all-feature Clippy, `cargo test` (47 regular tests passed; one opt-in PostgreSQL test ignored), and `git diff --check` passed. Provider tests use loopback Axum mocks and deterministic fake credentials; no test calls Yandex or reads a real `.env`. `graphify update .` was invoked but the installed Graphify environment failed before rebuilding with `No module named 'networkx.readwrite'`; generated graph artifacts remain absent and no global dependency was installed.
- Status: complete.

## RS-011 — 2026-08-14 — Opt-in real SpeechKit voice integration test

- Goal: add a manual live check for an actual recorded voice command sent to Yandex SpeechKit, with operational logs that do not disclose credentials or voice content.
- Scope: ignored integration test only; synchronous SpeechKit v1 recognition for a local mono Ogg/Opus recording; bounded request/response handling; deterministic environment validation; safe tracing; and current-state documentation. No public API, streaming protocol, or production STT adapter changes.
- Result: `tests/yandex_speechkit_live.rs` loads configured values only at explicit test execution, posts the recording to SpeechKit, verifies a 200 JSON result and normalized expected-command containment, and logs only test ID/status/timing/character count/match boolean. `generate_speechkit_fixture` explicitly synthesizes the committed Ogg/Opus fixture and its expected phrase with Yandex TTS; it requires only `YANDEX_AI_API_KEY` and `YANDEX_FOLDER_ID`. The live STT test uses those committed files by default, accepts optional audio/phrase overrides, and remains ignored because it is billable.
- Tests and checks: `cargo fmt --check`, strict all-target/all-feature Clippy, `cargo test` (the live test is correctly ignored), and `git diff --check` passed. `graphify update .` was invoked but the installed Graphify environment failed before rebuilding with `No module named 'networkx.readwrite'`; no global dependency was installed. A live invocation is not run unless the user explicitly requests it with configured credentials and a real local recording.
- Status: complete; live execution remains pending the local audio input.

## RS-012 вЂ” 2026-08-14 вЂ” Generate committed SpeechKit fixture

- Goal: create the repository's real voice-command fixture through Yandex TTS using the configured environment without exposing credentials.
- Result: added opt-in `generate_speechkit_fixture`, which sends only the fixed command text to the documented TTS v1 endpoint, bounds and validates the Ogg response, and writes the Ogg fixture and expected transcript only after success. It uses service-account API-key authentication and does not send or require `folderId`, as SpeechKit derives the folder from the key's service account. The live STT test now uses these committed files by default and retains optional overrides. Two explicit TTS runs, before and after that correction, returned HTTP 401; no audio fixture was generated and no secret or provider body was logged.
- Status: complete. After local authorization was corrected, the generated Ogg fixture was sent to the live STT test on 2026-08-14 and returned HTTP 200 with a normalized expected-command match. No credential or transcript was logged.

## RS-013 вЂ” 2026-08-14 вЂ” Safe SpeechKit fixture diagnostics

- Goal: make the failed live TTS request inspectable without exposing the configured API key.
- Result: `YANDEX_SPEECHKIT_DEBUG=1 cargo run --bin generate_speechkit_fixture` prints the fixed non-secret request fields, redacted authorization, response status/headers, and a 16 KiB bounded redacted non-success response body. It also removes provider-echoed key-like fragments. It intentionally does not print the credential or raw unbounded provider content.
- Status: complete.

## RS-014 вЂ” 2026-08-14 вЂ” Opt-in STT mismatch diagnostics

- Goal: make a live SpeechKit recognition mismatch directly inspectable by the developer.
- Result: `TEST_YANDEX_STT_DEBUG=1` logs the endpoint, non-secret query fields, audio path and size, expected transcript, raw successful JSON response, and recognized transcript. The `Authorization` header is represented only as `Api-Key [REDACTED]`.
- Status: complete.

## RS-015 — 2026-08-14 — Structured voice-command interpretation

- Goal: turn final SpeechKit text into a strict typed player command through Yandex structured output, while keeping the LLM provider replaceable and ordinary tests offline.
- Scope: serde-backed `VoiceCommand`, `Intent`, and `RadioQuery`; provider-neutral `CommandInterpreter`; strict Yandex-compatible JSON Schema; semantic validation and normalization; secret-safe bounded HTTP diagnostics; unit coverage; and a separate ignored `yandex_llm_live` target with calm-jazz, Russian-rock, next-station, and volume exact tests.
- Result: `LlmCommandInterpreter` sends only transcript and locale through the existing `LlmProvider`, deserializes the provider JSON directly, and rejects invalid intent/field combinations without an heuristic parser. Yandex diagnostics log method, endpoint, redacted authorization/folder, request body, status, and bounded response body; configured API keys and folder IDs are removed. Yandex requires every schema field to be listed as required, with nullable values used for intent-specific omissions; both voice-command and existing search-intent schemas now follow that constraint.
- Checks: `cargo fmt --check`, strict all-target/all-feature Clippy, and `cargo test` passed with 50 regular tests, four Yandex LLM live tests ignored, the existing SpeechKit live test ignored, and the PostgreSQL integration ignored. `cargo test --test yandex_llm_live -- --ignored --exact interprets_real_voice_command_with_safe_logs --nocapture` passed live with HTTP 200 and a typed calm-jazz command; logs exposed no credential or folder identifier. `graphify update .` rebuilt the local graph to 774 nodes, 1,692 edges, and 37 communities, with only the known zero-node `hooks.json` warning and stale community labels.
- Status: complete.

## RS-017 — 2026-08-18 — Application API Bearer gate

- Goal: establish the first fail-closed API access boundary before adding persisted RockCast
  clients and Bearer-based administrator sessions.
- Scope: production token configuration, HTTP and WebSocket authorization gate, OpenAPI security
  declaration, deterministic tests, and current-state documentation. No cookies, CSRF flow,
  database credential tables, or admin UI are included in this slice.
- Result: production startup requires `ROCKSERVER_API_BEARER_TOKEN` with at least 32 characters.
  All search and voice application routes require `Authorization: Bearer <token>`, including
  validation before a WebSocket upgrade. Missing, malformed, and invalid credentials receive the
  standard structured `401 authentication_required` response with `WWW-Authenticate: Bearer`.
  Liveness and readiness remain unauthenticated for supervision. The OpenAPI document declares
  the opaque Bearer scheme and each protected operation's 401 response.
- Documentation follow-up: updated `docs/service-diagrams.html` to show public health versus
  Bearer-protected application routes, current RockCast voice authorization, `401` semantics,
  and the separate NEXT state for persisted client/admin identity and the HTMX console.
- Checks: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test`, and `git diff --check` passed. The ordinary suite ran 56 tests successfully;
  PostgreSQL and provider-credential-dependent live tests remained explicitly ignored.
- Status: complete.

## RS-020 — 2026-08-19 — Genre DB, stream probe, multi-tag import

- Goal: store the genre hierarchy in the database, add live stream probing, expand the import to pull stations by tag, and filter degraded streams from search.
- Scope: migration 0006 (`genre_hierarchy` table with ~250 canonical genres/subgenres); migration 0007 (`last_probe_at`/`last_probe_error` on `station_streams`); `GenreTaxonomy` struct loadable from PostgreSQL with builtin fallback; `probe_streams` binary (8s timeout, 50 concurrency); `RadioBrowserClient::with_tag()` and `RADIO_BROWSER_TAGS` env; `health <> 'degraded'` filter in search SQL; persistence helpers for taxonomy loading and stream health updates.
- Result: all 52 unit tests pass. fmt/clippy/test clean. Backward-compatible static API preserved.
- Checks: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test` — all clean.
- Status: complete.
## RS-021 — 2026-08-19 — Station name search accuracy

- Goal: fix voice queries like "включи радио диджей" not finding station `radioDJ`; add embedding backfill to the database population script.
- Scope: stop-word filtering (command verbs removed from query terms); CamelCase splitting in `tokenize()` (`radioDJ` → `radio` + `dj`); word-level Cyrillic↔Latin transliteration (~50 mappings: радио↔radio, диджей↔dj, фм↔fm, etc.); substring matching with 0.5 weight in both in-memory ranking and PostgreSQL search CTE; `searchable_text()` enriched with CamelCase-split tokens at import time; `fill-database.ps1` updated with `backfill_embeddings` step.
- Result: all 59 unit tests pass; new tests cover stop-word removal, transliteration expansion, camelCase split, and tokenize integration. fmt/clippy/test clean.
- Checks: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test` — all clean.
- Status: complete.

## RS-022 — 2026-08-19 — Search name performance + intent normalization

- Goal: fix slow/timeouts for station name searches and improve relevance when the LLM returns empty or multi-word `terms`.
- Scope: normalize provider `terms` by splitting into atomic tokens (`tokenize()`), apply transliteration alias expansion for query terms, and add deterministic fallback when both `terms` and `tags` are empty; add PostgreSQL `pg_trgm` index on `lower(stations.name)` to speed up `LIKE '%term%'` prefiltering and substring scoring; keep existing metadata/semantic hybrid ranking and degraded-stream filtering.
- Result: unit/integration tests pass; SQL is still hybrid-ranked but prefiltering becomes index-friendly once `pg_trgm` is applied.
- Checks: `cargo fmt --check`, strict `cargo clippy`, and `cargo test` passed.
- Status: complete.

## RS-023 — 2026-08-19 — Full-text search and similarity scoring

- Goal: replace slow sequential `LIKE '%term%'` prefilter with index-backed FTS and pg_trgm similarity.
- Scope: migration 0009 (`searchable_tsv` tsvector column, GIN index, auto-update trigger using `simple` config); prefilter rewritten to use `plainto_tsquery` (FTS) and `%` operator (pg_trgm similarity) instead of `LIKE`; `trgm_score` added to scoring with 0.3 weight; `raw_query` field threaded through `QueryIntent` → `SearchQuery` → `PostgresSearchParameters` → SQL `$14`.
- Result: all 56 unit tests pass. fmt/clippy/test clean. Migration must be applied on live DB for effect.
- Checks: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test` — all clean.
- Status: complete.

## RS-024 — 2026-08-19 — Correct name ranking when LLM omits terms

- Goal: fix incorrect ranking and extra latency when the LLM returns only `tags` and `terms` is empty (e.g. "включи радио рокс"), which breaks station-name token matching.
- Scope: in `SearchService::interpret_and_search`, if `intent.terms` is empty then we run `DeterministicQueryParser` and:
  - keep provider hard genre tags (when present),
  - but replace `terms`, `core_term_count`, and `raw_query` with deterministic tokenization outputs for name matching.
  - extend transliteration vocabulary with common voice tokens: `ультра`↔`ultra`, `рокс`↔`roks`, `викер`↔`viker`.
- Result: unit tests pass, including new transliteration coverage.
- Checks: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- Status: complete.

## RS-025 — 2026-08-19 — Narrow prefilter candidate set before heavy scoring

- Goal: reduce slow search queries and timeouts on broad requests where `prefiltered` matched too many stations before exact scoring.
- Scope: make `prefiltered` `MATERIALIZED`, compute a cheap `prefilter_score` from exact tag hits + `ts_rank_cd` + max trigram similarity, sort by that score, and cap the prefiltered pool to `GREATEST(limit * 20, 200)` before running expensive `regexp_split_to_table`, substring, and embedding scoring.
- Result: library checks are clean; the change is designed to keep relevance while cutting the number of rows that reach the expensive scoring phase.
- Checks: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --lib`.
- Status: complete.

## RS-026 — 2026-08-19 — Remove per-candidate regex tokenization from exact matching

- Goal: speed up candidate scoring further by removing repeated `regexp_split_to_table(...)` work on each candidate station row.
- Scope: replace exact term matching in `matched_count` with `s.searchable_tsv @@ plainto_tsquery('simple', qt.term)` so the scoring phase uses the precomputed `searchable_tsv` document instead of re-tokenizing station names and tags; simplify `prefilter_score` to tag hits + `ts_rank_cd`, leaving trigram similarity only for the already-limited candidate set.
- Result: library checks stay green while candidate scoring becomes cheaper for broad queries.
- Checks: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --lib`.
- Status: complete.

## RS-027 — 2026-08-19 — Fast empty result when metadata prefilter finds nothing

- Goal: avoid long over-limit requests when no suitable station exists.
- Scope: remove semantic-only fallback branch from SQL search path; if metadata prefilter has no candidates, the query now returns empty immediately instead of scanning embedding neighbors and ranking irrelevant semantic matches.
- Result: no-match path becomes fail-fast and avoids timeout-prone broad semantic fallback.
- Checks: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --lib`.
- Status: complete.

## RS-028 — 2026-08-19 — Generic bidirectional transliteration for station-name recall

- Goal: avoid brittle point-by-point brand mappings and make station-name search work across Russian and Latin spellings by default.
- Scope: `expand_transliterations` now performs generic bidirectional transliteration for every query token (ru→lat and lat→ru), in addition to existing high-signal dictionary mappings. This improves recall for names like `radio BOB` / `радио боб` and `God Radio` / `год радио` without adding per-station hardcoded pairs.
- Result: transliteration-based recall works through a general mechanism; tests were updated to cover generic token conversion.
- Checks: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --lib`.
- Status: complete.

## RS-029 — 2026-08-19 — Name-priority mode for "включи радио ..."

- Goal: improve top-1 selection when the user explicitly asks to play a station by name, especially for short names like `Rock FM`.
- Scope: derive ordered station-name hint phrases when the command starts with `включи радио` / `поставь радио` (and English equivalents), pass them into `SearchQuery`, and add a strong SQL ranking bonus for ordered phrase matches inside normalized station names. Also keep a bonus for full core-term coverage in the station name, preferring shorter exact names over longer variants.
- Result: commands in station-name mode now prioritize exact ordered name matches over broader tag/semantic matches.
- Checks: `cargo fmt`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --lib`.
- Status: complete.

## RS-018 — 2026-08-18 — Read-only administration-console preview

- Goal: make the initial administration interface visible and usable during local development.
- Scope: a server-rendered `/admin` HTML page that authenticates API calls with the already configured
  application Bearer token, shows readiness, and searches catalog stations. It deliberately excludes
  persistent administrator identities, sessions, storage, and state-changing operations.
- Result: the page keeps the entered token only in JavaScript memory for the current browser tab, calls
  `/health/ready` and the existing protected `POST /v1/search` endpoint, and removes the in-memory
  token on disconnect. The OpenAPI contract records the HTML endpoint. The local launcher accepts a
  configured token or generates a random process-only token and prints it for the preview login.
- Checks: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test`, and `git diff --check` passed. The regular suite ran 57 tests successfully;
  PostgreSQL and provider-credential-dependent live tests remained explicitly ignored.
- Status: complete.

 
 

## RS-032 — 2026-08-20 — Package layout and architecture review

- Goal: review the repository for god objects and place related files into logical Rust packages
  without changing the public HTTP contract.
- Scope: catalog import packaging, voice-command/speech packaging, compatibility facades, graphify
  architecture review, and verification.
- Result: added `src/catalog/{mod.rs,import.rs}` and `src/voice/{mod.rs,command.rs,speech.rs}`;
  updated internal consumers to use the new package boundaries while preserving the old speech and
  voice-command module paths. Graphify identified `src/http/mod.rs` and `src/search/mod.rs` as the
  next low-cohesion seams; they remain explicitly documented for a follow-up incremental split.
- Checks: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and
  `cargo test` passed (61 unit/integration tests passed; 6 external/credential/database tests
  remained ignored). The test run used an isolated target directory because the normal debug binary
  was held open by an existing local process.
- Status: complete; next refactor should extract HTTP transport DTO/error/auth modules, then split
  search domain models, in-memory repository, and orchestration.

## RS-033 — 2026-08-20 — Confidence-gated local language and country intent filters

- Goal: stop Russian voice/text requests such as `Включи английский рок`, `Включи рок из Англии`,
  and `Включи испанские новости` from losing their requested language or country before station
  search.
- Scope: add an E5-based language-label classifier that reuses the query embedding, cache its
  compact multilingual label vectors at startup, add score and runner-up-margin gates, preserve a
  validated LLM language when deterministic parsing is silent, and preserve a validated LLM country
  only for explicit `из` / `from` requests. Add the `англии` country inflection and an environment
  rollback switch that does not disable semantic station ranking.
- Result: a confident semantic match applies `language` before repository search; a low-confidence
  match applies no hard filter. `из Англии` deterministically resolves to `GB`, and the existing
  provider's `GB` output no longer disappears during normalization. `ROCKSERVER_SEMANTIC_LANGUAGE_FILTERS`
  defaults to `on` when the local E5 embedding provider is configured and accepts `off` for
  rollback. Deterministic development embeddings cannot enable hard filters.
- Checks: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and
  `cargo test` are required before handoff. Unit coverage includes acceptance/rejection thresholds,
  integration before in-memory search, `англии`, and explicit-country provider preservation.
- Status: complete. `cargo fmt --check`, strict all-target/all-feature Clippy, and `cargo test`
  passed; 67 library tests and the regular HTTP/contract suites passed, while external database and
  credential-dependent tests remained explicitly ignored.

## RS-032 — 2026-08-20 — Reconcile Windows voice documentation

- Goal: align the Windows roadmap, near-term TODO, README, and status with the implemented RockCast/RockServer voice MVP.
- Result: documentation now records that RockCast text search and default-microphone capture are implemented, and that RockServer selects the bounded, commit-time Yandex SpeechKit adapter when configured. It also records the remaining work accurately: deterministic end-to-end coverage, input-device selection, cancellation/state reporting, retention-safe logging, provider resilience, and true upstream partial recognition.
- Checks: documentation-only review against `rockcast/src/{rockserver.rs,voice/}` and `rockserver/src/{main.rs,providers/yandex_speechkit.rs}`; no code or tests changed.
- Status: complete.

## DOC-RM-004 — 2026-08-21 — Shared station catalog implementation plan

- Goal: split RM-004 into independently executable, approval-gated tasks and assign the appropriate
  Codex model and reasoning effort to every stage.
- Scope: planning only for the canonical schema, catalog repository, legacy conversion, RockServer,
  RockMobile, RockCast, release automation, cross-project review, and final cutover.
- Result: added `docs/rm-004-shared-station-catalog-plan.md` with subplans RM-004-A through RM-004-I,
  model assignments, dependencies, boundaries, acceptance gates, rollback, and completion criteria.
- Checks: documentation reviewed against the existing catalog/import graph and current project
  contributor instructions; no implementation checks were run because no code or behavior changed.
- Status: complete; implementation not started and requires an explicit follow-up request.

## RM-004-A — 2026-08-21 — Shared catalog contract and migration design

- Goal: define the proposed canonical station-catalog v1 contract and migration policy without
  starting implementation.
- Scope: read-only architecture inventory of RockServer, RockCast, and RockMobile; proposed envelope,
  station, stream, legacy-ID, tombstone, ownership, versioning, immutable-release, checksum,
  migration, coexistence, rollout, rollback, risk, and compatibility rules. Writes were limited to
  `docs/**`; the existing RM-004 plan was not modified.
- Result: added `docs/rm-004-contract-proposal.md`,
  `docs/rm-004-stations-v1.schema.proposed.json`, and
  `docs/rm-004-stations-v1.example.json`. The proposal is explicitly **PROPOSED / NOT APPROVED /
  NOT IMPLEMENTED** and lists all decisions requiring user approval before RM-004-B.
- Checks: restarted Graphify navigation sequentially at user request; RockServer query completed,
  while hanging RockCast/RockMobile queries were stopped after bounded waits and replaced by the
  documented read-only `graph.json` fallback, with all findings checked against current source.
  Both JSON files parse, the schema declares Draft 2020-12 and a stable `$id`, and the embedded and
  standalone examples match. `git diff --check` passed (with only Git's existing LF→CRLF working-
  copy warning), and the complete working-tree change set is confined to `docs/**`. Cargo/Gradle
  tests were not run because no code or build behavior changed.
- Status: proposal complete at the RM-004-A acceptance gate; awaiting explicit user approval.

## RM-004-D addendum — 2026-08-21 — RockMobile extended SQLite catalog release gate

- Goal: prepare a real, prebuilt SQLite release package for the next RockMobile update without
  misrepresenting a development-sized PostgreSQL catalog as complete.
- Scope: a provider-neutral PostgreSQL-to-SQLite export CLI, strict active/playable eligibility,
  deterministic primary-stream selection and provider-safe dedupe, exact-byte manifest hashing,
  eligibility/gap reports, schema and consumption documentation. RockMobile source and any network
  provider import remain out of scope.
- Result: added `export_mobile_catalog` and schema-versioned SQLite output containing only stable
  station/provider identity, discovery metadata, normalized name/tags and selected primary stream.
  It excludes health/probe data, embeddings, import runs, timestamps, credentials and provider
  operational metadata. A complete release requires at least 16,000 eligible stations. The verified
  local disposable database initially had 42 and correctly emitted only a gap report. After the
  local `rockserver` migrations and checksum-pinned baseline activation, the verified real catalog
  had 16,825 eligible rows and produced the complete `2026.08.2-mobile.1` SQLite, manifest, and
  eligibility report. The exact SQLite SHA-256 is
  `ad469d405f177d7e476cf9b3d9985497d0e2c6132ac0f3ce14485f4eab402073`.
  `docs/rockmobile-extended-catalog.md` documents exact file names, tables, columns, indexes,
  checksum verification, Room bundling, and release procedure.
- Checks: SQLite fixture passed integrity, user/schema version, metadata/count, FTS-row-count,
  ordinary search-index, and SHA-256 checks. The live PostgreSQL export recorded the 42/16,000
  gap. Final Rust, catalog, graph, and diff checks are recorded in the task handoff.
- Status: complete; verified bundleable RockMobile artifact released at the RM-004-D acceptance
  gate. Android consumption remains a separately approved RM-004-E/F activity.

## RM-004-G — 2026-08-25 — Release and synchronization automation

- Goal: make immutable shared-catalog releases and consumer snapshot updates explicit, offline,
  reproducible, and checksum-gated.
- Scope: local catalog release tooling/tests/documentation and vendored snapshot metadata/artifacts
  in RockServer, RockMobile, and RockCast; no runtime fetches, releases, pushes, or legacy removal.
- Result: `tools/release_sync.py` publishes immutable `release/<version>` directories and validates
  selected releases before syncing a consumer. It supports dry-run, drift verification, and rollback
  to a retained prior release. RockMobile validates its independent extended SQLite manifest/hash,
  integrity, DB schema, metadata version, and count without conflating it with baseline JSON.
- Checks: catalog-tool unit suite passed, release publication/dry-runs/sync/verify passed for all
  consumers. Consumer build/test checks are recorded with this handoff.
- Status: complete at RM-004-G acceptance gate; RM-004-H/I not started.

## RM-004-H remediation — 2026-08-25 — Preserve catalog lifecycle metadata and fail closed

- Goal: resolve the two High findings in `docs/rm-004-h-cross-project-review.md` without changing
  the public HTTP/OpenAPI contract or the ownership of Radio Browser records.
- Scope: retain canonical tombstones and replacement semantics in RockServer preflight, activation,
  PostgreSQL persistence, and internal lookup; prevent an invalid production pin from falling back
  to the unrelated six-station test fixture.
- Result: `PinnedSharedCatalog` exposes validated tombstones and returns `Redirect` only for a
  single-target merge; splits return `Ambiguous` and removed IDs return `Removed`. Migration 0011
  adds the provider-scoped active lifecycle table. Transactional activation updates the lifecycle
  view, station/stream retirement, and import run atomically; reimport is idempotent and rollback
  by reactivating the prior release removes the newer tombstones and reactivates its stations.
  Search continues to exclude retired station/stream rows. Radio Browser is never included in
  RockCatalog lifecycle writes. Pinned checksum/schema/semantic failures now surface as startup or
  readiness failure and leave any already-active PostgreSQL release untouched; no production path
  can select the fixture, which is test-only.
- Checks: catalog unit coverage includes removed/merged/split semantics and invalid artifact
  rejection; an ignored disposable-PostgreSQL integration scenario covers activation, active search
  exclusion, replacement lookup, idempotence, rollback, and Radio Browser coexistence. Full Rust
  verification is recorded with the handoff.
- Status: complete pending the recorded verification results; RM-004-I was not started.

## RM-004-I — 2026-08-25 — Cutover and legacy cleanup

- Goal: complete the RM-004 cutover without weakening the already-remediated lifecycle/tombstone
  and corrupt-pin safety findings.
- Scope: synchronize and verify the approved immutable baseline across all consumers; remove only
  expired legacy paths; document source ownership, baseline/extended flow, rollback, and exceptions.
- Result: designated RockCatalog `2026.08.2` / SHA-256
  `3fa20dca94fc059bd433a47b9fba9bb6d5e5e1aa2957a5ffb58b2a7b20b1d74d` as the shared consumer
  baseline. RockMobile retains its separately verified `2026.08.2-mobile.1` SQLite package
  (16,825 records; SHA-256 `ad469d405f177d7e476cf9b3d9985497d0e2c6132ac0f3ce14485f4eab402073`).
  The only retained legacy item is RockCast's user-override TXT adapter: owner RockCast
  maintainers, reason preserve offline overrides, removal date 2026-10-31. No manual consumer-copy
  route is supported; `release_sync.py sync` and `verify` are the required offline workflow.
- Checks: catalog tooling tests passed 12/12; `release_sync.py verify` passed for RockServer,
  RockCast, and RockMobile; exact baseline and extended hashes, SQLite integrity/schema/count/FTS,
  RockServer `cargo fmt --check`, strict Clippy, and `cargo test` passed (81 regular tests);
  RockCast fmt, strict Clippy, and `cargo test` passed (50 regular tests). A loopback in-memory
  RockServer readiness smoke returned 200 and was stopped cleanly. RockMobile Gradle unit/lint
  checks were blocked before compilation because the configured API-36 SDK metadata is inaccessible
  and licenses are unaccepted; no download, license acceptance, device, network, provider, or live
  database test was performed.
- Status: cutover work complete locally, but the RM-004-I acceptance gate is not fully passed until
  RockMobile's offline unit/device verification can run against a readable, licensed Android API-36
  SDK. This is an environment handoff item, not a claimed successful check.

## Shared roadmap — 2026-08-25 — Internet beta planning

- Goal: create a cross-project execution route for RockServer, RockCast, RockMobile, and a future
  ESP32 client before public internet testing.
- Scope: documentation only in `docs/shared-product-roadmap.md`, plus status/task recording.
- Result: RM-007 → RM-011 → RM-012 is the current priority. The plan defines local personal data,
  account/session/device registration, synchronization and remote control, then ESP32 pairing after
  hardware arrives. It recommends a single-VPS Docker Compose/Caddy/private-PostgreSQL deployment
  with immutable-image releases, backup, readiness checks, and rollback.
- Checks: documentation reviewed for scope; `git diff --check` to be run before handoff.
- Status: planned; no implementation, live deployment, external access, credentials, API, or client
  code changes performed.

## Shared roadmap — 2026-08-25 — Executable task decomposition

- Goal: turn the internet-beta roadmap into bounded tasks that can be separately approved and sent
  to Codex.
- Scope: documentation only: `docs/shared-product-execution-plan.md`, roadmap link and status/task
  records.
- Result: created GATE-001, four RM-007 tasks, four OPS-001 deployment tasks, six RM-011 account
  tasks, six RM-012 sync/control tasks and two post-hardware ESP32 tasks. Each records repository
  boundary, dependency, model/reasoning, work and acceptance gate.
- Checks: `git diff --check` to be run before handoff.
- Status: planned; this decomposition authorizes no implementation or deployment.

## RM-007-D — 2026-08-25 — Cross-client local personal-data review

- Goal: verify RM-007-A field mapping, stable identity, lifecycle, offline-first behavior,
  migration/restart safety and rollback across the existing RockMobile RM-007-B and RockCast
  RM-007-C working trees without changing either client implementation.
- Scope: read-only client/catalog/source/test review and local offline verification; writes limited
  to RockServer `docs/**`. Existing dirty changes in all repositories were preserved. No network,
  production service, secret, live database, device test, shared catalog mutation or commit.
- Result: added `docs/rm-007-d-cross-client-review.md`. Baseline/catalog identity is consistent and
  pure lifecycle rules avoid automatic split selection, but four High blockers remain: incompatible
  RockMobile profile/timestamp shape, destructive fail-open Mobile profile reading without
  migration rollback, unwired Mobile lifecycle reconciliation, and URL-derived RockCast remote/
  voice history identity. RM-011-A is blocked; OPS-001-A remains independent design-only work.
- Checks: release verification passed for all three consumers including the Mobile extended
  manifest; catalog-tool tests passed 12/12; RockServer `cargo test catalog --lib` passed 14/14;
  RockCast `cargo test personal_data --lib` was initially blocked by its ordinary cargo lock and
  then passed 5/5 with `--target-dir C:\repos\rockserver\target\rm007d-rockcast`. RockServer fmt,
  strict Clippy and full tests passed. RockCast fmt, strict Clippy and full tests passed (55 library
  + 2 relay integration; 8 live-network tests ignored). RockMobile
  targeted unit and lint invocations used process-local
  `-Duser.home=C:\Users\alex` sequentially, but both stopped before compilation on the inaccessible
  Gradle wrapper `.zip.lck`; lint is not passed and the three known errors were not cleared.
- Status: **not passed**; remediate every High finding and repeat the same-fixture cross-client
  review before RM-011-A.

## RM-007-D remediation — 2026-08-25

- Goal: remediate every High/Medium implementation finding from the cross-client review without
  changing shared catalog data or adding server/auth/sync functionality.
- Result: RockMobile portable v1 storage, safe legacy migration/rollback, lifecycle integration and
  resolver edge cases were implemented; RockCast canonical remote identity, favourite merge,
  migration counts and explicit rollback were implemented. Client documentation was updated.
- Checks: RockCast fmt, strict Clippy and full tests passed (55 library + 2 relay integration; 8
  live-network ignored). RockMobile targeted unit and lint commands used the required sequential
  process-local `-Duser.home=C:\Users\alex` policy but both stopped before compilation on the
  inaccessible Gradle wrapper lock.
- Status: implementation findings remediated; cross-client gate remains **not passed** until
  RockMobile compile/unit/lint verification can execute successfully.

## OPS-001-B — 2026-08-25 — Reproducible container and Compose stack

- Goal: provide a reproducible local container/runtime foundation for the approved single-VPS
  Caddy, RockServer and private PostgreSQL design.
- Scope: root Dockerfile and ignore rules; Compose base plus local/production overrides; local and
  production Caddy templates; safe environment example; local preflight/startup script; fail-closed
  startup configuration requiring `ROCKSERVER_API_BEARER_TOKEN` and `DATABASE_URL`; deployment
  documentation. No VPS, DNS, registry, production secret, public port, database or deployment was
  changed.
- Result: the service now accepts the configured Bearer credential instead of a source-embedded
  bootstrap value and refuses to select the in-memory catalog when `DATABASE_URL` is absent. The
  Compose base keeps RockServer and PostgreSQL un-published; local Caddy is loopback-only and the
  production override publishes only Caddy 80/443. Healthchecks cover PostgreSQL, RockServer and
  Caddy; the verification script validates Compose without printing environment values and can run
  a disposable local readiness smoke.
- Checks: Docker image build passed. `deploy/verify-compose.ps1 -Mode local -Start` passed with
  healthy PostgreSQL, RockServer and Caddy and loopback Caddy `GET /health/ready` HTTP 200; it
  removed only its disposable project. Production rendering passed and exposed only Caddy `80` and
  `443`; PostgreSQL and RockServer had no host ports. `cargo fmt --check`, strict all-target/all-
  feature Clippy, and serial-target `cargo test` passed (82 regular tests plus HTTP/OpenAPI/
  WebSocket suites; external/credential/asset tests remained ignored). `graphify update .` passed.
- Status: **passed locally**; public domain/DNS/VPS/registry/secrets/firewall/deployment remain
  manual OPS-001-D/production actions and were not performed.

## OPS-001-C — 2026-08-25 — CI image, release, backup and rollback runbook

- Goal: make commit-SHA image verification, manual release approval, backup-before-deploy,
  readiness-gated rollout, previous-image rollback and non-production restore rehearsal explicit
  and reproducible without using production infrastructure.
- Scope: `.github/workflows/ci-release.yml`, `deploy/release.ps1`,
  `deploy/restore-rehearsal.ps1`, and the OPS-001-C sections of `deploy/README.md` plus current
  status/task records. No VPS, DNS, public endpoint, registry publication, production database,
  secret, client/catalog change or commit was made.
- Result: CI now runs format/Clippy/tests, labels a built image with the source commit SHA and
  executes a loopback Compose readiness smoke. GHCR publication is manual and protected by the
  `release-gate` environment. The release script enforces digest-pinned images and production port
  isolation, creates a custom-format PostgreSQL backup before deploy, records its checksum, waits
  for health/readiness, and supports previous-digest rollback without editing a running container.
  The restore script uses a disposable PostgreSQL network, `pg_restore`, restored-table validation,
  and an in-network RockServer readiness check; all temporary resources are cleaned by default.
- Checks: PowerShell parser checks passed for `deploy/release.ps1`,
  `deploy/restore-rehearsal.ps1`, and `deploy/verify-compose.ps1`. Production Compose rendering
  passed with only Caddy ports 80/443. The pinned Docker image built successfully. The local
  Compose stack reached healthy PostgreSQL, RockServer and Caddy states and loopback readiness
  returned HTTP 200. Release `preflight -DryRun` and `rollback -DryRun` passed without starting
  containers or contacting a deployment target. A corrected disposable `pg_dump`/`pg_restore`
  rehearsal restored the database and returned application readiness; the disposable network and
  containers were removed. The initial rehearsal findings (PowerShell binary redirect and omitted
  restore username) were fixed before the passing run. Final `git diff --check`,
  `cargo fmt --check`, strict all-target/all-feature Clippy, and `cargo test` passed; the ordinary
  suite reported 82 passed tests with only explicitly ignored PostgreSQL/provider/asset cases.
  `graphify update .` passed and refreshed the local graph to 1,683 nodes, 3,204 edges and 103
  communities; its existing zero-node JSON warnings were non-fatal.
- Status: **passed locally**, pending only manual external release/deployment actions. GHCR
  publication, production backup/deploy, public HTTPS readiness and live rollback were not run.

## OPS-001-D — 2026-08-25 — Registry-free single-VPS bootstrap and staging update

- Goal: make the approved single-VPS staging path owner-operated from one ignored inventory and one
  local update command, without GitHub/GHCR, a registry, or unsafe password handling.
- Scope: deployment launcher/module/remote script, Docker seed binary, Compose seed/ONNX wiring,
  examples, local script tests and current deployment documentation. No VPS, DNS, registry, GitHub
  secret, live credential, ONNX asset download, commit or push was used.
- Result: the inventory accepts exactly SSH user/host/domain and no password. First SSH access is an
  explicit OpenSSH prompt followed by generated-key use; bootstrap provides TTY-backed sudo and a
  validated command-scoped rule, while normal deploy is non-interactive. A clean current commit is
  locally built/tagged/labeled, saved and checksummed, then loaded and identity-checked on the VPS.
  Protected env merging preserves generated DB/API secrets, writes distinct lines, and transfers
  only the four allowlisted Yandex names. Backup → embedded migrations → pinned idempotent full
  catalog activation → HTTPS readiness and opt-in checksum-gated ONNX remain fail closed.
- Checks: focused PowerShell tests passed for registry independence, commit/image identity,
  password-free inventory, secret-safe output/env serialization, TTY/sudo construction, and current
  catalog/ONNX safeguards. Dry-run, production Compose rendering, PowerShell parsing, Rust fmt,
  strict Clippy, full tests, and diff checks passed. An all-features Docker build downloaded locked
  dependencies and entered compilation, then was stopped at the owner's request before completion;
  Docker image build is therefore not claimed as passed. External actions remain unverified.
- Status: **passed locally** pending the owner’s real VPS/DNS/firewall/backup-monitoring setup and a
  deliberate staging launch. A registry is not a staging prerequisite.

## Station icons — шаг 0 — 2026-08-26 — Контракт и границы

- Goal: зафиксировать совместимый RockServer-first контракт и границы station icons до реализации.
- Scope: только документация в `docs/roadmap/station-icons.md` и текущие status/task records. Не
  менялись Rust, SQL migrations, router, OpenAPI, фактическое HTTP-поведение или service diagram.
- Result: roadmap теперь явно помечает будущий `GET /api/v1/stations/{id}/icon` и nullable
  `faviconUrl` как planned state; внешний `source_url` остаётся внутренним. Зафиксированы приоритет
  source URL → homepage favicon → отсутствует, запрет network I/O в migrations и request path,
  v1 WebP/raster decision и лимиты, `200`/`304`/`404`, cache policy и безопасный rollout/rollback.
- Checks: reviewed `AGENTS.md`, roadmap, architecture/status/tasks, current OpenAPI and service
  diagram; current OpenAPI was confirmed not to expose the planned endpoint or `faviconUrl`.
  Required-contract text and trailing-whitespace checks, `git diff --check`, and a no-index diff
  check for the untracked roadmap file passed.
- Status: **documentation step complete**; feature remains unimplemented and requires separately
  approved roadmap steps.

## MVP-001-A — 2026-08-26 — Public API contract and abuse model

- Goal: define an approval-only anonymous public `/v1` contract and abuse model for the MVP-001
  zero-config catalog/search/voice path.
- Scope: `docs/mvp-001-a-public-api-contract.md`,
  `docs/mvp-001-a-openapi.proposed.yaml`, and current status/task records only. Existing dirty
  roadmap/status/task work was preserved. No Rust, runtime endpoint, current `api/openapi.yaml`,
  RockCast/RockMobile, secret, network, live provider, commit or push was changed.
- Result: the proposal names exactly five anonymous operations, bounds all request/audio/session/
  concurrency resources, distinguishes `429` quota exhaustion from global `503` voice capacity,
  fixes safe error/metric/logging fields and fail-closed protected endpoint families. It explicitly
  rejects shared Bearer tokens and all client secrets as public-user authorization. It contains
  acceptance criteria and five human-approval decisions before MVP-001-B.
- Checks: reviewed `AGENTS.md`, current OpenAPI and MVP-001 roadmap/execution plan; Markdown
  whitespace, proposed-OpenAPI structural/policy assertions (five anonymous operations, no
  credential security scheme), current OpenAPI unchanged, and `git diff --check` passed. Cargo
  checks are not applicable because no code or runtime behavior changed. The Graphify navigation
  query left two confirmed helper processes; both were stopped and verified exited.
- Status: **proposal complete; awaiting human approval**. No public endpoint is implemented or
  declared current by this task.

## MVP-001-B — 2026-08-26 — anonymous public endpoint policy

- Goal: implement the approved anonymous `/v1` catalog/search/voice allowlist without a shared
  release-client credential, while retaining Bearer protection for `/api/v1` aliases.
- Result: bounded catalog/search/voice handlers, direct-peer fail-closed quotas, global voice
  admission, safe error bodies and redacted public-path diagnostics were added. No client,
  provider, secret, network, deployment, commit, or push was used.
- Checks: `cargo fmt`, `cargo check`, and focused deterministic search, voice-command and
  WebSocket tests passed. Full required Clippy/test and OpenAPI reconciliation remain pending.
- Status: implementation in progress; no public rollout is authorized.

## RM-011-A — 2026-08-26 — account/device contract and threat model

- Goal: prepare the smallest secure approval-only account, session, device and future pairing
  contract after the owner-confirmed RM-007-D/MVP-001 gates, without implementing authentication.
- Scope: `docs/rm-011-a-auth-device-contract.md`,
  `docs/rm-011-a-openapi.proposed.yaml`, and factual roadmap/status/task records only. Existing
  unrelated changes were preserved. No runtime OpenAPI, Rust, PostgreSQL migration, client UI,
  email/SMS, production secret, network, provider, commit or push was used.
- Result: revised by owner direction to remove email/password/SMS and require no RockMobile
  installation. The proposal now uses a passkey in a first-party mobile browser to approve a
  desktop-originated QR/short-code request; the desktop alone receives its distinct opaque
  10-minute access/30-day rotating-refresh session. The threat model fixes separate desktop and
  approval proofs, safe errors, rate-limit keys, redacted logs and minimum audit events. It lists
  eight owner decisions plus WebAuthn/migration prerequisites for RM-011-B/C. Phone-first browser
  registration is explicit, and an additive `AccountIdentity` boundary preserves a future optional
  email or explicit-login method without redesigning user/device/session ownership.
- Checks: new proposal Markdown heading/whitespace check passed; a temporary no-runtime Rust test parsed the
  revised proposed OpenAPI and asserted phone-first passkey registration plus QR/short-code pairing,
  removal of the old register/login routes and email/password fields, opaque desktop bearer,
  browser-cookie scheme and reserved additive-identity boundary (1 passed), then was removed.
  `cargo fmt --check`,
  strict all-target/all-feature Clippy, `cargo test` (84 library + 18 regular integration tests;
  7 configured external/PostgreSQL tests ignored), and `git diff --check` passed. Current
  `api/openapi.yaml` was inspected and deliberately left unchanged because it has no matching
  runtime behavior.
- Status: **proposal approved by the owner on 2026-08-26**. No account/device endpoint is current.
  The proposal intentionally preserves explicit recovery/retention/operational alternatives; the
  relevant option and listed RM-011-B/C prerequisites require approval before implementation.

## RM-011-B — 2026-08-26 — approved account/session persistence subset

- Goal: implement only RM-011-A persistence boundaries that do not require selecting unresolved
  recovery, retention, WebAuthn-operation, proxy, browser or credential policy.
- Scope: migration `0012`, passkey-only account/device/session domain and PostgreSQL store, plus
  deterministic unit and opt-in PostgreSQL integration coverage. No password/email identity,
  browser/pairing, WebAuthn verification, rate limit, HTTP endpoint, client, external provider,
  secret, deployment, commit or push was added.
- Result: users/devices/sessions/refresh-token chains and audit classifications persist hashes
  only; a used refresh token revokes its session family atomically. Account deletion tombstones the
  user and revokes active devices, sessions, refresh tokens and passkeys. The reserved identity
  table is unused and does not introduce login behavior.
- Checks: `cargo fmt --check`, offline compile, strict Clippy and full tests were run locally;
  the PostgreSQL integration test is deliberately ignored unless `TEST_DATABASE_URL` points to a
  disposable database. No live database was provided by this task.
- Status: **approved subset complete locally**. RM-011-C remains blocked until RM-011-B2 review and
  runtime OpenAPI reconciliation; this historical B1 entry does not claim those later gates.

## RM-011-B2 — 2026-08-26 — browser, pairing, WebAuthn-boundary and rate-limit persistence

- Goal: implement the approved second persistence boundary without adding public HTTP routes or
  client UI: browser sessions, one-time pairing requests, WebAuthn challenge context/sign-count
  guards, and PostgreSQL-backed rate-limit buckets.
- Scope: migration `0013_add_browser_pairing_and_rate_limits.sql`, `src/auth/mod.rs`,
  `src/persistence/account_postgres.rs`, deterministic auth unit tests and an opt-in PostgreSQL
  integration test. No raw token, QR/code, cookie, WebAuthn assertion, password, email, HTTP route,
  runtime OpenAPI or client change was added.
- Result: browser approval requires a live, recently reauthenticated session; pairing approval and
  completion are single-use and transactional; completion enforces the ten-device cap; challenges
  are origin/RP/ceremony-bound and single-use; sign-count rollback is rejected; rate-limit buckets
  increment atomically in PostgreSQL. Existing account deletion revokes the new browser sessions;
  pairing/challenge records remain server-only.
- Checks: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test` (88 unit/library tests and regular integration suites), and the disposable PostgreSQL
  test `postgres_b2_browser_pairing_webauthn_and_rate_limits -- --ignored --nocapture` passed. The
  normal compose volume was not modified; a temporary disposable container was removed afterward.
- Status: **B2 persistence boundary complete locally**. Cryptographic WebAuthn signature provider,
  HTTP routes and runtime OpenAPI reconciliation remain RM-011-C prerequisites.

## RM-011-C — 2026-08-26 — unified first-party web shell (partial)

- Goal: establish the agreed single frontend/admin technology and static delivery boundary.
- Scope: added `web/` TypeScript + Vite + Preact project with shared API client/types and pairing
  presentation, plus a Caddy image that builds and serves it while proxying API routes.
- Result: `/admin` is no longer a separate public frontend when accessed through Caddy; it is
  handled by the same SPA bundle. No secret is bundled or persisted by the UI.
- Checks: TypeScript `tsc --noEmit` passed using the installed dependency runtime. Vite production
  build is blocked locally because the environment's package policy refuses esbuild's required
  install script; it must be run in CI/container with the approved dependency-build policy.
- Status: **partial**. No RM-011-C auth API, WebAuthn verifier, session cookie/CSRF handling or
  runtime OpenAPI update is claimed by this UI-only increment.

## RM-011-C — 2026-08-26 — pairing request and lookup runtime slice (partial)

- Scope: production startup now supplies the existing PostgreSQL account store to the HTTP router;
  added pairing-request creation and neutral short-code lookup, with the corresponding runtime
  OpenAPI paths.
- Result: request proofs are UUID-derived opaque values sent only to the desktop initiator and
  persisted as SHA-256 hashes; PostgreSQL calculates the ten-minute expiry. Lookup returns only
  the non-secret preview already defined by B2.
- Checks: `cargo fmt --check`, strict Clippy, `cargo test` (88 tests), and `git diff --check`
  passed. Disposable PostgreSQL integration tests remain opt-in and were not run.
- Status: **partial**. No approval/completion route, rate-limit use, trusted-proxy validation,
  WebAuthn, browser cookie/CSRF boundary, or end-to-end pairing claim is made.

## RM-011-C — 2026-08-26 — browser-proof approval slice (partial)

- Scope: migration `0014_add_browser_session_cookie_hash.sql`, browser-proof persistence method,
  `POST /v1/pairing-requests/{request_id}/approve`, CSRF header/cookie checks and OpenAPI entry.
- Result: approval no longer accepts a bare/guessable browser session ID; the persistence query
  requires the opaque cookie hash, CSRF hash, active session and fresh passkey timestamp.
- Checks: offline cargo check, strict Clippy and targeted API test passed.
- Status: **partial**. Browser session issuance still requires WebAuthn registration/authentication;
  trusted-proxy enforcement and desktop completion remain outstanding.

## RM-011-C — 2026-08-26 — pure-Rust WebAuthn browser ceremonies (partial)

- Scope: added `passkey-auth` with fixed RP ID/origin policy, registration and authentication
  options/verify routes, PostgreSQL opaque challenge state blobs, credential lookup and atomic
  sign-counter advancement, plus Secure/HttpOnly/SameSite browser cookie issuance.
- Result: registration and authentication verification now perform cryptographic client-data,
  RP-ID, challenge, user-verification and signature checks; replay/rollback is rejected by the
  existing B2 challenge and counter guards. No OpenSSL dependency is required.
- Checks: sequential offline `cargo check` passed after dependency download. Full Clippy/test
  rerun is pending after this slice; no disposable PostgreSQL run was performed.
- Status: **partial**. Pairing completion, trusted proxy secret binding and full web UI ceremony
  wiring remain outstanding.

## RM-011-C — 2026-08-26 — desktop pairing completion slice (partial)

- Scope: added `POST /v1/pairing-requests/{request_id}/complete`; it delegates to B2's atomic
  approval/consumption transaction, enforces the owner UUID, ten-device cap and one-time desktop
  proof, and issues short-lived native access plus rotating refresh credentials.
- Result: access expires after 15 minutes and refresh after 30 days using PostgreSQL's clock; raw
  credentials are returned only in the response and never logged.
- Checks: sequential offline `cargo check` passed; full test and Clippy rerun remains pending.
- Status: **partial**. End-to-end browser UI wiring and final security review remain.

## RM-011-C — 2026-08-26 — browser ceremony UI wiring (partial)

- Scope: enabled the Preact UI's real `navigator.credentials.create()` path, strict base64url
  serialization of attestation data, registration verify call, in-memory CSRF token and pairing
  approval button. QR links may carry the approval secret without persisting it.
- Result: the browser can register a passkey and approve a looked-up device from the same origin;
  no token is written to localStorage or bundled into the frontend.
- Checks: source changes are complete; TypeScript build must run in the Caddy image because the host
  package policy blocks esbuild's install script.
- Status: **superseded by the current RM-011-C snapshot below**.

## RM-011-C — 2026-08-26 — unified auth/pairing delivery and verification (current snapshot)

- Goal: finish the first-party browser API/UI slice on the approved B2 persistence boundary using
  one TypeScript + Vite + Preact stack for user and admin surfaces.
- Result: cryptographic passkey registration and authentication, cookie/CSRF/origin checks,
  authenticated Caddy proxy proof, PostgreSQL-backed pairing/rate-limit usage, pairing approval,
  desktop completion, and a shared Preact UI with passkey controls, short-code lookup, device and
  verification-phrase display, and in-memory QR rendering are implemented. Native access/refresh
  tokens are returned only by the desktop completion endpoint; the browser does not persist them.
- Checks: `cargo fmt --check`, strict Clippy with `--jobs 1`, `cargo test --jobs 1` (93 tests passed),
  OpenAPI contract tests, and all four opt-in PostgreSQL integration tests against a disposable
  `pgvector/pgvector:pg17` container passed sequentially.
  `pnpm install --frozen-lockfile`, `pnpm typecheck`, `pnpm lint`, and `pnpm build` passed with
  the bundled Node runtime. No git push was performed.
- Status: **complete — RM-011-C server/browser implementation verified**. The first-party UI and
  server API are validated autonomously; real-client RockCast/RockMobile staging E2E is deferred
  to RM-011-D/E and is not a prerequisite for this task.

## RM-011-C — 2026-08-26 — native session and device HTTP surface

- Goal: expose the B2 native-session persistence primitives through owner-scoped HTTP routes.
- Scope: added refresh rotation with transactional access-token replacement, logout/family
  revocation, account profile projection, active-device listing, and owner-checked device revoke;
  updated both router constructors and `api/openapi.yaml`.
- Checks: `cargo fmt`, `cargo check --all-targets --all-features --jobs 1`, strict Clippy, full
  `cargo test --all-targets --all-features --jobs 1` (93 passed), OpenAPI contract tests, and all
  four PostgreSQL integration tests against a disposable pgvector container passed.
- Status: **complete** at the RM-011-C server/browser boundary; real-client E2E is deferred to
  RM-011-D/E.

## RM-011-C — 2026-08-26 — completion owner derivation

- Goal: remove the impossible native requirement to know a browser-approved account UUID.
- Result: completion request payload is now only `{ "desktop_token": "…" }`; the locked approved
  pairing request is the sole source of owner identity for the device/session transaction.
- Checks: PostgreSQL integration asserts completion returns the approved owner and rejects a replay;
  strict JSON schema rejects an extra forged `user_id` field at the HTTP boundary.
- Status: **complete**.

## HTTP transport refactor — 2026-08-26

- Goal: reduce the `src/http/mod.rs` god object without changing HTTP behavior.
- Scope: moved the public facade to `src/http/mod.rs`, route and DTO implementation to
  `src/http/endpoints.rs`, and shared `AppState`/anonymous admission controls to
  `src/http/state.rs`.
- Result: `mod.rs` is now a 10-line export surface; endpoint behavior, route registration, and
  existing test boundaries remain unchanged.
- Checks: `cargo fmt --check`, strict all-target/all-feature Clippy, and `cargo test` passed;
  92 unit tests and all non-ignored integration tests passed.
- Status: **complete locally**.

## Limit local Cargo build parallelism — 2026-08-26

- Goal: prevent Cargo builds from consuming the whole workstation.
- Scope: added `.cargo/config.toml` with `[build] jobs = 2`.
- Result: project-local Cargo commands now use two compilation jobs by default; acceptance checks
  explicitly used `--jobs 1` for sequential execution.
- Checks: configuration is valid TOML; no source behavior changed.
- Status: **complete locally**.
## RM-011-G3 — 2026-08-28 — browser account and device centre

- Goal: provide one safe browser view proving that RockCast and RockMobile belong to the same
  named account, with device rename, revoke and current-browser logout.
- Scope: added cookie/proxy/CSRF-protected browser account endpoints and Preact account centre;
  device responses omit account IDs, tokens, credentials and audit details, while an opaque route
  handle remains internal to the browser UI and is never rendered.
- Result: rename rejects empty, control-character and over-128-character names; rename/revoke
  remain owner-scoped and auditable. Revoke ends all native sessions and refresh tokens on the
  selected device, whereas browser logout revokes only the current browser session. The UI uses
  explicit confirmations and explains that these two actions are different.
- Checks: `cargo fmt --check`, strict all-target/all-feature Clippy, and `cargo test` passed
  (100 tests; five disposable PostgreSQL tests intentionally ignored without
  `TEST_DATABASE_URL`). TypeScript typecheck/lint, Vite build and five deterministic browser UX
  regressions passed using the checked-in web dependencies. No staging deploy, commit or push.
- Status: **complete locally; staging/physical-phone smoke remains.**
