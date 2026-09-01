# RM-007-A — контракт локальных персональных данных и миграции station ID

**Статус:** proposed design — ready for human approval.

**Дата:** 2026-08-25.
**Границы:** общий контракт для будущих offline-first реализаций RM-007-B (RockMobile) и
RM-007-C (RockCast). Этот документ не создаёт код, локальное избранное/историю, аккаунты,
синхронизацию, серверные endpoints или изменения OpenAPI.

## Нормативность и фактическая база

Слова **MUST**, **MUST NOT** и **SHOULD** задают предлагаемый обязательный контракт после
human approval. «Verified» означает наблюдаемое текущее поведение, а не уже реализованную
функцию RM-007.

| Область | Verified current behavior | Следствие для контракта |
|---|---|---|
| Canonical identity | The retired RM-004 proposal defined a globally unique, URL-independent `station.id`; IDs were not reused. Changing an operational-successor stream URL preserved station/stream ID, and a rename preserved station ID. | Персистентная ссылка на станцию — только canonical `stationId`, никогда URL, имя, alias или stream ID. |
| Lifecycle | Текущий RockServer `PinnedSharedCatalog` хранит `tombstones` и возвращает redirect только для `merged`, а `split` как ambiguity (`src/catalog/shared.rs`). В текущем baseline `2026.08.2` tombstones пусты. | Клиенты MUST применять lifecycle лишь из проверенного локального catalog snapshot; `merged` разрешается автоматически, `split` — никогда. |
| RockMobile | `Station.id` уже принимает canonical ID; `RockcastAssetStationSource` строит `legacyIds → station.id`; `UnavailableVoiceStationStore` атомарно мигрирует лишь этот существующий набор unavailable voice IDs и хранит максимум 100. См. `C:\repos\rockmobile\app\src\main\java\com\rockmobile\data\stations\StationSources.kt`, `...\settings\LegacyStationIdMigration.kt`, `...\settings\UnavailableVoiceStationStore.kt` и `LegacyStationIdMigrationTest.kt`. | Существующая миграция — доказательство legacy-map механизма, но не реализация favourites/history. |
| RockCast | Bundled schema-v1 JSON загружается с canonical `Station.id`; переходный `stations.txt` adapter создаёт URL-derived `legacy-<sha256-prefix>` IDs (`src/stations/catalog.rs`). Remote search/voice DTO сейчас создают отдельные URL-derived `rockserver-...` IDs (`src/rockserver.rs`, `src/voice/dto.rs`). | Ни `legacy-*`, ни `rockserver-*` не являются portable station IDs. До отдельного RM-004/RM-007 изменения они не могут автоматически стать favourite/history ссылкой без проверенного mapping. |
| Personal data | В проверенных исходниках RockMobile и RockCast нет локального favourites/history/profile store. | Все ниже — target model для RM-007-B/C, а не описание уже существующей функции. |

## Portable profile v1

Каждый клиент хранит один или более **локальных** profiles в собственном durable storage. Формат
должен быть сериализуемым без платформенных типов (UTF-8 JSON либо эквивалентная SQLite/Room
проекция) и переносимым без аккаунта. Внешний export/import не является частью RM-007-A, но
использует те же поля и значения.

```json
{
  "schemaVersion": 1,
  "profileId": "550e8400-e29b-41d4-a716-446655440000",
  "createdAt": "2026-08-25T10:15:30Z",
  "updatedAt": "2026-08-25T10:15:30Z",
  "favourites": [],
  "playbackHistory": [],
  "unresolvedReferences": [],
  "metadata": {}
}
```

All timestamps MUST be RFC 3339 UTC instants with millisecond precision when available. UUIDs are
random local identifiers, not account, device, advertising, or telemetry identifiers. `metadata`
is optional and contains only non-secret, client-private forward-compatible values; consumers MUST
ignore unknown keys. It MUST NOT contain URLs, credentials, tokens, raw audio, diagnostics, IP
addresses, account IDs, or inferred user attributes.

