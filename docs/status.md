# Project status

Last updated: 2026-08-28

## SECURITY-001 — Rust TLS dependency remediation (deployed, 2026-08-28)

GitHub Dependabot reported one high and two low advisories for `rustls-webpki 0.101.7`,
reachable through `yandex-cloud 2025.4.14 → tonic 0.9`. The published Yandex crate has no
compatible release that removes this chain, so the unused parts of its generated SpeechKit API
were not vendored. RockServer now owns a small, checked-in SpeechKit v3 protocol boundary for
the existing bidirectional `Recognizer/RecognizeStreaming` RPC and uses `tonic 0.14.6`,
`tonic-prost 0.14.6`, and `rustls 0.23.43` with native roots and ring TLS support.

The streaming endpoint, API-key metadata, PCM16 mono configuration, explicit EOU event, partial
and final transcript handling, endpoint validation, and all HTTP/security behavior are unchanged.
Wire-tag regression tests cover the SpeechKit audio request and partial response. `cargo tree`
contains no `rustls-webpki 0.101.7`, `tonic 0.9`, or `yandex-cloud`; the remaining
`rustls-webpki` is `0.103.14` through the maintained `rustls 0.23` graph. Local `cargo fmt
--check`, strict all-target/all-feature Clippy, locked compilation, full all-target/all-feature
tests (102 passed; five disposable PostgreSQL, four billable Yandex LLM, one SpeechKit, and one
ONNX test remain explicitly ignored) and `git diff --check` passed. Commit
`266b9740ae2f0489952f46a15e80a18791d09a7e` was pushed to `origin/master` and deployed through
the OPS-001-D rollout. The remote worker reported `status=succeeded`; the immutable server image
`sha256:a3a9fcad59fd6ef464261d29dd5409aa411f3ee7e9add5c65689078d8adddd5b` passed the public
readiness gate.

## REFACTOR-001 — HTTP transport module extraction (deployed, 2026-08-28)

The behavior-preserving HTTP extraction is complete for the first slice. The former 3,704-line
`src/http/endpoints.rs` is now a 392-line composition root. Route handlers and their local
transport types live in `auth.rs`, `account.rs`, `pairing.rs`, `catalog.rs`, `search.rs`, and
`voice.rs`; shared request/response and trust-boundary helpers live in `transport.rs`, admission
state remains in `state.rs`, and liveness/admin behavior lives in `health.rs`.

All existing routes, OpenAPI files, migrations, serialization, authentication, CSRF, proxy trust,
pairing ownership, audit classifications, rate limits, and voice limits are unchanged. Security
and DTO regressions now sit beside their transport boundaries. Verified locally with formatting,
strict Clippy, full Rust tests (100 passed; five disposable PostgreSQL tests remain opt-in without
`TEST_DATABASE_URL`), OpenAPI contract tests, and `git diff --check`. No web files were changed.

Commit `2afcae0` was pushed to `origin/master` and deployed through the OPS-001-D staging
rollout. The immutable server image was accepted by the remote worker and the public readiness
gate passed.

The next reasonable extractions are `src/persistence/account_postgres.rs` (1,150 lines),
`src/search/mod.rs` (1,103 lines), and the PostgreSQL integration test module (1,184 lines); they
are intentionally left for separate, independently verifiable tasks.

## HTTP transport maintainability (2026-08-28)

Added the always-applied project rule `.cursor/rules/http-module-boundaries.mdc`: new HTTP work
must enter a domain transport module, keep router composition thin, and include a post-change
review for misplaced responsibilities and dead code. The existing `src/http/endpoints.rs` remains
a legacy mixed transport module and needs a dedicated, behavior-preserving extraction of account,
pairing, catalog, and voice domains before further broad endpoint work.

## RM-011-G3 — browser account and device centre (local, 2026-08-28)

The authenticated first-party browser view now presents one named Rock account and its active
RockCast/RockMobile devices without rendering UUIDs, credentials, tokens, or audit data. It shows
the human device name/type, connection time, most recent activity when available, an aggregate
active/inactive native-session status, and the 10-device limit with an explanation for freeing a
slot. Anonymous landing remains a sign-in/pairing entry point rather than an account centre.

`GET /v1/browser/account` is cookie/proxy-bound and returns only the safe browser projection.
Rename, revoke, and browser logout use separate first-party trusted-proxy, Origin and CSRF checks;
the server derives ownership from the cookie, writes only safe audit classifications, and owner
checks every target device. Browser logout revokes only its cookie session. Device revoke revokes
the target's native sessions/refresh tokens and explicitly cannot terminate the current browser
session. Existing native refresh and revoke semantics are unchanged.

Verified locally: `cargo fmt --check`, strict all-target/all-feature Clippy, and `cargo test`
(100 tests passed; five disposable PostgreSQL tests stayed opt-in without `TEST_DATABASE_URL`).
The checked-in web dependencies also passed TypeScript typecheck/lint, Vite production build, and
five deterministic UX regressions. Remaining acceptance is the requested staging/physical-phone
smoke test; no staging deployment, account creation, credential ceremony, commit, or push was run.

## RM-011-G2 — browser account and pairing UX (deployed to staging, 2026-08-28)

The ordinary page is now a safe account landing page: it states whether the browser is signed in,
offers username-less passkey sign-in when anonymous, and has no general pairing-code or device
flow. A secure link carrying the existing opaque code and approval secret opens only that pending
request. It displays the human device name/product, verification phrase, short code and expiry;
it never renders request IDs, account UUIDs, credential IDs or native tokens.

Anonymous pairing makes the two choices explicit: sign in with an existing passkey, or create a
new Rock account with an unambiguous warning. The current URL remains unchanged through both
ceremonies, so the original request survives login/registration and reloads. An authenticated
browser sees the exact confirmation question for its account, with connect and cancel actions.
`POST /v1/auth/browser-session` rotates a tab-local CSRF value for a live cookie session; it is
first-party/proxy-bound, no-store, returns only the display name and CSRF value, and lets the
specific pairing screen recover safely after reload without browser token storage.

User-facing statuses cover missing key, cancelled prompt, expired/already-completed request,
already-connected device, and server unavailability without HTTP details. Existing approval owner
derivation, CSRF/cookie, RP/origin, trusted Caddy proxy, and native-token boundaries are unchanged.
Local checks passed: Rust format, strict all-target/all-feature Clippy, `cargo test` (99 regular
tests; four disposable PostgreSQL tests remain ignored without `TEST_DATABASE_URL`), web source
regressions, typecheck/lint and production build. A clean browser automation session confirmed no
UUID/code field on the landing page, then used a local deterministic API harness (no account,
credential or staging request) to verify both the anonymous target-device screen and the signed-in
named account confirmation. Staging deploy of `efc405a` completed through the standard detached
worker with readiness passed; a fresh read-only browser session shows the new anonymous landing
without a UUID or general pairing-code field.

## RM-011-F P0 blocker fix — username-less discoverable passkey login (deployed, 2026-08-27)

The browser login flow now starts WebAuthn authentication without an account UUID and sends an
empty `allowCredentials` list, so the browser/OS can offer a saved discoverable passkey for the
fixed `alex.vault57.ru` RP ID. The server requires the assertion's `userHandle`, decodes it as the
16-byte account UUID used during registration, checks any legacy challenge owner binding, and
loads the credential only within that derived owner. The client-supplied `user_id` field is no
longer accepted by authentication verify, and no account identifier is rendered by the UI.

The existing registration, Secure/HttpOnly/SameSite browser cookie, CSRF, trusted-Caddy proxy,
WebAuthn origin/RP checks, rate limits, audit/device policy, and pairing completion contract are
unchanged. Native tokens remain outside the browser flow. Missing/invalid handles, unknown or
cross-account credentials, and invalid assertions receive the same neutral rejection; the UI
reports a missing-or-cancelled passkey, user cancellation where the browser distinguishes it, or
a generic server error without exposing internal details.

Local verification passed: `cargo fmt --check`, strict all-target/all-feature Clippy, `cargo test`
(97 regular tests; four disposable-PostgreSQL tests remain ignored), and web `typecheck`, `lint`,
and production `build`. The standard deploy completed successfully for commit
`7b4f306635e95297ac3cd8b2d99063500d90bc0f`; the deploy worker reported image
`sha256:531f1d5051260a3e008f36c96c2dfd3ce84bcc8158d8ccc5c7d09fa7f7f63de8`, two Yandex keys,
and `readiness=passed`. A read-only staging browser inspection shows no UUID field and the
discoverable-login buttons; no staging account was created and no passkey assertion was made.
The remaining acceptance item is a manual physical-device passkey smoke test.

