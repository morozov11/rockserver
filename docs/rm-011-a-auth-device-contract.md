# RM-011-A — контракт account/device и threat model

**Статус:** human-approved proposal, revised and approved 2026-08-26. Это не описание текущего
runtime-поведения. Перечисленные ниже варианты recovery/retention и operational policy остаются
явными implementation decisions; approval не подменяет их невыбранным вариантом.

Связанный машиночитаемый черновик: `rm-011-a-openapi.proposed.yaml`. Действующий
`api/openapi.yaml` и HTTP runtime намеренно не изменены: сегодня они не предоставляют
регистрацию, login, account или device API.

## Выбранный пользовательский путь

Email, пароль, SMS и обязательная установка RockMobile **не являются частью proposal**.
Основной путь начинается на Windows desktop:

```text
RockCast desktop показывает QR и короткий код
      ↓                         ↓
мобильный браузер сканирует QR  или пользователь вводит код на странице RockServer
      ↓
браузер создаёт/использует passkey и показывает понятное подтверждение устройства
      ↓
сервер привязывает ожидающий desktop к account
      ↓
desktop забирает только свою новую session/token pair
```

Для QR не нужна камера на desktop: QR показывается на его экране, а камерой пользуется телефон.
Короткий код — равноценный ручной fallback для телефона без сканирования. RockMobile может в
будущем предоставить тот же UX, но не является ни доверенной стороной, ни prerequisite: это
first-party HTTPS mobile web page с WebAuthn/passkey.

**Phone-first registration** equally supported: user opens the same mobile web origin directly,
creates a passkey and receives a short-lived browser session. No desktop request is needed. Later
a Windows PC or ESP32 displays its own QR/code; the already signed-in mobile browser confirms it.
If registration began after scanning a desktop/ESP32 QR, the same ceremony first creates the account
and then presents that pending device for explicit approval.

MVP-001-A был одобрен владельцем 2026-08-26. Его anonymous `/v1` операции для каталога, search
и voice остаются независимыми от account: они не получают hidden shared Bearer token, client
secret или обязательный user identity. Account/device routes добавляются отдельно и не меняют
ни URL, ни лимиты anonymous MVP.

Входной статус RM-011-A фиксирует ручное подтверждение владельцем MVP-001 и RM-007-D и наличие
staging OPS-001. Это снимает плановый dependency gate для подготовки proposal; это не утверждение,
что старый технический report RM-007-D перестал быть историческим evidence, и не заменяет approval
ниже.

Не входят в RM-011-A: Rust handlers, migrations, PostgreSQL, email/SMS, client UI, production
secrets, внешние providers и сеть. Mobile browser UX описан только как будущая contract boundary,
а не как реализованная страница.

## Модель и ownership

Все public identifiers — UUID в canonical lower-case form; время — RFC 3339 UTC. Секреты ниже
хранятся и сравниваются только как keyed hash. Raw WebAuthn attestation/assertion, challenge,
access/refresh/desktop token, approval secret, QR string и short code не попадают в persistence,
response logs, trace spans или audit payloads.

