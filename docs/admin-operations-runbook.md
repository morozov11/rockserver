# Administrator operations runbook

## Scope and retention

This runbook covers the separate RockServer administrator identity and its opaque
Bearer sessions. RockCast product routes are public and user identity is pairing;
this document does not create, rotate, revoke, or otherwise manage RockCast
machine credentials.

Administrator request records retain only `request_id`, administrator principal
and session UUIDs, static endpoint, safe outcome, duration and timestamp. They
never retain an Authorization header, Bearer value, password, browser data,
request body, search text, or voice transcript. The service enforces a 30-day
retention window on every new request-record write. Database backups may contain
records within that window and follow the same access controls.

## Backup and restore

1. Use the deployment-approved PostgreSQL backup process with encryption at rest
   and in transit; do not copy database dumps into source control, chat, or a
   workstation downloads directory.
2. Restrict backup and restore access to designated operators. A backup contains
   Argon2id hashes and opaque-token hashes, never usable raw admin Bearers.
3. Before a restore, declare an incident/change window and restore into an
   isolated environment first. Verify migration `0020` and administrator-table
   row counts without printing security-sensitive fields.
4. A database restore can resurrect sessions that were valid at backup time.
   Immediately revoke all administrator sessions after restore, then require
   administrators to sign in again.

## Incident response and session revocation

1. Treat a suspected Bearer exposure, unexpected administrator request record,
   lost administrator device, or compromised browser as an incident.
2. Preserve request IDs and timestamps from safe logs; never paste Bearers,
   passwords, database URLs, or headers into the incident record.
3. Revoke the affected session immediately. If scope is uncertain, use a separate
   approved and audited incident operation to revoke all administrator sessions
   and force re-login; do not change product route authentication or pairing.
4. Rotate deployment secrets only through the deployment secret manager and
   restart through the existing change process. A refresh atomically replaces one
   session, so a replayed old Bearer is rejected after success.
5. Review safe security events and bounded request records, capture the timeline,
   and document restoration/closure under the approved security process.

## Deployment acceptance checklist

- TLS terminates at the approved reverse proxy; direct public application access
  is not exposed.
- `ROCKSERVER_TRUSTED_PROXY_TOKEN` is supplied only by the reverse proxy and is
  not logged or committed. Browser admin state changes present the canonical
  HTTPS Origin and proxy protocol marker.
- Bootstrap credentials are supplied only to the protected one-time terminal or
  deployment process, never in a URL, API payload, or source-controlled file.
- `/admin` and `/api/v1/admin/*` responses have `Cache-Control: no-store`, restrictive
  same-origin CSP, no third-party assets, nosniff, referrer and permissions
  protections. The console keeps the Bearer only in current-page memory.
- Verify login, refresh, logout, expired/revoked session rejection, and public
  `/v1` product-route behavior with non-secret test credentials in the approved
  environment.