## RM-011-F — staging security acceptance (in progress, 2026-08-27)

Local acceptance has verified the RockServer, RockCast, and RockMobile security boundaries and
their relevant test/build checks. During review, the QR approval URI was found to be protected by
the weaker Caddy `Referrer-Policy: same-origin` header. Both Caddy configurations now use
`no-referrer`, preventing the short-lived approval secret in the QR URI from being sent as a
same-origin request referrer; `tests/deploy_security.rs` locks this down and both configurations
validate in Caddy 2.10 with placeholder-only environment values.

The approved staging deploy script refuses to build from this intentionally uncommitted fix, so
deployment of the secure configuration is pending an approved immutable commit. Safe staging
checks on 2026-08-27 returned HTTPS readiness `200` and refused direct TCP port `3000`, but `/`
and the two negative RM-011 route probes returned `404`; the live site therefore predates the
RM-011 auth router and could not be accepted as this release before the corrected immutable
commit was deployed.

The first immutable RM-011-F deploy attempt uploaded its verified server image and Caddyfile but
stopped before backup/migration because the protected VPS `release.env` was missing the required
Caddy-to-server proof variable. Bootstrap now creates that root-only random value just as it
already does for the database password and legacy API credential; it is neither printed nor copied
through the desktop environment. A second bootstrap is required to install that operator-script
fix and provision the value before retrying deploy.

The next deploy diagnosis found that the existing VPS Caddy container was the upstream base image,
while the script neither transferred the built web bundle nor supplied the required immutable
`ROCKSERVER_CADDY_IMAGE`. The rollout now builds, labels, checksums, transfers, and verifies a
Caddy image tagged with the same source commit as RockServer. Bootstrap must install this updated
root-side deploy script before the final retry.

The first clean Caddy image build also exposed an unfinished pnpm allowlist value for `esbuild`.
It is now explicitly `true`, allowing only that already locked build dependency in the container;
the lockfile and dependency set are unchanged.

The Caddy Docker build also now sets `CI=true` only for its pnpm build command, so its clean,
non-interactive image build cannot wait for a modules-directory prompt after source copy.

Post-deploy probing found that Caddy's SPA `try_files` rewrite was taking precedence over the
API matcher, returning the static page for `/health/*` and `/v1/*`. Both Caddyfiles now use an
exclusive API `handle` before the static fallback; the regression test requires that order.

Final staging deployment `f960e323a2b9e06e0281bf144bb368dc244e54c9` succeeded through its
catalog and readiness gates. HTTPS `/health/ready` returns JSON `200`; the public Caddy response
sets CSP, `Referrer-Policy: no-referrer`, `nosniff`, `DENY`, and Permissions-Policy headers; and
direct public TCP port `3000` is unreachable. Browser passkey options without first-party Origin
return `403`; malformed native completion returns `422`. One disposable staging pairing request
returned `201` with the expected in-memory-only proof shape, while completion carrying a forged
`user_id` was rejected with `422`. RM-011-F is now blocked only on physical phone passkey/Keystore
evidence; no secret, pairing proof, code, URI, ID, or protected log was printed.

## RM-011-B2 — browser/pairing persistence implementation (2026-08-26)

Владелец разблокировал проектирование и реализацию `RM-011-B2`, приняв рекомендуемые значения:
WebAuthn RP ID и first-party origin — `alex.vault57.ru`; синхронизированные passkey разрешены;
автоматического восстановления после потери всех passkey нет; максимум 10 устройств на аккаунт;
audit retention — 90 дней. Также приняты рекомендованные сроки access/refresh/browser/pairing
сессий, лимиты короткого кода, несколько passkey на аккаунт и clone/sign-count policy.
Единственный trusted proxy — Caddy; прямые подключения fail-closed; состояние rate limits — в
PostgreSQL.

Реализованы migration `0013`, browser sessions, pairing requests, одноразовые WebAuthn challenges,
атомарные PostgreSQL rate-limit buckets, passkey sign-count advancement и транзакционное
approve/complete pairing с лимитом 10 устройств. Добавлены нейтральные WebAuthn context/sign-count
проверки и детерминированные unit/integration tests. Полный cryptographic WebAuthn verifier и HTTP
routes остаются частью RM-011-C; B2 не меняет публичный runtime API.

Проверка: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
`cargo test` (88 unit/library tests и обычные integration suites) и отдельный disposable
PostgreSQL B2 integration test прошли. Исходный compose PostgreSQL был остановлен после проверки;
его persistent volume не изменялся.

## RM-011-B — approved persistence subset (2026-08-26)

На момент выполнения B1 были реализованы только persistence boundaries, не выбирающие policy: a
passkey-only user schema, reserved but unused account-identity boundary, owner-scoped devices,
hashed native access/refresh-token records, atomic refresh rotation/replay family revocation, and
safe-classification audit events. Raw credentials, passwords, email/phone identities,
cryptographic WebAuthn verification, HTTP routes и client UI оставались вне B1; browser/pairing
и rate-limit persistence добавлены последующей B2. Migration `0012_add_account_session_persistence.sql` contains no plaintext
secret columns; the Rust store accepts only opaque fixed-size secret hashes.

Deterministic unit coverage verifies hash-debug redaction and the audit vocabulary. An opt-in
PostgreSQL integration test covers duplicate users, ownership isolation, refresh rotate/replay,
and account deletion revocation when `TEST_DATABASE_URL` names a disposable database. Its live run
remains pending that explicitly provided database; B2 has its own disposable PostgreSQL verification.

## RM-011-A — account/device contract and threat model (2026-08-26)

Approval-only design is recorded in `docs/rm-011-a-auth-device-contract.md` with the separate
`docs/rm-011-a-openapi.proposed.yaml`. It defines passkey/WebAuthn mobile-browser approval of a
desktop QR/short-code pairing request, opaque short-lived access bearer and rotating refresh-token
families, account deletion, and owner-scoped device list/revoke. Email, password, SMS and required
RockMobile installation are deliberately absent.
MVP-001 anonymous `/v1` endpoints remain separate: no shared token, credential or changed behavior
is proposed for them. The current runtime `api/openapi.yaml`, Rust code, database migrations,
clients, deployment, secrets and external services were not changed.

The entry records owner confirmation of MVP-001 and RM-007-D as supplied task context, and records
that OPS-001 staging exists; it does not overwrite the historical RM-007-D technical evidence.
RM-011-A contract/threat model was **approved by the owner on 2026-08-26**. The owner subsequently
selected the RM-011-B2 values recorded above. Trusted-proxy CIDRs, deployment wiring and the live
PostgreSQL test environment remain operational prerequisites and must be configured fail-closed;
RM-011-C must wait for B2 review and runtime OpenAPI reconciliation.

The proposal separately records phone-first browser registration: a passkey may create an account
before any desktop/device exists. It reserves an additive `AccountIdentity` boundary so a later
explicit login page or optional verified email/phone/password method can attach to the same user
without changing user/device/session ownership; none of those methods is currently proposed.

## MVP-001-A — public API contract and abuse model (2026-08-26)

Added approval-only `docs/mvp-001-a-public-api-contract.md` and a separate proposed OpenAPI
contract. They specify the exact five-endpoint anonymous `/v1` allowlist for bounded catalog,
search and voice flows; account, device, sync, admin, operations, health and legacy routes remain
protected or infrastructure-only. The proposal explicitly bans shared Bearer tokens and client
secrets for public users, and fixes draft rate/audio/session/concurrency limits, error semantics,
aggregate metrics and redacted logging. Existing `api/openapi.yaml`, router/runtime behavior,
RockCast and RockMobile were not changed; current protected routes remain protected until the
separately reviewed MVP-001-B implementation.

Владелец одобрил proposal 2026-08-26: в MVP входят paged public catalog, text и streaming voice;
draft quotas являются стартовыми и требуют отдельного review для увеличения. До публичного rollout
proxy CIDRs, rate-limit store и retention keyed-IP hashes должны быть заданы в deployment
configuration; в implementation до этого действует fail-closed direct-peer policy. Existing
`/api/v1` aliases остаются protected. MVP-001-B разрешена к реализации в утверждённых границах.
Local checks are recorded in `docs/tasks.md`.

## Station icons — шаг 0: контракт (2026-08-26)