| Record | Необходимые поля | Ownership и lifecycle |
|---|---|---|
| `User` | `id`, `status`, `created_at`, `updated_at`, `deleted_at` | account не содержит обязательных contact data. `deleted` не может получить session. |
| `PasskeyCredential` | `id`, `user_id`, `credential_id`, `public_key`, `sign_count`, `transports`, `created_at`, `last_used_at`, `revoked_at` | public key и credential ID не секреты, но остаются server-only. Один user имеет один или несколько passkeys. |
| `AccountIdentity` | `id`, `user_id`, `kind`, `subject_hash`, `subject_ciphertext`, `verified_at`, `created_at`, `revoked_at` | зарезервированный extension boundary; в текущем proposal не создаётся. Позже `email`, `phone`, `password` или external identity добавляется как отдельная verified identity к тому же `user_id`, а не как новая user/device/session model. |
| `Device` | `id`, `user_id`, `name`, `platform`, `app_version`, `created_at`, `last_seen_at`, `revoked_at` | принадлежит ровно одному active user. Удаление account revokes every device. Name is untrusted bounded display data. |
| `Session` | `id`, `user_id`, `device_id`, `access_token_hash`, `access_expires_at`, `refresh_family_id`, `revoked_at`, `last_seen_at`, `created_at` | отдельная desktop/native session; bearer access привязан к ней. Revoking device revokes its sessions. |
| `RefreshToken` | `id`, `session_id`, `family_id`, `token_hash`, `issued_at`, `expires_at`, `used_at`, `replaced_by_id`, `revoked_at` | append-only rotation chain. Plaintext появляется только в successful desktop completion/refresh response и не persistent. |
| `PairingRequest` | `id`, `desktop_token_hash`, `approval_secret_hash`, `short_code_hash`, `device_metadata`, `expires_at`, `approved_by_user_id`, `approved_at`, `consumed_at`, `revoked_at` | создаётся desktop до account. Один request можно approve ровно один раз и consume ровно один раз; после expiry/revoke никакая сторона не получает session. |
| `BrowserSession` | `id`, `user_id`, `passkey_reauthenticated_at`, `expires_at`, `revoked_at` | только first-party mobile browser, `Secure`, `HttpOnly`, `SameSite` cookie; не заменяет desktop bearer API. |

Response schemas intentionally omit all hashes, credential public-key material, IP address,
user-agent, raw identifiers, refresh family IDs, WebAuthn material and secrets. `UserProfile`
contains no email or phone.

`AccountIdentity` is deliberately an additive boundary, not a hidden future login implementation.
Current permitted authenticator is only `PasskeyCredential`. A future email identity needs a separate
approval for verified delivery, encrypted canonical address, unique normalized/hash lookup, change
and deletion semantics; a password identity needs its own Argon2id policy. Neither may be inferred
from passkey data. A future explicit login page can first offer passkey; optional email/phone/password
methods are additional authenticators behind the same `User` → `Device` → `Session` ownership rules.

## Passkey and session lifecycle

Passkey follows WebAuthn: the authenticator on the phone creates a private key that stays in the
device/OS credential manager and a public key stored by RockServer. The server verifies the
assertion's challenge, origin, RP ID, type, credential ID and signature before it creates a browser
session or treats a request as freshly reauthenticated. It must reject a credential belonging to a
different user, a reused/expired challenge, unexpected origin/RP ID or clone signal that the
approved sign-count policy considers unsafe.

- A new phone browser can open the first-party registration page directly and create its first
  discoverable passkey. That atomically creates `User`, `PasskeyCredential` and a browser session,
  without a desktop request. The identical ceremony can run after a QR/code continuation; it still
  does **not** reveal a desktop token to the browser.
- An existing passkey signs the user into a short-lived first-party browser session. The browser
  sees a device name/platform and explicitly approves or cancels pairing.
- The desktop authenticates its `complete` request with its high-entropy desktop token, not the QR
  secret or short code. After approval it receives a distinct opaque access token (maximum 10 min)
  and a distinct opaque refresh token (maximum 30 days).
- Each refresh atomically consumes its old token and creates one successor. Replay of used, expired,
  revoked or unknown refresh token returns the same `401 invalid_refresh_token`; a detected reuse
  revokes the complete family/session and records an audit event.
- `POST /v1/auth/logout` revokes the current bearer session/family. Device revoke atomically revokes
  its sessions/families. Account deletion requires a current bearer and a fresh passkey assertion,
  then revokes all browser sessions, desktop sessions, devices, refresh families, passkeys and
  pending pairing requests.

Native account scopes are `profile:read`, `session:write`, `device:read`, `device:revoke`, and
`account:delete`; they authorize only the authenticated subject. `pairing:approve` is a browser
session action, not a token copied into web JavaScript. A future non-interactive device receives
only `device:connect`, restricted to its own future device route family; it never receives an owner
passkey, browser cookie, broad user scopes or a desktop/mobile refresh token.

## QR and short-code protocol

`POST /v1/pairing-requests` is called by unauthenticated desktop and creates an **unapproved**
request. Its response has three independent proofs:

- `desktop_token`: high-entropy secret kept only by the desktop; it can complete/poll, but cannot
  approve or choose an account.
- `approval_uri`: high-entropy one-time URL rendered as the QR code for the phone browser. It can
  locate the approval screen, but still requires passkey authentication and human confirmation.
- `short_code`: an 8-character unambiguous Base32 fallback. It only locates an unexpired request
  after a rate-limited browser lookup; the browser still displays device details and requires the
  same passkey confirmation.

Default TTL is five minutes. A request allows one approve and one completion; use/expiry/revoke
invalidates every proof. The desktop displays a non-secret verification phrase derived from the
request; the phone shows the same phrase before approval, protecting against approving the wrong
visible desktop. QR and code are never passwords, account credentials or long-lived bearer values.
The user can cancel from mobile browser before desktop completion.

The mobile page must set strict CSP, `Referrer-Policy: no-referrer`, no-store cache headers and
never embed approval URI/code in third-party resources. Browser state uses HTTPS-only secure,
HttpOnly, SameSite cookies plus CSRF protection; it is not a generic CORS API. The exact page
presentation belongs to RM-011-C/RockCast/RockMobile work, but these security properties are
contract requirements.

## HTTP contract and safe errors

`docs/rm-011-a-openapi.proposed.yaml` is the approval-only JSON/API surface; it separately marks
the first-party browser session. The minimal families are:

| Route | Auth | Semantics |
|---|---|---|
| `POST /v1/pairing-requests` | anonymous desktop | Creates a five-minute desktop request and returns the QR URI, short code and desktop proof once. |
| `POST /v1/auth/passkeys/{registration,authentication}/{options,verify}` | first-party browser | Registration can be phone-first or follow QR/code continuation; verification creates/uses browser session. |
| `POST /v1/pairing-requests/{id}/approve` | mobile browser session + fresh passkey | Explicitly binds request to that user; no desktop credential in response. |
| `POST /v1/pairing-requests/{id}/complete` | desktop proof | Returns a new owner desktop `TokenPair` exactly once after approval. |
| `POST /v1/auth/refresh`, `POST /v1/auth/logout` | refresh body / `session:write` bearer | Rotate or revoke native desktop session. |
| `GET /v1/account/profile`, `DELETE /v1/account` | bearer; delete also fresh passkey | Profile has no contact field; deletion begins revoke-all. |
| `GET /v1/devices`, `DELETE /v1/devices/{device_id}` | owner bearer | List/revoke only caller-owned desktop/native devices. |

Every error follows the existing `code`, neutral `message`, `request_id`, `details` envelope.
Absent/expired/used/revoked/unknown desktop proof, approval URI, code and pairing request use the
same safe error class; invalid device ownership uses the same `404` for missing/foreign IDs.
`401` sends `WWW-Authenticate: Bearer` only for bearer routes. `403 insufficient_scope` is for a
valid bearer lacking scope, never for an ownership probe. Browser pages must not disclose whether a
passkey/account exists before the WebAuthn ceremony completes.

Rate limits run before expensive WebAuthn/database work where possible: pairing create 6/hour/IP,
QR/code lookup 10/15 min/IP, approval 5/15 min/browser session, completion/poll 30/15 min/IP and
10/request, passkey ceremony 10/15 min/IP, refresh 30/15 min/IP and 20/session. `429` includes
`Retry-After` but never key/request/account existence information. Proxy identity uses explicit
trusted CIDRs; missing required proxy identity fails closed, otherwise direct peer is the key.

Server diagnostics may retain only request ID, internal event ID, outcome class and a keyed rotating
rate-limit-key hash. They never store token/header/cookie, passkey assertion or challenge, QR URI,
short code, IP, user agent or response body. Append-only, access-controlled, retention-bounded audit
events are: pairing created/lookup-throttled/approved/cancelled/expired/completed; passkey
registered/asserted/revoked; refresh rotated/reuse-detected; logout; device revoke; account deletion
accepted/completed. Audit records are not a credential-recovery source.

## Threat model