| Type | Required fields | Optional metadata and invariants |
|---|---|---|
| `LocalProfile` | `schemaVersion` (integer, `1`), `profileId` (UUID), `createdAt`, `updatedAt`, `favourites`, `playbackHistory` | `unresolvedReferences` records migration quarantine; `metadata` as above. An unsupported schemaVersion MUST leave the prior readable local data untouched and fail closed for write. |
| `Favourite` | `recordId` (UUID), `stationId` (canonical ID), `addedAt`, `updatedAt` | `metadata.lastKnownName` and `metadata.catalogVersion` MAY be retained only for an unavailable-station label. They are display snapshots, never resolution keys. One active favourite per resolved `stationId`. |
| `PlaybackHistoryEntry` | `recordId` (UUID), `stationId` (canonical ID), `startedAt`, `lastPlayedAt` | `endedAt` (nullable), `playDurationMs` (non-negative), `metadata.lastKnownName`, `metadata.catalogVersion`, and `metadata.source` (`bundled`, `extended`, or `remote`) MAY be stored. `source` is informational only; it MUST NOT select identity or cause networking. |
| `UnresolvedStationReference` | `referenceId` (UUID), `sourceKind` (`favourite` or `history`), `originalStationId`, `firstSeenAt`, `reason` (`split`, `removed`, `missing`, `legacy-unmapped`) | `candidateStationIds` (only for `split`), `lastKnownName`, and `catalogVersion` MAY be retained. It is local recovery data, not a playback target. |

`stationId` uses the RM-004 station-ID grammar and is normalized exactly as catalog IDs are; it
MUST NOT be case-folded, slugged, URL-normalized, or replaced by an alias. A profile MUST reject a
blank/invalid `stationId` rather than inventing a value. Stream selection remains catalog/playback
state and is deliberately outside personal-data identity.

## Ordering, deduplication, limits, and retention

| Data | Proposed deterministic rule |
|---|---|
| Favourites | Resolve lifecycle first, then deduplicate by resolved active `stationId`. Keep the oldest `addedAt` and its `recordId`; use the latest non-null optional display metadata and greatest `updatedAt`. Display in `addedAt` ascending, then `stationId` ascending. No manual ordering is specified by RM-007-A. Maximum: **500** active favourites; adding above the limit MUST fail visibly and preserve existing data. |
| History coalescing | A new play of the same resolved station updates the newest entry instead of appending when its `lastPlayedAt` is no more than **5 minutes** before the new `startedAt`; update `lastPlayedAt`, duration/end data, and `updatedAt`. Otherwise append a new entry. This suppresses retries/reconnect churn without merging distinct listening sessions. |
| History order | Descending `lastPlayedAt`, then descending `startedAt`, then `recordId` ascending. Equal timestamps therefore remain deterministic across clients. |
| History retention | Keep at most **500** entries and retain at most **90 days** from `lastPlayedAt`; on each write/migration delete the oldest entries exceeding either bound. Entries moved to `unresolvedReferences` retain their original timestamps but are not counted as playable history. |
| Non-syncable / non-portable data | Active queue, selected stream URL, relay/cast state, volume, transient playback errors, availability/probe data, catalog bytes/checksums, server search ranking, diagnostics/logs, tokens, credentials, account/device IDs, and telemetry are excluded. No telemetry is sent by default. |

## Stable-ID and lifecycle resolution

The resolution input MUST be the currently accepted local canonical snapshot (bundled baseline or
validated extended projection with compatible lifecycle data). A remote response that only has a
URL-derived client ID is not sufficient evidence for migration. The resolver MUST be deterministic,
idempotent, offline, and use this order:

1. If the ID is an active canonical `station.id`, retain it.
2. Otherwise, if a unique `legacyIds` entry equals the namespaced legacy ID, replace it with that
   active canonical ID.
3. Otherwise, follow an acyclic `merged` tombstone chain to its sole active target and replace it.
4. A `split` MUST NOT be resolved automatically. Move the source record to
   `unresolvedReferences` with all candidate IDs, removing it from active favourites/playable
   history only after the original record is safely represented there.
5. A `removed` tombstone, unknown canonical ID, malformed ID, or no locally available mapping is
   `missing`/`legacy-unmapped`: preserve it as an unresolved reference and do not play or silently
   delete it.

| Catalog event | Required result for a favourite/history reference |
|---|---|
| URL change under same station (including primary stream URL) | Keep `stationId` and record unchanged. The new catalog primary stream is chosen by playback; URL is never persisted as identity. |
| Primary stream role changes | Keep `stationId`; do not store a preferred `streamId` in profile v1. Playback follows the current catalog primary. |
| Rename/rebrand or aliases change | Keep `stationId`; refresh optional `lastKnownName` only when the client next writes the record. |
| `merged` tombstone | Rewrite to the sole final active target; coalesce duplicates using the table above, retain the earliest favourite `addedAt`, and record migration only in a local journal/backup. |
| `split` tombstone | Preserve an unresolved entry and require explicit user choice in a later UI. Never assign a favourite, history entry, last-played, or unavailable state to an arbitrary candidate. |
| `removed` tombstone | Preserve unresolved reference with reason `removed`; it is not a playback candidate. |
| Missing local station or lifecycle data | Treat as `missing`; never query the network merely to resolve it, and never generate a replacement from name/URL. |