Документационный шаг 0 завершён в `docs/roadmap/station-icons.md`. В нём явно как
**планируемое**, а не текущее поведение, зафиксированы RockServer-first URL
`GET /api/v1/stations/{id}/icon`, nullable `faviconUrl`, внутренний `source_url`, приоритет
источников, формат/лимиты, `200`/`304`/`404`, cache policy и rollout/rollback. Проверка текущего
`api/openapi.yaml` подтвердила, что endpoint и поле ещё не входят в действующий HTTP-контракт.
Маршруты, Rust-код, SQL migrations, OpenAPI и service diagram не менялись; текущий сервис не
получил выдачу или загрузку station icons.
Проверки документации и `git diff --check` (включая no-index check нового roadmap-файла) прошли.

## OPS-001-D automated VPS bootstrap and staging rollout (2026-08-25)

`deploy/ops-001-d.ps1` now provides a registry-free owner-facing bootstrap and staging launcher.
The ignored inventory contains only SSH user/host/domain; no password field exists. First access is
an explicit OpenSSH password prompt used only to install an ignored generated key, and bootstrap
allocates a TTY for a possible one-time interactive `sudo` prompt. It installs a command-scoped
non-interactive deploy rule; normal updates use `sudo -n` and fail with a repair instruction when
that privilege is unavailable.

One subsequent `-Action deploy` command requires a clean worktree, resolves current `HEAD`, builds
`rockserver:sha-<full SHA>` locally, verifies its OCI revision label and immutable image ID, transfers
a checksummed `docker save` artifact, and revalidates ID/label after remote `docker load`. Staging
does not require GitHub, GHCR, another registry, remote Git pull, or mutable `latest`. Protected
runtime env updates are newline-correct and preserve generated DB/API secrets; only the four
documented Yandex names can transfer from ignored root `.env`. Backup precedes the importer, whose
embedded migrations and checksum-pinned idempotent full-catalog activation precede HTTPS readiness.
ONNX staging assets are fetched automatically from the committed checksum-pinned lock; focused local tests
and dry-run, production Compose rendering, PowerShell parsing, Rust fmt/strict Clippy/full tests,
and diff checks passed. The final all-features Docker build downloaded dependencies and reached Rust
compilation but was stopped at the owner's request before completion; it is not recorded as passed.
No VPS, DNS, registry, credential, ONNX download, or public readiness operation was run.

## RM-007 local personal-data contract

### RM-007-A common model and station-ID migration (2026-08-25)

The proposed cross-client contract is recorded in
[`rm-007-a-local-personal-data-contract.md`](rm-007-a-local-personal-data-contract.md). It defines
versioned local `LocalProfile`, `Favourite`, and `PlaybackHistoryEntry` shapes; local-only privacy,
dedupe/order/retention, safe rollback, and RM-004-compatible identity lifecycle rules. It specifies
automatic resolution only for verified canonical IDs, reviewed legacy mappings, and `merged`
tombstones; `split`, removed, and unknown references remain local unresolved records. The document
does not claim existing favourites/history: verified sources show only RockMobile unavailable-voice
ID migration and RockCast's catalog/legacy adapters. No code, database, API/OpenAPI, catalog, sync,
or authentication behavior changed. Human approval remains required for the explicitly listed
ordering/limits, split UX, RockCast legacy mapping, lifecycle artifact, and future-sync decisions.

## RM-004 shared catalog

### RM-004-I cutover and legacy cleanup (2026-08-25)

RM-004-I designates RockCatalog `2026.08.2` as the approved immutable curated baseline:
SHA-256 `3fa20dca94fc059bd433a47b9fba9bb6d5e5e1aa2957a5ffb58b2a7b20b1d74d`. RockServer, RockCast,
and RockMobile pin the same JSON release; RockMobile additionally pins the separate extended
SQLite `2026.08.2-mobile.1` (schema 1, 16,825 stations, SHA-256
`ad469d405f177d7e476cf9b3d9985497d0e2c6132ac0f3ce14485f4eab402073`). RockCatalog is the only
baseline authoring source. `release_sync.py` is the only supported consumer-update route; ordinary
builds/startup neither copy nor fetch catalog data. Rollback selects retained immutable artifacts
through the same offline sync/verify workflow.

RockCast's TXT adapter remains the sole cutover legacy exception: owner RockCast maintainers,
reason existing offline user overrides, removal date 2026-10-31. RockMobile has no TXT path. The
service diagram reflects the verified authoring/release/baseline/extended flow. Offline release
tool tests (12/12), `release_sync.py verify` for all consumers, RockServer fmt/Clippy/full tests,
RockCast fmt/Clippy/full tests, and a loopback RockServer readiness smoke passed on 2026-08-25.
RockMobile Gradle checks remain unrun because this environment cannot read the configured Android
API-36 SDK package metadata and its SDK licenses are unaccepted; no SDK download or license change
was performed. Device, stream-playback, live provider, and live PostgreSQL checks remain outside
this offline cutover verification. The RM-004-I acceptance gate is therefore not fully passed:
Android offline-client execution remains an environment handoff item.

### RM-004-H High-issue remediation (2026-08-25)

RockServer now preserves canonical `tombstones` through preflight, activation, PostgreSQL
persistence, and internal lifecycle lookup. `removed` records have no successor, `merged` records
may resolve automatically to their sole replacement, and `split` records remain explicitly
ambiguous so no caller can silently move user state. Migration `0011_add_catalog_tombstones.sql`
stores the active `rockcatalog` lifecycle view separately from soft-retired station rows. Shared
catalog activation replaces that view, provider-scoped retires missing RockCatalog records, and
commits both changes with the import run; reimporting a release is idempotent and reactivating a
previous release removes its newer lifecycle view. Radio Browser rows remain outside these writes.
The existing public HTTP/OpenAPI response remains unchanged.

Pinned catalog failure is now fail-closed. The production in-memory path returns a preflight error
for missing, malformed, checksum-invalid, schema-invalid, or semantically invalid pins; startup
does not select the old six-station fixture. PostgreSQL preflights the immutable catalog before
activation, so such a failure leaves the prior successfully activated database snapshot untouched
and causes startup/readiness to fail when no active snapshot exists. The compact fixture is compiled
only for isolated Rust unit tests and has no production selection path. This behavior is offline and
does not fetch a replacement catalog.

RM-004-G is complete at its release-and-synchronization acceptance gate. The local immutable
baseline release layout is `C:\repos\rockcast-station-catalog\release\2026.08.2`, containing
canonical JSON, schema, SHA-256 manifest, release metadata, and changelog. The explicit offline
sync/verify tool now updates RockServer, RockMobile, and RockCast only after checksum and
schema/version validation; ordinary builds and startup remain download-free. All three consumer
snapshots verified against baseline `2026.08.2` SHA-256
`3fa20dca94fc059bd433a47b9fba9bb6d5e5e1aa2957a5ffb58b2a7b20b1d74d`. RockMobile additionally
validated its independent SQLite package `2026.08.2-mobile.1`, schema 1, count 16,825, SHA-256
`ad469d405f177d7e476cf9b3d9985497d0e2c6132ac0f3ce14485f4eab402073`. RM-004-H/I have not started.

RM-004-B and RM-004-C have passed their acceptance gates in the separate local repository
`C:\repos\rockcast-station-catalog`. The approved 41-station release candidate is version
`2026.08.2`, with SHA-256 `3fa20dca94fc059bd433a47b9fba9bb6d5e5e1aa2957a5ffb58b2a7b20b1d74d`.
RockServer now vendors that immutable artifact and schema, validates it before use, selects it as
the in-memory fallback, and activates it transactionally under provider source `rockcatalog` when
the PostgreSQL repository initializes. Records use canonical station IDs and `<station-id>:<stream-id>`
stream identities; missing baseline rows are provider-scoped soft-retired. Public HTTP/OpenAPI
result fields and single selected primary stream semantics are unchanged. RM-004-E onward remain
out of scope. RM-004-D also includes a deterministic PostgreSQL-to-SQLite exporter for a future
RockMobile/Room bundle, documented in `docs/rockmobile-extended-catalog.md`. Its release gate
requires at least 16,000 active/playable stations with exactly one active primary HTTP(S) stream.
The verified local `rockserver` database contained 16,825 eligible stations on 2026-08-21 and
produced the complete `2026.08.2-mobile.1` release SQLite, manifest, and eligibility report under
`release/mobile-catalog/`. Its exact-file SHA-256 is
`ad469d405f177d7e476cf9b3d9985497d0e2c6132ac0f3ce14485f4eab402073`. The report records
provider-safe dedupe, primary selection, and server-only exclusions.

Verification on 2026-08-21 passed `cargo fmt --check`, strict all-target/all-feature Clippy, the
regular `cargo test` suite, the opt-in PostgreSQL integration test against a disposable local
PostgreSQL/pgvector container, and the catalog repository's offline validator, formatting check,
and nine-tool-test suite. The mobile SQLite unit fixture additionally passed `PRAGMA integrity_check`,
schema/version, metadata/count, FTS count, search-index, and exact-byte SHA-256 checks.

