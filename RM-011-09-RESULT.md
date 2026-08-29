# RM-011-09 — RockServer Wave 9 A4 result

## Result

The web client now accepts the approval secret from `#secret` and immediately removes it from the
address bar and current history entry before any lookup/effect runs. The short code remains in the
query. The first render retains the parsed handoff only in memory, so rerenders cannot reread the
scrubbed URL. The old `?secret=` input remains accepted only through 2026-09-29T00:00:00Z, is
scrubbed immediately, and is neither logged, rendered, persisted nor placed in a
navigation/referrer.

No HTTP/API/OpenAPI or pairing lifecycle behavior changed. The existing approval request remains
the only place that sends the in-memory secret, in its same-origin JSON body.

## Verification

- `cd web && pnpm test` — passed, 10/10.
- `cd web && pnpm build` — passed.
- `cargo fmt --check` — passed.
- `cargo clippy --all-targets --all-features -- -D warnings` — passed.
- `cargo test` — passed; six disposable PostgreSQL and six credential/live tests were ignored.
- `git diff --check` — passed after this documentation commit.

## App Link blocker

This repository contains no `assetlinks.json`/`.well-known` asset; production Caddy serves only
the built web files. The required published statement and the private release-signing certificate
fingerprint are external to this checkout. No deploy or production verification was performed.

## Commits

- Implementations: `2128772e28d0ee76f07b7b755b075d8033aad03d`,
  `38ecf1d5a60768e62bffb62b7ffeba3fdc642eb7`
- Documentation: this commit