| Threat | Contract mitigation | Required proof |
|---|---|---|
| QR screenshot, shoulder-surfing or short-code guessing | separate desktop/approval secrets, short TTL, 8-char code, request/IP limits, mobile passkey and visible verification phrase | expiry/replay/race/wrong-phrase tests |
| Attacker attaches their account to another waiting desktop | code/QR only locate; approval requires authenticated user confirmation and matching phrase; desktop proof is separate | account-swap and two-user integration tests |
| Phishing / credential database breach | WebAuthn RP ID/origin binding; no password/email/SMS secret; only public credential material is stored | origin/RP/challenge/signature negative tests |
| Lost phone or all passkeys | no silent recovery; user needs another registered passkey or owner-approved recovery mechanism | recovery decision and revoke/loss tests |
| Desktop/refresh-token theft | opaque short-lived access, hashed persistence, rotation/reuse family revoke, server-side device revoke | redaction, rotation-race and incident tests |
| Cross-account IDOR | owner derived from session; every device query constrained by `user_id`; same foreign/missing 404 | two-user PostgreSQL/API tests |
| Browser XSS/CSRF/token leakage | first-party secure HttpOnly session, CSRF, CSP, no-referrer/no-store; browser never receives desktop token | header/cookie/CSP and malicious-origin tests |
| Proxy spoofing/rate-limit bypass | explicit trusted-proxy CIDRs and fail-closed requirement | staging reverse-proxy test |

## Human approval decisions

On 2026-08-26 the owner approved the RM-011-B2 values: `alex.vault57.ru` as staging RP ID and
first-party origin, synchronized passkeys, no automatic recovery after loss of all passkeys, a
ten-device account cap, 90-day audit retention, and the recommended access/refresh/browser/pairing
lifetimes, code alphabet, passkey multiplicity and clone/sign-count policy.
The owner also approved the operational topology: Caddy is the only trusted reverse proxy, direct
connections fail closed, and rate-limit state is stored in PostgreSQL.

Implementation remains blocked only until these technical prerequisites are available:

1. WebAuthn library/security review and concrete staging/production deployment configuration.
2. Audit incident-reader roles, keyed-IP-hash rotation and the concrete Caddy trusted-proxy CIDR
   wiring.
3. Hosting, CSP/CSRF review, accessibility and no-JavaScript/fallback behavior for the first-party
   mobile web origin. RockMobile installation remains explicitly optional.

Current-device self-revoke and the recommended account-deletion purge mechanics are approved.
Explicit login, email, phone, password and external identities remain optional future work and do
not block RM-011-B2.

## RM-011-B/C prerequisites

**Before RM-011-B2:** the approved policy above; OPS-reviewed HTTPS, trusted-proxy and durable
PostgreSQL baseline; a WebAuthn library/security review; migration plan for `users`,
`passkey_credentials`, reserved `account_identities`, `devices`, `sessions`, `refresh_tokens`, `pairing_requests`, browser session,
rate-limit state and audit events; expiry/revocation/uniqueness indexes; transaction/locking plan for
approve/complete and refresh rotation; backup/retention plan; deterministic fakes/tests without an
external call.

**RM-011-B must prove:** a request cannot complete before approval; only the matching desktop proof
can complete once; QR/code reuse/expiry/race is safe; WebAuthn challenge/origin/RP/signature checks
are strict; a fresh assertion is required for approval/deletion; no secret/WebAuthn payload is
logged; refresh reuse revokes family; revoke/deletion takes effect for HTTP/WebSocket authorization;
and two-user ownership isolation holds.

**Before RM-011-C:** RM-011-B review passed, migrations are deployed through the approved OPS
procedure, this proposal is reconciled into runtime `api/openapi.yaml`, first-party web origin and
WebAuthn configuration are validated, and contract/integration tests exist. RM-011-C keeps every
MVP-001 anonymous route compatible and does not require RockMobile installation for mobile approval.
An explicit login page or an email method is a separately approved additive route family, not an
implicit prerequisite for QR pairing or phone-first registration.

## Non-claims

No user can currently create/assert a passkey, pair a desktop, refresh/log out, list/revoke a device
or delete an account through RockServer. The proposal is deliberately not merged into the runtime
contract until human approval and implementation verify these statements.