## Architecture refactor review

The package layout was reviewed and the first safe refactor was completed. Catalog import code now
lives under `src/catalog/`, and voice-command plus streaming speech boundaries live under
`src/voice/`. Compatibility facades preserve the former `rockserver::speech` and
`rockserver::voice_command` paths for existing clients and tests. HTTP and search remain the next
large seams for incremental extraction: `src/http/mod.rs` still owns several transport concerns,
and `src/search/mod.rs` still combines domain models, the in-memory repository, and orchestration.

Verification for this refactor: `cargo fmt --check`, strict all-target/all-feature Clippy, and
`cargo test` passed. Tests used an isolated `target-codex` directory because an existing local
RockServer process held the normal debug executable open. No public route or OpenAPI behavior was
changed.

## Local ONNX semantic search

`onnx-local` provides CPU-only `intfloat/multilingual-e5-small` inference (384 dimensions) using local ONNX Runtime and tokenizer assets. Migration 0005 persists normalized station text and adds an E5-provenance cosine HNSW index. On 2026-08-17 the local PostgreSQL database contained 1,005 stations (999 imported from Radio Browser), all 1,005 searchable documents were backfilled with matching E5 embeddings, and live Russian queries completed through `POST /v1/search`.

## Semantic language and country intent filters

When the local E5 semantic embedding provider is configured, startup now prepares 28 compact,
multilingual E5 language-label vectors. The query vector is reused for both the language decision
and station ranking, so a request performs no duplicate query inference. A language becomes a hard
filter only when its cosine score is at least 0.72 and exceeds the next candidate by at least 0.04;
otherwise the search remains broad. `ROCKSERVER_SEMANTIC_LANGUAGE_FILTERS=off` disables only this
hard-filter layer while preserving semantic station ranking.

The parser distinguishes language wording from an explicit country request. For example,
`Включи английский рок` can yield `language=en`, while `Включи рок из Англии` yields
`country_code=GB`. The deterministic parser recognizes `англии`, and a validated LLM country is
preserved only after an explicit `из` or `from` country phrase. This prevents the UI/STT locale or
a cultural adjective from becoming a country hard filter. The E5 label set currently covers Arabic,
Chinese, Czech, Danish, Dutch, English, Finnish, French, German, Greek, Hebrew, Hindi, Hungarian,
Indonesian, Italian, Japanese, Korean, Norwegian, Polish, Portuguese, Romanian, Russian, Spanish,
Swedish, Thai, Turkish, Ukrainian, and Vietnamese; additional labels require a deliberate quality
evaluation before they are added.

Latest verification for this change: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test` passed. The regular suite ran 67 library tests plus HTTP/contract tests; database and credential-dependent live tests remained explicitly ignored.

## Current state

The first API-access security slice is implemented. Startup requires the configured application
Bearer credential; `POST /v1/search`, both voice-command
routes, and both WebSocket voice-stream handshakes reject malformed or invalid
`Authorization: Bearer` credentials with structured `401 authentication_required` responses.
`/health/live` and `/health/ready` remain unauthenticated for process supervision. The initial
token is deployment configuration only, compared without prefix-based early exit, and is not
logged. Durable, per-RockCast client credentials, administrator sessions, and the HTMX console
remain the next deliveries described in `docs/admin-security-plan.md`.
`GET /admin` now provides a local, read-only administration-console preview: a browser operator
enters the existing application Bearer token, which stays only in the current tab's memory, then
can inspect readiness and search catalog stations. It is not an administrator-account, session,
audit, or catalog-management implementation.
`run-rockserver-local.ps1` loads `ROCKSERVER_API_BEARER_TOKEN` and `DATABASE_URL` from the ignored
local `.env`, rejects missing/short credentials, and never prints the token. Its startup output
reports the configured bind address and derives localhost plus active LAN admin-preview URLs from
the actual port and network interfaces instead of printing a fixed `127.0.0.1` address.
`docs/service-diagrams.html` now distinguishes this deployed single deployment credential from
the planned persisted RockCast clients and Bearer-based administrator sessions, and records the
current RockCast Bearer voice-handshake integration.

The active product direction is Windows-first: RockCast text search, Windows microphone capture, the RockServer voice/STT path, and production hardening will be completed and validated before any ESP32 work. The ordered plan is recorded in `docs/windows-production-roadmap.md`. ESP32 is future backlog, not a current delivery stage.

RS-007 was re-verified on 2026-08-14 against the disposable local `rockserver` database on PostgreSQL 18.1 with pgvector 0.8.6. A clean `public` schema accepted migrations 1–4, the ignored real integration test passed, and two deterministic development backfills produced seven unique 32-dimensional station embeddings with no duplicate provenance keys. A live HTTP smoke check reported both health endpoints ready and returned hybrid-ranked `jazz` results. Credentials remain environment-only.

Stages 0–10 are complete in the current working tree: repository bootstrap, Axum HTTP skeleton, OpenAPI search contract, deterministic in-memory search, PostgreSQL persistence, controlled Radio Browser import, RS-007 semantic ranking foundation, RS-008 voice-command JSON contract, the provider-neutral streaming voice API, and Yandex AI Studio intent parsing.

`POST /v1/search` keeps the existing request and response schemas. `SearchService` now owns request-only query interpretation and optional query embedding before calling `StationRepository`. The in-memory repository remains metadata-only. PostgreSQL uses exact pgvector cosine similarity when the query and station embedding provenance match, and otherwise preserves metadata fallback.

RS-008 stabilizes the Windows voice-command transport contract without introducing audio upload or an STT provider. The canonical route is `POST /api/v1/voice/command`; `POST /v1/voice/command` is a deprecated compatibility alias with identical behavior. Both accept only an already-recognized `transcript` plus the established locale, limit, and station-exclusion controls, then delegate to the existing `SearchService`. A successful response returns the trimmed transcript, normalized query, full deterministic result list, and `selected_station` equal to the first result or `null` when there is no match. Existing `POST /v1/search` remains unchanged.

The JSON voice route retains its existing limits and error behavior. Canonical WebSocket `GET /api/v1/voice/stream` (deprecated alias `/v1/voice/stream`) now accepts a validated start event and bounded PCM16 mono binary chunks, emits partial/final transcript events, and resolves the final transcript through `SearchService`. Chunks are limited to 65,536 bytes, sessions to 10 MiB, provider operations to ten seconds, and search to five seconds. Terminal WebSocket errors retain `code`, `message`, `request_id`, and `details`.

The `StreamingSpeechRecognizer`/`SpeechStreamSession` traits keep recognizers replaceable without exposing credentials or upstream protocol details to clients. With `YANDEX_AI_API_KEY`, startup exposes both `YandexSpeechKitRecognizer` (buffered v1) and `YandexSpeechKitStreamingRecognizer` (v3 gRPC); otherwise both modes use `UnavailableSpeechRecognizer`. A client selects `recognizer_mode` in its start event: omitted or `buffered_v1` preserves commit-time recognition, while `streaming_v3` forwards chunks upstream and emits partial/final updates.

An ignored `yandex_speechkit_live` integration test covers a real, pre-recorded mono Ogg/Opus command against SpeechKit's synchronous endpoint. Its committed fixture and expected phrase are generated explicitly by `generate_speechkit_fixture` through Yandex TTS; both network operations are billable and only run when selected. The recognition test is gated by explicit credentials, while optional variables can override the committed audio and phrase. Service-account API-key requests intentionally omit `folderId`, because SpeechKit derives the folder from the service account. The generator's opt-in `YANDEX_SPEECHKIT_DEBUG=1` mode prints non-secret request metadata, response status and headers, plus a 16 KiB bounded redacted error body; it also removes key-like fragments echoed by the provider. For explicit mismatch diagnosis, `TEST_YANDEX_STT_DEBUG=1` logs the non-secret STT request metadata and actual/expected transcript; its authorization is always redacted. It does not make the streaming `SpeechStreamSession` production-ready. After correcting local authentication, the 2026-08-14 live TTS generation and STT recognition succeeded: the committed 18,341-byte Ogg fixture produced HTTP 200 and matched the expected transcript. Earlier 401 attempts were retained only as safe diagnostic history.

`diagnose_speechkit_pcm` is an opt-in local diagnostic binary. It synthesizes a short Russian command as 48 kHz PCM16 and sends that same in-memory audio directly to SpeechKit v1, printing only text, byte count, duration, timings, and transcript. It writes no audio and helps separate provider behaviour from RockCast microphone capture and WebSocket transport.

The query parser, LLM provider, and embedding provider are traits. `LlmQueryParser` turns a bounded provider JSON response into the existing `QueryIntent`; it receives no catalog. The deterministic metadata parser is the default and the parser failure fallback. `YandexLlmProvider` is enabled only when both `YANDEX_AI_API_KEY` and `YANDEX_FOLDER_ID` are present; absent configuration preserves deterministic startup, while partial configuration fails safely without revealing values. It uses the documented synchronous AI Studio completion API, `Api-Key` authorization, `gpt://<folder>/yandexgpt/latest` by default, JSON Schema output, a 3-second timeout, and response/token bounds. Malformed/oversized/non-2xx/timeout responses and invalid hard filters all degrade through the existing deterministic path. Optional local `.env` loading is provided by `dotenvy`; `.env` and `.env.*` stay ignored. This is intent parsing for text and already-recognized voice transcripts, not SpeechKit STT. Ordinary tests use loopback mocks and never contact Yandex or another external provider.

