# Admin console and API access plan

## Decision record

RockServer will expose a server-rendered administrative console built with
Axum HTML handlers and HTMX. The administrative API uses explicit `Bearer`
session tokens, rather than cookie authentication. The browser keeps an
access token in memory and attaches it to HTMX requests; the first delivery
does not persist an administrator token in `localStorage`. This avoids
cookie-driven CSRF, while reducing the impact of a future XSS defect.

RockCast is a separate machine client. An administrator creates a revocable
client credential for each RockCast installation. Credentials are random,
shown once at creation, and stored only as hashes. They never grant access to
the administration console.

## Security boundaries

- Application endpoints under `/v1` and `/api/v1` require a valid Bearer
  credential. This includes search and voice endpoints before a WebSocket
  upgrade.
- Liveness remains suitable for local process supervision. Readiness and
  administrative access are deployment-controlled and must not expose secrets.
- Admin Bearer tokens have the `admin` role, a short lifetime, server-side
  revocation, and rotation on login/refresh.
- RockCast client credentials have the `client` role, an independently
  configurable expiry, last-use metadata, and revocation.
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
3. Provide filtered, paginated read models for stations and request logs.

### D. HTMX administration console

1. Serve a minimal HTML shell and HTMX fragments for login, stations, request
   logs, RockCast clients, and security events.
2. Keep presentation DTOs separate from search DTOs and persistence rows.
3. Attach the in-memory Bearer access token to HTMX requests and clear it on
   `401` or logout.
4. Create and rotate RockCast credentials through an admin-only operation;
   display a newly minted secret exactly once.

### E. Deployment hardening and acceptance

1. Document TLS/reverse-proxy trust, token/bootstrap configuration, backup,
   retention, and incident revocation procedures.
2. Add CSP and response-security headers, request limits, rate limits, and
   observability without logging secrets.
3. Verify with deterministic unit, HTTP, contract, and PostgreSQL integration
   tests. No ordinary test may contact an external provider.

## Definition of done

The API is inaccessible without a valid revocable credential; administrative
operations require an admin role; a disabled RockCast client loses access
without affecting other clients; failed logins are throttled; the admin can
list stations and filtered request records; security-sensitive values never
appear in response bodies, trace logs, or persisted audit records. Every
public HTTP change is represented in `api/openapi.yaml`.

## RM-011-G6 operator boundary

The staging cleanup path is deliberately separate from this future admin-session console. The
`account_cleanup` one-shot binary runs only from the root-scoped deployment wrapper, defaults to
read-only preview, and accepts one exact account/device/passkey row UUID plus an action-specific
confirmation phrase for mutation. It uses the existing PostgreSQL tombstone/revoke policy and
safe audit vocabulary; it does not create an admin role, add a public HTTP route, merge accounts,
or expose token/credential material. A live admin identity, if represented in the reserved
`account_identities` table, blocks account deactivation, and a live account's last passkey cannot
be revoked.