## Local-first migration and rollback

Both clients MUST run migration before exposing personal data to UI/playback and before the first
new write. It MUST use only locally available snapshots and state. Network loss or RockServer
unavailability MUST not block startup or change a reference merely because a remote result differs.

1. Read the old store without deleting it; validate the profile/version and make a durable backup.
2. In one local transaction (or replace-on-success write), copy records to v1, apply the resolver,
   coalesce/dedupe, retain quarantine records, enforce retention, and write a migration journal with
   source/target schema version, catalogVersion, timestamp, and counts — never raw station URLs or
   secrets.
3. Mark the migration complete only after the new store is durably readable. Re-running it MUST
   produce the same profile; a crash leaves either the old complete store or the new complete store,
   never a partial replacement.
4. Keep the pre-migration backup until one successful restart after migration. A rollback restores
   the complete backup and does not discard original legacy IDs; it MUST NOT reverse a later user
   edit automatically.

### Client-specific mapping

| Concern | RockMobile | RockCast |
|---|---|---|
| Canonical local catalog | `RockcastAssetStationSource` exposes schema-v1 IDs and already obtains `legacyIds → station.id` from the bundled JSON. | `src/stations/catalog.rs` deserializes schema-v1 canonical `id` from bundled JSON. |
| Existing local ID state | Only `UnavailableVoiceStationStore` is verified; it maps raw `rockcast-<16 hex>` through `rockmobile:<id>` and preserves unmatched values. It is not favourites/history. | No verified favourites/history/profile state exists. The TXT adapter generates `legacy-<sha256-prefix>` locally; no approved mapping in the current canonical release maps that namespace. |
| RM-007 migration input | Reuse the verified `legacyIds` namespace mechanism, but give favourite/history their own transactional store and journal; do not reinterpret unavailable-voice storage as profile data. | Canonical JSON records migrate directly. Treat existing `legacy-*` and remote `rockserver-*` values as `legacy-unmapped` unless a future reviewed canonical release supplies a globally unique namespace mapping. |
| Safe rollback | Restore the separate profile backup; leave the existing unavailable-voice migration unchanged. | Restore the separate profile backup; do not mutate catalog override/TXT files. |

## Sync-ready boundary, ownership, and privacy

This v1 shape deliberately has stable `profileId` and per-record `recordId` plus creation/update
times, so a future RM-012 contract can define ownership, export, delete, conflict resolution, and
transport without changing station identity. That future work MUST separately approve account
binding, authentication, server retention, encryption, merge/delete semantics, and APIs. RM-007-A
creates none of them.

Before that approval, each profile is owned solely by the local client installation and may exist
without an account. It contains no secrets and sends no telemetry by default. The two clients must
not read each other's local storage, claim shared ownership, or upload/export automatically.

## Open decisions requiring human approval

1. **Favourite ordering:** v1 proposes insertion order and no manual reorder. Approve this, or
   approve a stable user-order field before UI work begins.
2. **Limits and retention:** approve 500 favourites, 500 history entries, 90 days, and a five-minute
   history coalescing window, or replace them with product limits.
3. **Split UX:** approve that split records remain quarantined until a user explicitly selects a
   successor; decide whether a later UI retains a visible retired-history section.
4. **RockCast legacy IDs:** current `legacy-*` and `rockserver-*` namespaces have no canonical
   mapping. Approve either a reviewed mapping in a future catalog release or the proposed
   preservation-as-unresolved policy.
5. **Lifecycle availability in the extended/mobile and remote paths:** approve the exact consumer
   artifact/API source that will carry tombstones. Until it exists, missing lifecycle input is
   intentionally unresolved; no endpoint is added here.
6. **Future sync semantics:** profile sharing, account binding, conflict resolution, deletion/export,
   server-side retention, and opt-in consent are deferred to RM-012-A and require a separate
   privacy/security approval.

## Acceptance trace

- One portable, versioned model defines `LocalProfile`, `Favourite`, and
  `PlaybackHistoryEntry`, timestamps, safe optional metadata, limits, ordering, and retention.
- RM-004 stable-ID, URL-change, rename, removed/merged/split, replacement-graph, and no-silent-split
  rules are preserved.
- RockMobile and RockCast migration/rollback rules are explicit and distinguish verified current
  state from planned RM-007 implementation.
- No server endpoint, authorization, sync, client code, catalog, OpenAPI, or database change is
  introduced by this contract.