RS-015 adds a provider-neutral `CommandInterpreter` above the same replaceable `LlmProvider`. `LlmCommandInterpreter` sends bounded STT text and locale with a strict Yandex-compatible JSON Schema, deserializes directly into serde-backed `VoiceCommand`, `Intent`, and `RadioQuery`, and then performs only semantic validation and normalization. It supports radio play/search, stop, next/previous station, relative volume change, and unknown intent without an heuristic command parser. The separate ignored `yandex_llm_live` target has four exact tests; its primary calm-jazz case passed live on 2026-08-14 and safely logged the POST endpoint, redacted authorization/folder, request body, HTTP 200 response body, and final typed command.

Station embedding generation is a separate `backfill_embeddings` command. It is never called by HTTP startup or `POST /v1/search`. Radio Browser import ownership and update semantics remain unchanged.

## Configuration and behavior

- HTTP listener: `ROCKSERVER_BIND_ADDR`, default `0.0.0.0:3000`.
- Verification on 2026-08-20: `cargo fmt --check`, strict all-target/all-feature Clippy, and `cargo test` passed; `graphify update .` refreshed the local code graph.
- Application access gate: clients send `Authorization: Bearer <token>`; the process requires
  `ROCKSERVER_API_BEARER_TOKEN` and rejects missing/blank values at startup. Persisted users and
  revocable client tokens remain future work.
- Logging filter: `RUST_LOG`, default `info` when unset or invalid.
- HTTP catalog backend: `DATABASE_URL` is required and selects PostgreSQL; the in-memory repository
  is limited to deterministic tests and is not selected by the service process.
- Optional query embeddings: `ROCKSERVER_SEMANTIC_PROVIDER=deterministic-dev` for tests/development or `onnx-e5-local` with the `onnx-local` Cargo feature and local model/tokenizer/runtime paths; absence means metadata-only search.
- Optional LLM parser: `YANDEX_AI_API_KEY` and `YANDEX_FOLDER_ID` together enable Yandex AI Studio; absence of both selects deterministic parsing, and a partial configuration is a safe startup error.
- Yandex parser tuning: `YANDEX_LLM_MODEL=yandexgpt` by default and `YANDEX_LLM_TIMEOUT_MS=3000`, bounded to 100–10,000 ms.
- Development embedding dimension: `ROCKSERVER_EMBEDDING_DIMENSION`, default 32, valid range 1–16,000.
- Development embedding provenance: model `rockserver-deterministic-dev`, version `1`, plus configured dimension.
- Embedding command: `cargo run --features onnx-local --bin backfill_embeddings`; it requires `DATABASE_URL`, explicit provider selection, and local assets for the E5 provider.
- Radio Browser import command and configuration remain as documented in `README.md`; no import provider is called by search.
- Local database: `compose.yaml` uses `pgvector/pgvector:pg17` with development-only credentials and a PostgreSQL healthcheck.
- Migrations: embedded versioned files run for PostgreSQL search, import, and embedding store connections.
- Public HTTP/OpenAPI schemas: unchanged by RS-007; the description now records optional semantic ranking and metadata fallback.

## Query, ranking, and failure semantics

`QueryParser` receives only validated query text and locale. It returns terms, tags, language, and country intent and has no catalog parameter. Provider intent is normalized and hard filters are validated before repository use. A parser error or invalid intent uses the deterministic metadata parser.

`EmbeddingProvider` receives one query string during search or one station document during the controlled backfill. It never receives the full catalog. Invalid embeddings are rejected before persistence/search: model and version must be non-empty, dimension must be 1–16,000 and equal to the vector length, values must be finite, and the vector must be non-zero. Embedding provider failure omits semantic input and continues with metadata search.

PostgreSQL applies language/country hard filters and station exclusions in the candidate CTE, before score ordering and final limit. Metadata score retains the RS-003/RS-004 exact-token formula. Compatible cosine similarity is normalized and clamped with `1 - cosine_distance / 2`. Final hybrid score is `0.70 * metadata_score + 0.30 * semantic_score`. A station without compatible model/version/dimension provenance retains its full metadata score. Results order by final score descending and station ID ascending; station ID is the last tie-break.

Liveness remains process-only. Readiness calls only the selected repository, so optional parser/embedding failure does not report false unready. PostgreSQL loss still returns 503/`not_ready`; in-memory readiness remains 200/`ok`.

## Persistence and backfill

Migration `0004_add_station_embeddings.sql` enables pgvector and adds `station_embeddings` with station ownership, model, version, dimension, unbounded `vector`, and created/updated timestamps. CHECK constraints enforce declared dimension and non-zero norm. The table key is `(station_id, model, version)`; changing dimension for the same development model/version replaces that station embedding.

The unbounded vector column remains dimension-neutral for tests and future providers. Exact search filters compatible provenance before the cosine operator; migration 0005 adds a partial 384-dimensional HNSW cosine index only for `intfloat/multilingual-e5-small:onnx-v1`.

`backfill_embeddings` reads station documents in stable station-ID pages and upserts one embedding at a time. Repeating the workflow is idempotent for the same provenance and updates existing rows. It does not change Radio Browser ownership, delete catalog rows, probe streams, or make network requests.

## Known limitations

- `deterministic-dev` remains a repeatable hash fixture; production-like semantic behavior requires explicitly provisioned local E5/ONNX assets.
- Yandex AI Studio is the only production LLM query parser; it has bounded per-request fallback but no retry policy or circuit breaker.
- The deterministic parser currently derives a station-language hard filter from the request locale. Live checks therefore used `en-US` to inspect cross-language E5 ranking over the broader English catalog; `ru-RU` intentionally restricts candidates to Russian-language stations.
- The backfill currently visits all stations and upserts every embedding; it does not yet skip unchanged station/model inputs or resume a failed run from durable workflow state.
- A model/version is expected to imply one dimension, but RS-007 records/enforces compatibility per row rather than introducing a separate model registry.
- Existing Radio Browser pagination, stale-run, language-code, and upstream-health limitations from RS-006 remain.
- RockCast calls the protected canonical voice WebSocket route, records from the default microphone, maps server results into playback, and distinguishes missing/invalid token and unavailable-server outcomes. Input-device selection/testing, deterministic end-to-end coverage, and clearer upload/recognition/cancellation states remain.
- Yandex SpeechKit supports selectable buffered v1 and streaming v3 modes when its API key is present. Provider retries/circuit breaking, a second provider, and live end-to-end streaming coverage remain.
- Per-client credential persistence/revocation, administrator authentication, login throttling,
  request audit storage, admin console state-changing operations, rate limiting, metrics, scheduler,
  stream probing, and deployment hardening remain future work.

## Verification

The regular suite passes with 67 deterministic unit, HTTP, contract, and WebSocket tests; the real PostgreSQL, SpeechKit, and four Yandex LLM integration cases remain opt-in and ignored by default. Coverage includes typed command serde/schema/semantic validation, loopback Yandex LLM mocks for header/model/schema, non-2xx, timeout, malformed/oversized response, and secret-safe errors; a real loopback WebSocket upgrade with fake recognition; application Bearer rejection while health stays public; the unauthenticated admin-preview shell; and existing query, provider-fallback, embedding, ranking, and persistence boundaries.

