# Windows-first production roadmap

## Product decision

RockServer and the Windows RockCast application will be completed and validated as one production workflow before any ESP32 client work begins. Voice capture belongs to RockCast; speech recognition, query interpretation, catalog search, and ranking remain server responsibilities. ESP32 is a future client of the stabilized API and is outside the current roadmap.

Target user flow:

```text
RockCast microphone
  -> RockServer voice endpoint
  -> speech-to-text
  -> structured query interpretation
  -> existing SearchService
  -> station and stream result
  -> existing RockCast playback path
```

## Delivery order

### 1. Verify the existing RS-007 foundation

- Run migrations and the ignored PostgreSQL/pgvector integration test against the disposable local `rockserver` database.
- Run deterministic embedding backfill and verify metadata-only, hybrid, incompatible-provenance, and provider-failure fallback behavior.
- Keep database credentials in environment variables; never store them in Git, logs, or documentation.
- The local development database is disposable and may be reset when a clean migration/backfill run is required.

### 2. Integrate remote text search into RockCast — complete

- RockCast calls the protected search endpoint and maps returned stations into the existing playback path.
- Its local catalog remains the fallback when RockServer search is unavailable or fails.
- Bounded timeout/cancellation behavior and a deterministic client/server integration suite remain to be added.

### 3. Add microphone capture to Windows RockCast — MVP complete

- RockCast records PCM16 mono from the default input device until release or the 60-second limit, then sends it to the canonical WebSocket endpoint.
- It handles configured-token, invalid-token, unavailable-server, and generic voice-result states and can play the selected result.
- Input-device selection/test, explicit upload/recognition states, cancellation after upload starts, retry policy, and deterministic end-to-end coverage remain.

### 4. Add the RockServer voice command path — MVP complete

- RS-008/RS-009 provide the JSON transcript contract and canonical WebSocket `GET /api/v1/voice/stream`; `/v1/voice/stream` remains a deprecated alias.
- The bounded PCM16 session is resolved through `SearchService`, and provider credentials remain exclusively on RockServer.
- When `YANDEX_AI_API_KEY` is configured, RockServer exposes both `YandexSpeechKitRecognizer` (buffered v1 after `commit`) and `YandexSpeechKitStreamingRecognizer` (v3 upstream gRPC). RockCast selects the mode for each voice session; buffered v1 stays the compatibility default.

### 5. Introduce production AI providers — partially complete

- Yandex SpeechKit provides bounded commit-time and selectable upstream-streaming recognition; local ONNX E5 supplies production-like embeddings; Yandex AI Studio parses structured request intent without receiving the catalog.
- Deterministic fakes and metadata fallback remain the ordinary offline/degraded path.
- Provider retries/circuit breaking, a second STT provider, and live end-to-end streaming coverage remain.

### 6. Complete Windows end-to-end behavior

- Validate `press microphone -> speak -> recognize -> search -> play` in Russian and English.
- Cover empty results, silence, unsupported audio, slow network, server loss, STT/provider failure, repeated commands, cancellation, and playback failure.
- Preserve usable text search and local-catalog playback when voice or server dependencies fail.

### 7. Production hardening

- Add authentication, rate limiting, request-size limits, stream probing, metrics, traces, retention-safe logs, and operational alerts.
- Add unit, contract, PostgreSQL integration, provider-fake, and Windows end-to-end tests.
- Package and update RockCast for Windows; document RockServer deployment, configuration, backup, rollback, and model/backfill procedures.
- Run a limited Windows beta, fix the highest-impact recognition/search/playback failures, and define release acceptance metrics.

### 8. Future backlog: ESP32

ESP32 work begins only after the Windows client and RockServer meet production acceptance criteria. It should reuse the stabilized public API and provider pipeline rather than introduce a parallel server architecture.

## Immediate next task

The bounded end-to-end MVP exists: RockCast captures from the default microphone, RockServer can select Yandex SpeechKit, and the result returns through the existing search/playback path. Next, add deterministic client/server end-to-end tests, input-device selection, cancellation across recording/upload/recognition, explicit no-result/server/network states, and retention-safe logging. OpenAI or another recognizer remains a second adapter behind shared conformance tests.
