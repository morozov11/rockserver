# MVP-001-A — публичный API-контракт и модель защиты

**Статус:** approved владельцем 2026-08-26. MVP-001-B может реализовать этот контракт. Это всё
ещё не текущий runtime и не разрешение на rollout, изменение провайдеров или секретов.

## Решение и область

Документ конкретизирует MVP-001 из [shared-product-roadmap.md](shared-product-roadmap.md) и
MVP-001-A из [shared-product-execution-plan.md](shared-product-execution-plan.md). Официальный
клиент получает каталог, поиск и голосовой сценарий без регистрации, URL-настройки или
учётного секрета; локальный каталог/воспроизведение остаётся fallback.

Release-клиент не посылает общий `Authorization: Bearer` token и не содержит API key,
SpeechKit/Yandex key, HMAC secret, client secret или иной статический доверительный материал.
Его можно извлечь, поэтому он не является авторизацией публичного пользователя. Provider
credentials существуют только в server environment и никогда не попадают в HTTP-ответы, логи,
OpenAPI examples или клиентскую конфигурацию.

Текущий `api/openapi.yaml` и runtime пока Bearer-защищены. Proposed contract находится в
[mvp-001-a-openapi.proposed.yaml](mvp-001-a-openapi.proposed.yaml); его внедрение — отдельная,
после-approval задача MVP-001-B.

## Точный anonymous allowlist

Только эти product endpoints доступны без `Authorization` и без любого клиентского секрета.
Все используют HTTPS, выдают `X-Request-Id`; caller-provided request ID — только correlation ID,
не credential.

| Endpoint | Назначение | Limit | Запрещено |
| --- | --- | --- | --- |
| `GET /v1/catalog/stations` | страничный список active/playable station summaries | 60 req/min/IP, burst 20; page 1–50; opaque signed cursor <=512 bytes | bulk export, source/provenance/admin metadata |
| `GET /v1/catalog/stations/{station_id}` | active station summary по stable ID | 120 req/min/IP, burst 40 | retired/tombstoned/provider-internal data |
| `POST /v1/search` | bounded text discovery | 30 req/min/IP, burst 10; JSON <=16 KiB, query <=500 chars, results <=20, deadline 5 s | unbounded catalog scan, URL override |
| `POST /v1/voice/command` | recognized text transcript → bounded search | 12 req/min/IP, burst 4; JSON <=16 KiB, transcript <=500 chars, results <=10, deadline 5 s | audio, provider mode/credentials, transcript log |
| `GET /v1/voice/stream` (WebSocket) | one bounded PCM16 mono voice session | 6 upgrades/min/IP, burst 2; 1 active/IP, 2/IPv6-/64, global 100 | browser-origin trust, reconnect/audio queue, client-selected provider credentials |

`IP` — immediate peer для direct connection; за утверждённым proxy — только forwarded client IP
от allowlisted proxy CIDR. Не доверять произвольному `X-Forwarded-For`; если client IP не может
быть безопасно определён, применять более строгий connection/proxy bucket. Ключ quota — rotating
keyed hash normalized IP; raw IP не писать в application logs. `429` не создаёт provider call.

### Audio/session/resource budget

Stream: одна start frame, PCM frames, одна commit frame. Только PCM signed-16 little-endian mono,
16 kHz; frame <=32 KiB, total <=2 MiB и <=60 s audio, wall <=75 s, idle <=10 s, STT <=15 s,
final search <=5 s. Server закрывает сессию до provider call при format/order/limit violation.

Admission control резервирует global slot до `101` и освобождает его на каждом terminal path.
При global cap: `503 voice_capacity_exhausted`, `Retry-After: 30`; это не quota. Per-IP или
concurrency exhaustion: `429 voice_session_limited`. Очередь аудио/сессий запрещена.

## Protected/admin/account/device policy

Вне allowlist application routes fail closed: ещё не реализованные — `404`; реализованные без
valid user/session credential — `401 authentication_required`; недостаточный scope/role — `403
forbidden`. Нельзя заменять user auth одним deployment Bearer.

| Family | Policy |
| --- | --- |
| `/v1/account/*`, `/v1/auth/*`, `/v1/sessions/*` | вне MVP-001; RM-011 отдельно определяет account token, rotation, anti-enumeration и scopes |
| `/v1/me/*`, `/v1/sync/*`, favourites/history/preferences | только owner short-lived access token; RM-012 |
| `/v1/devices/*`, `/v1/device-commands/*` | owner account/device-scoped credential; ESP32 не получает password/main refresh token |
| `/admin`, `/v1/admin/*`, catalog write/import, providers/config/metrics/debug | separate operator/admin network boundary and role; никогда anonymous |
| `/health/live`, `/health/ready` | только infrastructure/reverse-proxy allowlist, не product public API |
| `/api/v1/*` и existing aliases | не входят в allowlist; migration — отдельное compatibility решение |