Docker Engine 29.7.2 and Compose 5.3.1 ran isolated project `rockserver-rs007-test` on host port 55438 with `pgvector/pgvector:pg17`. The real integration test passed for extension/migrations, embedding insert and repeat update, provenance/dimension storage, exact cosine similarity, hard filters, exclusions, final limit, semantic tie-break, metadata fallback, HTTP search, and repository-only readiness. The real deterministic backfill command also completed twice; inspection showed seven unique development rows for `rockserver-deterministic-dev:1:8`. Only that project's container, network, and volume were removed afterward.

For RS-015, `cargo fmt --check`, strict all-target/all-feature Clippy, and `cargo test` passed. The exact ignored live command test also passed against Yandex with a typed calm-jazz result. `graphify update .` then rebuilt the local graph to 774 nodes, 1,692 edges, and 37 communities; it reported the pre-existing zero-node `hooks.json` warning and stale community labels.

For RS-016, `cargo fmt --check`, strict all-target/all-feature Clippy, and `cargo test --all-features` passed with 50 regular tests; six credential/asset/database-dependent tests remained ignored. The real local import produced 999 Radio Browser stations alongside six built-ins, and E5 backfill produced 1,005 `intfloat/multilingual-e5-small:onnx-v1:384` rows. Live search ranked `Midnight Jazz Lounge` first for `спокойный джаз` and `# RdMix Classic Rock 70s 80s 90s` first for `рок 80-х`. `классическая музыка без слов` returned weaker ambient/jazz results, documenting a current catalog/locale-filter quality limitation. `graphify update .` rebuilt 809 nodes, 1,774 edges, and 42 communities with the known zero-node `hooks.json` and stale-label warnings.

## Genre hierarchy and search fallback

RS-019 introduces a genre taxonomy hierarchy and progressive search fallback. `CANONICAL_TAGS` now includes subgenres present in the real Radio Browser catalog (`black metal`, `death metal`, `alternative rock`, `classic rock`, `pop rock`, `smooth jazz`, etc.). A `genre_parent` function maps each subgenre to its nearest parent (`black metal` → `metal` → `rock`, `smooth jazz` → `jazz`). `station_matches_requested_genre` accepts stations whose tags match any ancestor of the requested genre, so a `"heavy metal"` query matches stations tagged `"rock"`.

`SearchService::search` applies progressive fallback when the exact genre filter yields no results: first broadening to parent genres, then dropping the genre constraint entirely while keeping the `MIN_RELEVANCE_SCORE` gate. The LLM system prompt automatically includes the expanded tag vocabulary. A new builtin catalog station (`Iron Forge Radio`, tags `heavy metal`/`metal`, language `en`) validates the hierarchy. Deterministic tests confirm that `"english heavy"` finds English rock/metal stations, that subgenre queries match parent-tagged stations, and that unrelated genres remain rejected.

## Genre database and stream probing

RS-020 moves the genre hierarchy from compiled-in constants to a PostgreSQL `genre_hierarchy` table seeded with ~250 genres across 25 root categories. `GenreTaxonomy` loads from the database at startup with a builtin fallback for in-memory mode. A new `probe_streams` binary connects to each stream URL (8-second timeout, 50 concurrent), marking unreachable streams as `degraded` with the error stored in `last_probe_error`. The PostgreSQL search CTE now excludes `degraded` streams. The Radio Browser importer supports `RADIO_BROWSER_TAGS` for per-tag import passes, enabling broader genre coverage. Migrations 0006 and 0007 are applied automatically on connect.

## Station name search improvements (RS-021)

RS-021 improves station name search accuracy for voice queries such as "включи радио диджей" → `radioDJ`:

- **Stop-word filtering**: Command verbs (`включи`, `поставь`, `найди`, `play`, `find`, etc.) are removed from query terms before matching, preventing score dilution.
- **CamelCase splitting**: `tokenize()` now splits `"radioDJ"` into `["radio", "dj"]` and `"XMLParser"` into `["XML", "Parser"]`. This affects both query tokenization and `searchable_text()` at import time.
- **Transliteration expansion**: A word-level mapping (`радио`↔`radio`, `диджей`↔`dj`, `фм`↔`fm`, `рок`↔`rock`, ~50 entries) expands query terms so that Cyrillic queries match Latin station names and vice versa.
- **Substring matching**: Both in-memory ranking and the PostgreSQL search CTE now award partial credit (0.5 per term) for substring matches in station names (terms ≥ 3 chars), so "радио" finds stations with "radio" in their name even without exact token match.
- **Backfill script**: `fill-database.ps1` now includes `backfill_embeddings` as the final step after import and stream probing.
- **LLM intent normalization + fallback**: Provider `terms` are normalized into atomic tokens (`tokenize()`), expanded with transliteration aliases, and if the provider returns empty `terms` and empty `tags` the system switches to deterministic parsing for more reliable station-name matching.
- **Search performance**: Added a PostgreSQL `pg_trgm` + trigram index on `lower(stations.name)` to accelerate `LIKE '%term%'` prefiltering/substrings.

## Full-text search and similarity scoring (RS-023)

RS-023 replaces the expensive sequential `LIKE '%term%'` prefilter with three index-backed search layers:

1. **PostgreSQL FTS (tsvector)**: Migration 0009 adds a `searchable_tsv` tsvector column derived from `searchable_text` with a GIN index and auto-update trigger (config `simple` for language-neutral exact tokens). The prefilter uses `plainto_tsquery('simple', $14)` for O(log n) candidate selection.
2. **pg_trgm similarity**: The prefilter now uses the `%` operator (`lower(s.name) % qt.term`) instead of `LIKE '%term%'`, leveraging the existing GIN trigram index for fuzzy name matching. A `trgm_score` (max similarity) contributes 0.3 weight to the metadata score.
3. **Embedding cosine (unchanged)**: Semantic fallback via pgvector HNSW index remains as before.

`raw_query` (stop-words removed) is passed through `QueryIntent` → `SearchQuery` → `PostgresSearchParameters` → `$14` for FTS matching.

## Correct name ranking when LLM omits terms (RS-024)

RS-024 fixes incorrect ranking and extra latency when the LLM returns `tags` but omits `terms` (e.g. "включи радио рокс"). Previously this caused name-token matching to be skipped and ranking fell back to mostly genre-tag + semantic similarity.

Now in `SearchService::interpret_and_search`:

- if `intent.terms` is empty, we run `DeterministicQueryParser` and:
  - keep provider genre `tags` (when present),
  - but replace `terms`, `core_term_count`, and `raw_query` with deterministic tokenization output.
- the transliteration vocabulary was extended with common voice tokens:
  - `ультра`↔`ultra`, `рокс`↔`roks`, `викер`↔`viker`.

## Narrow prefilter candidate set before heavy scoring (RS-025)

RS-025 reduces slow broad queries by making the `prefiltered` CTE `MATERIALIZED`, assigning each row a cheap `prefilter_score` (exact tag hits + `ts_rank_cd` + max trigram similarity), and limiting the candidate pool to `GREATEST(limit * 20, 200)` before the expensive exact-token, substring, and embedding scoring phase.

## Indexed candidate-branch search (RS-030)

RS-030 addresses slow PostgreSQL station searches observed in request logs. The search SQL now obtains candidate IDs through three independent, index-backed branches (tag GIN, FTS GIN, and name trigram GIN), unions and de-duplicates those IDs, and only then calculates the unchanged prefilter and final ranking scores. Full-text term queries are generated once per search term, and each candidate station name is normalized and tokenized once before its exact-name, substring, and trigram scores reuse those values. This removes repeated parsing and regex tokenization for expanded/transliterated voice terms without changing matching semantics. Query interpretation, matching rules, limits, hard filters, exclusion handling, score formulae, and deterministic ordering are unchanged. Debug logs now record parser, embedding, repository, and database-search elapsed milliseconds plus result count. Live `POST /api/v1/voice/command` checks against the configured PostgreSQL database returned 30 stations in 1,387 ms for `Включи немецкий рок или хэви метал` and in 1,388 ms for the STT transcription `Включи немецкий рок или хейли метал`; the latter spent 1,097 ms in parsing and 233 ms in PostgreSQL, below the five-second voice timeout. A production `EXPLAIN (ANALYZE, BUFFERS)` comparison remains useful for broader capacity monitoring.

## Explicit world-country filters (RS-031)

RS-031 expands deterministic country recognition from three countries to the ISO 3166-1 alpha-2 country set. It recognizes common Russian and English country names plus demonyms, including `немецкий`/`Germany` -> `DE`. The parser still derives `country_code` only from an explicit word sequence in the original request, never from locale or an LLM guess. If a phrase is ambiguous (for example, `Конго` without a republic qualifier), it emits no hard country filter.

## Remove per-candidate regex tokenization from exact matching (RS-026)

