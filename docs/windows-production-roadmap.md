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

### 2. Integrate remote text search into RockCast

- Add a RockCast client for `POST /v1/search`.
- Map returned stations into the existing playback path.
- Preserve RockCast's local catalog as the offline/server-failure fallback.
- Add bounded timeouts, cancellation, and user-visible distinctions between no results, server failure, and network failure.
- Keep all RockCast UI and playback changes in the RockCast repository.

### 3. Add microphone capture to Windows RockCast

- Let the user select and test an input device.
- Implement start, stop, cancellation, retry, maximum duration, and maximum upload size.
- Show explicit recording, uploading, recognizing, searching, playing, and failure states.
- Release the audio device reliably after success, cancellation, errors, and application shutdown.

### 4. Add the RockServer voice command path

- RS-008 defines the stable JSON transcript contract. RS-009 adds canonical WebSocket `GET /api/v1/voice/stream` with a deprecated `/v1/voice/stream` alias.
- Stream bounded PCM16 mono chunks through a provider-neutral recognizer, return incremental/final transcripts, and resolve the final transcript through the existing `SearchService`.
- Pass the transcript through the existing query parser and `SearchService`; do not create a second search implementation.
- Return the transcript, selected station/stream data, propagated/generated request ID, and the standard structured error shape where appropriate.
- Keep provider credentials exclusively on RockServer.

### 5. Introduce production AI providers

- Add a production speech-to-text provider behind a testable trait.
- Add a production embedding provider and controlled backfill/migration procedure.
- Add an LLM query parser that receives only request text and locale, never the catalog.
- Add timeouts, bounded retries, safe logging, failure fallbacks, and circuit breaking where justified.
- Keep deterministic fakes and metadata fallback for ordinary tests and degraded operation.

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

RS-009 now stabilizes the server-side streaming wire protocol and provider seam. Next, implement the Yandex SpeechKit v3 adapter, then connect RockCast microphone capture with bounded cancellation, explicit no-result/server/network states, and local-catalog fallback. OpenAI remains a second adapter behind the same conformance tests.
