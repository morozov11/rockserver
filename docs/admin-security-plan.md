# Admin console and API access plan

## Decision record

RockServer will expose a server-rendered administrative console built with
Axum HTML handlers and HTMX. The administrative API uses explicit `Bearer`
session tokens, rather than cookie authentication. The browser keeps an
access token in memory and attaches it to HTMX requests; the first delivery
does not persist an administrator token in `localStorage`. This avoids
cookie-driven CSRF, while reducing the impact of a future XSS defect.

RockCast product routes are currently public and its user identity uses pairing.
RockCast machine-credential lifecycle is explicitly outside this administrator roadmap.

## Security boundaries

- Product endpoints under `/v1` remain public; legacy `/api/v1` compatibility
  routes retain their separately configured deployment Bearer boundary.
- Liveness remains suitable for local process supervision. Readiness and
  administrative access are deployment-controlled and must not expose secrets.
- Admin Bearer tokens have the `admin` role, a short lifetime, server-side
  revocation, and rotation on login/refresh.
- Passwords use Argon2id. Tokens and session identifiers are compared and
  persisted as hashes only. Authorization, passwords, and tokens are always
  redacted from logs.
- Login throttling combines an account and source-IP key, progressive delay,
  temporary lockout, generic failures, and a security audit event. It must be
  durable across service instances.
- Bearer authentication removes cookie-CSRF as the primary threat, but all
  state-changing admin requests still validate `Origin` as defense in depth.
  The admin UI requires a restrictive CSP, escaped HTML, no third-party
  scripts, `Cache-Control: no-store`, and no sensitive token persistence by
  default.
- Session refresh is an atomic replace-and-revoke persistence operation: a
  failure leaves the old session active and does not mint a usable replacement.
- Authenticated administrator request records contain only request ID,
  principal/session IDs, a static route name, outcome, duration, and timestamp.
  They exclude request bodies, query text, transcripts, headers, credentials and
  token material. The implemented retention period is 30 days, enforced when a
  new record is written.

## Delivery sequence

### A. Authentication foundation and API gate

1. Add durable principal, credential, session, login-attempt, and security
   event storage migrations.
2. Add a domain/persistence boundary for credential verification and test fakes.
3. Add Bearer middleware to protect public application endpoints while keeping
   health semantics explicit.
4. Add OpenAPI security schemes and `401`, `403`, and `429` responses.
5. Cover absent, malformed, expired, revoked, and valid credentials for HTTP
   and WebSocket upgrades.

### B. Administrator sign-in and abuse resistance

1. Add a bootstrap-admin command that accepts a password only from a terminal
   or protected environment; there is no public registration endpoint.
2. Add login, short-lived opaque access sessions, logout, server-side
   revocation, and audit events.
3. Implement login throttling and retention-safe security logs.

### C. Structured operational records

1. Persist a bounded request record with request ID, principal, endpoint,
   outcome, duration, and timestamp.
2. Decide and document a retention period before recording search text or voice
   transcripts; credentials and raw sensitive headers are never recorded.
3. Provide filtered, paginated read models for stations and request logs in a
   later explicitly scoped task; RS-ADMIN-005 records metadata only.

### D. HTMX administration console

1. Serve a minimal HTML shell and HTMX fragments for login and stations; request
   logs and security-event pages require later scoped work.
2. Keep presentation DTOs separate from search DTOs and persistence rows.
3. Attach the in-memory Bearer access token to HTMX requests and clear it on
   `401` or logout.

### E. Deployment hardening and acceptance

1. Document TLS/reverse-proxy trust, token/bootstrap configuration, backup,
   retention, and incident revocation procedures.
2. Add CSP and response-security headers, request limits, rate limits, and
   observability without logging secrets.
3. Verify with deterministic unit, HTTP, contract, and PostgreSQL integration
   tests. No ordinary test may contact an external provider.

## Definition of done

Administrator operations are inaccessible without a valid revocable credential;
failed logins are throttled; the admin can list stations; and security-sensitive
values never appear in response bodies, trace logs, request records, or persisted
audit records. Product routes remain public and pairing remains the user-identity
boundary. Every public HTTP change is represented in `api/openapi.yaml`.

## RM-011-G6 operator boundary

The staging cleanup path is deliberately separate from this future admin-session console. The
`account_cleanup` one-shot binary runs only from the root-scoped deployment wrapper, defaults to
read-only preview, and accepts one exact account/device/passkey row UUID plus an action-specific
confirmation phrase for mutation. It uses the existing PostgreSQL tombstone/revoke policy and
safe audit vocabulary; it does not create an admin role, add a public HTTP route, merge accounts,
or expose token/credential material. A live admin identity, if represented in the reserved
`account_identities` table, blocks account deactivation, and a live account's last passkey cannot
be revoked.