RS-026 replaces the exact-term part of `matched_count` with `searchable_tsv @@ plainto_tsquery('simple', qt.term)` so PostgreSQL reuses the precomputed normalized document instead of re-running `regexp_split_to_table(...)` over station names and tags for every candidate row. The cheap prefilter ordering also now avoids early full-table trigram scoring and keeps trigram similarity only in the limited candidate phase.

## Fast empty result when prefilter has no candidates (RS-027)

RS-027 removes semantic-only fallback when metadata prefilter returns zero rows. For unknown or unsupported requests this makes search return empty quickly instead of spending extra time on embedding-neighbor fallback and returning low-value semantic matches.

## Generic bidirectional transliteration (RS-028)

RS-028 adds generic bidirectional transliteration for query terms (Russian Cyrillic ↔ Latin) as a standard step in `expand_transliterations`, instead of relying on point-by-point station-brand mappings. This improves cross-script station-name recall (for example, spoken Russian name forms matching Latin station names and vice versa) while keeping existing dictionary-based genre/radio-word normalization.

## Name-priority mode for "включи радио ..." (RS-029)

RS-029 introduces a station-name priority path for commands that explicitly start with `включи радио` / `поставь радио` (and English equivalents). The parser derives an ordered station-name hint phrase from the tail of the command, the query carries that hint through to PostgreSQL, and SQL ranking gives a strong bonus when the normalized station name contains the same words in the same order. This helps short exact station names outrank longer related variants.

## RM-004-A contract planning gate

RM-004-A is complete and approved. The repository
now contains a proposed shared-catalog contract, Draft 2020-12 JSON Schema, and matching example.
The design was based on a read-only inventory of RockServer, RockCast, and RockMobile and records the
41-row RockCast/RockMobile baseline, current URL-derived mobile IDs, provider ownership, multiple-
stream database behavior, public DTO compatibility, migration, release, rollback, and tombstone
rules. No runtime contract, code, database, client, build, CI, catalog data, or external repository
was changed. RM-004-B and later stages have not started.

Planning verification on 2026-08-21 parsed both JSON files successfully, confirmed that the schema's
embedded example matches the standalone example, checked proposal/schema terminology and required
fields, ran `git diff --check`, and confirmed the change scope is `docs/**`. Cargo and Gradle checks
were intentionally not run because this task changes documentation only.

## Next step

The next RM-004 work, if separately approved, is client integration (RM-004-E/F) and later release
automation. RockMobile can consume the verified `2026.08.2-mobile.1` package according to the
documented checksum procedure; future catalog versions must satisfy the same fixed 16,000-station
release gate. The previously recorded service follow-up remains to add deterministic RockCast-to-RockServer end-to-end coverage
for search and voice, then improve voice cancellation and state reporting across capture, upload,
recognition, search, and playback, followed by retention-safe logging, provider resilience, and a
second recognizer behind the existing trait.

## Shared internet-beta roadmap

Planning-only product roadmap added in `docs/shared-product-roadmap.md`. It places RM-007, RM-011,
and RM-012 ahead of unrelated roadmap work to prepare a hosted RockServer beta with users,
registration, device linking, synchronization, and remote control across RockMobile and RockCast.
The ESP32 client is intentionally deferred until hardware is available. The documented proposed
first deployment is a single VPS with Docker Compose, Caddy, private PostgreSQL, immutable images,
manual release approval, backup, health checks, and rollback. No runtime, API, database, client,
deployment, credential, or infrastructure change was made by this documentation task.

## Executable internet-beta plan

The shared roadmap is now decomposed into concrete, sequential Codex tasks in
`docs/shared-product-execution-plan.md`: GATE-001; RM-007-A through D; OPS-001-A through D;
RM-011-A through F; RM-012-A through F; then ESP-001 and ESP-002 when hardware exists. Every
task has repository boundaries, dependency, recommended model/reasoning effort, scope and
acceptance gate. The plan itself is not an approval to implement any task.

## RM-007-D cross-client review (historical evidence, 2026-08-25)

At the time of this evidence-based review, RM-007-D was **not passed**. The review in
`docs/rm-007-d-cross-client-review.md` confirmed the shared baseline release and the clients'
offline local-catalog paths, but found four unresolved High issues: RockMobile's persisted JSON is
not the portable RM-007-A profile shape; its read/init path can replace corrupt or unsupported data
with an empty profile and has no migration backup/rollback; its lifecycle resolver is not wired to
production catalog state; and RockCast records URL-derived `rockserver-*` IDs for remote/voice
plays instead of canonical server station IDs. Both clients also have incomplete favourite merge
metadata semantics.

Offline release verification passed for RockServer, RockCast and RockMobile (including the Mobile
extended manifest); catalog tooling passed 12/12 tests; RockServer targeted catalog tests passed
14/14; RockCast targeted personal-data tests passed 5/5 using a separate writable target directory
after its ordinary target lock was inaccessible. RockServer fmt, strict Clippy and full tests, а
также RockCast fmt, strict Clippy и full tests прошли; RockCast full result — 55 library и 2 relay
integration tests, 8 live-network tests ignored. RockMobile targeted unit and lint commands both
stopped before compilation on the inaccessible Gradle wrapper `.zip.lck`, despite the required
process-local `-Duser.home=C:\Users\alex`; lint is not claimed as passed and its three previously
known errors remain unresolved/unverified in this run. No network, production service, secret,
live database, device test, product code or catalog data was changed.

OPS-001-A remained independently available as design-only work. The owner later manually
confirmed RM-007-D for the limited purpose of proceeding with RM-011-A proposal work on
2026-08-26; this historical review remains evidence and must not be reinterpreted as a passed
client verification or a compatible-sync guarantee.

## OPS-001-A production deployment design

`deploy/README.md` now records the proposed single-VPS boundary: Caddy alone exposes 80/443;
RockServer (3000) and PostgreSQL (5432) remain Compose-internal, with named data/certificate
volumes, off-VPS encrypted logical backups, readiness-based rollout, and a compatibility-aware
application/database rollback procedure. It defines the safe exact placeholder
`api.rockserver.example.invalid`, an environment-variable contract with no values, owners and the
required restore rehearsal. OPS-001-B is the implementation that closes the former production gaps:
the service now requires both its Bearer credential and `DATABASE_URL`. No VPS, DNS, production
credential, registry image, firewall or deployment was created.

OPS-001-A design review was explicitly confirmed by the user on 2026-08-25. The approved design
uses the documented non-routable placeholder until a real owned domain/DNS and deployment owners
are supplied as a separate manual launch step; it does not authorize publication, credentials or
VPS changes. Secret injection, backup encryption/retention/key custody/RPO/RTO, SSH allowlist,
Caddy ACME policy and restore authority remain pre-launch operational inputs, not repository
secrets.
`git diff --check`, `cargo fmt --check`, strict all-target/all-feature Clippy, and `cargo test`
passed locally (81 regular Rust unit tests plus HTTP/contract suites; two disposable-PostgreSQL,
four billable Yandex LLM, and one credential/audio SpeechKit integration tests remained explicitly
ignored). No Docker or external-infrastructure check applied to that design-only task.

## OPS-001-B reproducible container and Compose stack

OPS-001-B adds the root `Dockerfile`, `.dockerignore`, `deploy/compose.yaml`, local and production
overrides, Caddy templates, a safe `.env.example`, and `deploy/verify-compose.ps1`. The base topology
contains `caddy`, `rockserver`, and `postgres`; only Caddy receives host ports in either local or
production launch, while PostgreSQL stays on the internal `database` network. The application and
PostgreSQL have healthchecks, Caddy waits for application readiness, and the local check reaches
`/health/ready` through loopback Caddy.

Docker image build passed with the pinned Rust 1.95/bookworm builder and non-root runtime. Local
Compose configuration and production rendering passed without printing environment values. The
disposable local stack reached healthy PostgreSQL, RockServer and Caddy states, and the loopback
Caddy request to `GET /health/ready` returned HTTP 200; the script removed its containers, network
and volumes afterward. Production launch remains a manual step requiring a real owned domain,
secret injection, immutable image, VPS, DNS, firewall and backup owners. No production external
action or secret was performed.

OPS-001-B verification: `cargo fmt --check` and strict all-target/all-feature Clippy passed;
`cargo test` passed with 82 regular tests plus HTTP/OpenAPI/WebSocket suites using the serial
`target/ops001b-test-serial` target to fit this host's pagefile. Two disposable-PostgreSQL, four
billable Yandex LLM, one credential/audio SpeechKit and one ONNX asset test remained explicitly
ignored. `graphify update .` refreshed the local graph to 1,673 nodes, 3,192 edges and 98
communities. OPS-001-B is **passed locally**; public deployment remains outside this task.