## Errors, metrics и логи

Все JSON errors: `{code, message, request_id, details}`. `details` содержит только bounded
machine-readable limits/field names, никогда provider/policy internals, tokens, audio/transcript,
stream URL secrets или stack trace.

| Status/code | Condition | Response |
| --- | --- | --- |
| `400 malformed_request` | invalid JSON/upgrade/frame/order/codec | `request_id`, safe terminal WS error/close |
| `413 request_too_large` / `audio_too_large` | body/frame/aggregate bound | `details.max_bytes` only |
| `422 validation_failed` | schema/value violation | safe field reason only |
| `429 rate_limited` / `voice_session_limited` | request/upgrade/concurrency quota | integer `Retry-After`, `details.limit_scope` only |
| `503 voice_capacity_exhausted` / `service_unavailable` | global admission/provider/readiness | retryable `Retry-After` when known; generic body |
| `504 voice_timeout` / `search_timeout` | bounded deadline | `details.timeout_ms`, no upstream identity |
| `401` / `403` | protected route only | standard `WWW-Authenticate`; missing/invalid credential indistinguishable |

`500 internal_error` is generic and includes only `request_id`. Public client retries idempotent GET
and `429`/`503` after `Retry-After`; voice discards partial capture and returns to local fallback.

Metrics have aggregate labels only: `endpoint`, `method`, `status`, `error_code`, `limit_scope`,
`ip_source`, `provider_outcome`, plus bounded latency/size buckets. Required series:
`public_rate_limited_total`, `voice_upgrade_rejected_total{reason}`, `voice_active_sessions`,
`voice_audio_seconds_total`, `public_request_rejected_total{reason}`, `client_ip_source_total`,
`protected_route_denied_total` and aggregate provider operation/cost estimate. No raw request ID,
IP, query, transcript, user ID or URL as label.

Logs: timestamp, request ID, endpoint template, method, final status/code, duration, size bucket,
rate/admission result, proxy-trust result and rotating keyed IP hash only when operationally needed.
Never log Authorization/Cookie, query/transcript/audio, stream URL query, provider payload,
secret, raw IP/forwarded chain, account data or stack trace. Access logs and sampling must redact
the same fields. Retention/rotation/access control need deployment approval.

## Threat model and acceptance

| Threat | Required control | Signal |
| --- | --- | --- |
| extracted shared token/key | no client secret; server-only credentials; release review | `public_auth_header_present_total`, secret-scan gate |
| flooding/scraping | proxy-aware buckets, bounded cursor/results, no bulk export | rate-limited count and endpoint latency |
| STT abuse/reconnect | quota, admission cap, no queue, audio/deadline bounds | rejected upgrades, active sessions, audio seconds/cost |
| memory/CPU exhaustion | body/frame/aggregate limits and deadlines | rejected/timeout/size metrics |
| proxy spoofing | CIDR-only forwarded parsing, strict fallback | `client_ip_source_total` |
| privacy/authorization regression | DTO/log allowlists and fail-closed route tests | scrubber violations, denied-route metric |

Acceptance before MVP-001-B: proposed OpenAPI marks exactly the five operations anonymous;
all non-allowlisted families remain protected; every limit/error/metric/log rule is testable with
deterministic fakes; tests cover direct/proxied IP, malformed forwarded header, all limits,
timeouts, redaction and protected route denial without network, production secrets or live provider.

## Зафиксированные решения владельца

1. Paged `GET /v1/catalog/stations` входит в первый пользовательский UI; bulk export остаётся
   запрещённым.
2. Draft quotas/caps утверждены как консервативные стартовые значения для MVP-001-B: 100 global
   sessions, 60 s/2 MiB audio и IPv6-/64 policy. Их увеличение требует отдельного review.
3. Proxy CIDRs, выбор local/shared rate-limit store и retention keyed-IP hashes должны быть
   заданы в deployment configuration до публичного rollout; до этого implementation использует
   fail-closed direct-peer policy и не доверяет forwarded headers.
4. Streaming audio входит в MVP-001-B и остаётся public в пределах утверждённых quota/admission
   limits.
5. Новый public allowlist применяется только к явно перечисленным `/v1` operations. Existing
   `/api/v1` aliases и все неразрешённые `/v1` routes остаются protected до отдельного migration
   approval.