### RM-007-D remediation

The four original High findings were remediated in client source: RockMobile now uses the portable
v1 profile/timestamp shape with checked persistence, fail-closed reads, legacy backup/journal,
explicit rollback and wired baseline lifecycle resolution; RockCast now preserves canonical server
IDs for search/voice personal records and completes dedup/journal/rollback behavior. RockCast fmt,
strict Clippy and full tests pass after remediation. Android targeted unit and lint commands
remained blocked before compilation by the inaccessible Gradle wrapper lock. The owner later
manually confirmed RM-007-D for RM-011-A proposal work; that decision does not claim the blocked
Android verification as passed.

## OPS-001-C CI image, release, backup and rollback runbook

OPS-001-C is implemented locally. `.github/workflows/ci-release.yml` runs the required Rust
format/Clippy/test checks, builds a container labelled with the source commit SHA, and runs a
loopback Compose readiness smoke using credentials generated only inside the CI job. Publishing to
GHCR is behind `workflow_dispatch`, `publish_image=true`, and the repository `release-gate`
environment approval. No registry credential or application secret is stored in the workflow.

`deploy/release.ps1` is the documented release entry point. It accepts only an image reference with
an SHA-256 digest, renders production Compose without printing its environment, rejects host ports
on PostgreSQL/RockServer, and requires an environment file outside the repository for deploy or
rollback. Deploy backs up PostgreSQL in custom `pg_dump` format before starting the immutable image,
records the backup SHA-256 beside a release record, waits for Docker health and then requires an
approved readiness URL to return HTTP 200. Rollback reuses the same path with a previous verified
image digest; incompatible migrations still require the documented backup restore procedure.

`deploy/restore-rehearsal.ps1` performs a non-production `pg_restore` into a disposable PostgreSQL
container/network, validates restored RockServer tables, runs the local RockServer image against
the restored database and checks readiness inside that network. Temporary credentials are generated
at runtime and are not printed. Disposable cleanup completed successfully in the verified rehearsal.

OPS-001-C local verification on 2026-08-25: PowerShell syntax checks passed for all deployment
scripts; production Compose rendering and port isolation passed; the pinned Docker image built;
the local Compose stack reached healthy PostgreSQL/RockServer/Caddy and loopback readiness HTTP 200;
release preflight dry-runs passed for both `preflight` and `rollback`; and the full disposable
`pg_dump`/`pg_restore` rehearsal passed with restored application readiness. The first rehearsal
attempt exposed and fixed a binary stdout redirection issue and a missing explicit restore username;
the corrected run passed. Final local checks also passed: `git diff --check`, `cargo fmt --check`,
`cargo clippy --all-targets --all-features -- -D warnings`, `cargo test` (82 passed; 2 PostgreSQL,
4 Yandex LLM, 1 SpeechKit and 1 ONNX integration cases ignored by their explicit gates), and
`graphify update .` (1,683 nodes, 3,204 edges, 103 communities; the existing zero-node JSON
warnings remain non-fatal).

The actual GHCR publication, production backup/deploy, public HTTPS readiness, and live rollback
remain manual actions requiring an approved registry/release environment, owned domain/VPS,
external backup target and injected secrets. They were not performed or claimed here.

## MVP-001-B — anonymous `/v1` public policy (local implementation)

`/v1/catalog/stations`, `/v1/catalog/stations/{station_id}`, `/v1/search`,
`/v1/voice/command`, and `/v1/voice/stream` now use the approved anonymous route boundary;
the existing `/api/v1` aliases retain Bearer authentication. Anonymous JSON is capped at 16 KiB;
search/transcript length is capped at 500 characters and results at 20/10. Until trusted proxy
CIDRs are configured, forwarded headers are ignored and a conservative direct-peer fallback
bucket is used. Voice streaming has global admission, 16 kHz PCM16 mono, 32 KiB frames,
2 MiB/60-second audio, and bounded idle/wall/STT/search time.

The changed public handlers log only aggregate-safe fields; request content, transcripts, URLs,
credentials, provider payloads, IPs and forwarded headers are not emitted. `api/openapi.yaml`
remains unchanged until it is reconciled line-for-line with the runtime contract.

## MVP-001-C follow-up — local launcher credential fallback

`run-rockserver-local.ps1` no longer rejects a local `.env` solely because it
omits `ROCKSERVER_API_BEARER_TOKEN`. It generates a random, non-production,
process-local development credential only for the legacy protected `/api/v1`
and local admin access gate. The anonymous `/v1` allowlist remains
unauthenticated; production startup policy is unchanged.

## RM-011-C — unified web shell and auth runtime (current snapshot)

Итоговый отчёт текущего прогона: [`docs/rm-011-c-report.md`](rm-011-c-report.md). RM-011-C
имеет статус **RM-011-C server/browser implementation verified**. Full real-client staging E2E
явно перенесён в RM-011-D/E и не является prerequisite этой серверной задачи.

The chosen single first-party frontend/admin stack is **TypeScript + Vite + Preact** in `web/`.
It has one shared same-origin API client and types, passkey registration/authentication, pairing
lookup/approval, and local QR presentation; it never stores a bearer, refresh, browser-session or
pairing secret in localStorage. Caddy serves the static bundle and proxies only `/v1`, `/api/v1`,
and health routes to RockServer, injecting a deployment-only proxy proof header so direct browser
requests fail closed. The Rust routes include registration and authentication options/verify with
cryptographic WebAuthn checks, Secure/HttpOnly/SameSite cookies, CSRF/origin checks, PostgreSQL
rate limits, and desktop pairing completion with one-time native access/refresh issuance.
The browser never receives native tokens; the desktop completion client belongs to RM-011-E.

The runtime now also creates a ten-minute desktop pairing request at `POST /v1/pairing-requests`
and resolves pending short codes at `GET /v1/pairing-requests/lookup?code=...`. PostgreSQL supplies
the expiry clock and persists only SHA-256 proof hashes; lookup exposes only device metadata and the
verification phrase. Approval, completion and request throttling are implemented; the browser UI
executes approval, while the native desktop client remains responsible for calling completion.
Completion accepts only the desktop proof; PostgreSQL derives the owner from the approved request
inside the same transaction, so native clients never need or submit an account UUID.

The approve endpoint is now present at `POST /v1/pairing-requests/{request_id}/approve`; it
requires an `HttpOnly` browser cookie proof, the `X-CSRF-Token` double-submit header, exact
first-party HTTPS origin, and the configured Caddy proxy proof. A new nullable cookie-hash column
keeps old B2 rows readable while all newly created browser sessions bind an opaque cookie hash.

Native session/account management is now exposed through `POST /v1/auth/refresh`,
`POST /v1/auth/logout`, `GET /v1/account/profile`, `GET /v1/devices`, and
`DELETE /v1/devices/{device_id}`. Access-token lookup and device revocation are owner-scoped in
PostgreSQL; refresh rotation replaces the access token in the same transaction and replay revokes
the refresh family.

Verification on this snapshot: `cargo fmt --check`, `cargo clippy --all-targets --all-features --jobs 1
-- -D warnings`, `cargo test --jobs 1` (93 unit/library tests plus regular integration suites), web
`tsc --noEmit`, and a direct Vite production build with the bundled Node runtime all pass.
All four opt-in PostgreSQL integration tests passed sequentially against a disposable local
`pgvector/pgvector:pg17` container. Both Caddyfiles validated in Caddy 2.10 containers with test
environment values. Web dependencies were installed with the bundled pnpm runtime; `pnpm typecheck`,
`pnpm lint`, and `pnpm build` passed with the bundled Node path (the unconfigured host PATH still
has no `node` command).

RM-011-C is complete at its server/browser boundary. Real-client staging pairing and mobile/native
session UX are explicitly deferred to RM-011-D/E.

## HTTP transport refactor

The HTTP module is now a thin public facade. Route handlers and transport logic live in
`src/http/endpoints.rs`, while shared application state and anonymous admission controls live in
`src/http/state.rs`. Runtime behavior and the public API are unchanged.

Verification after the refactor: `cargo fmt --check`, `cargo clippy --all-targets --all-features
-- -D warnings`, and `cargo test` passed (92 unit tests plus regular integration suites; external
PostgreSQL, LLM, SpeechKit, and ONNX cases remain ignored behind their explicit gates).

Local Cargo concurrency is capped at two jobs via `.cargo/config.toml` to reduce CPU and memory
pressure during builds; acceptance checks in this run used explicit `--jobs 1` sequentially.
