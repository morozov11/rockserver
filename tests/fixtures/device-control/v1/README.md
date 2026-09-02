# Device-control v1 golden fixtures

These are canonical raw JSON fixtures for protocol v1. RockServer, RockCast,
RockMobile, and firmware contract tests should consume these files directly;
do not copy them into client repositories. Timestamps and UUIDs are deterministic
test data, not credentials or real identifiers. Files are raw protocol messages
unless their schema below is an explicitly source-neutral projection component.

| Fixture | Direction | OpenAPI component schema | Validity / expected outcome | Flow |
| --- | --- | --- | --- | --- |
| `hello-client.json` | client → server | `ProtocolHelloMessage` | valid | v1 negotiation starts. |
| `welcome-server.json` | server → client | `ProtocolWelcomeMessage` | valid | server selects v1 and publishes fixed limits. |
| `rockcast-register-client.json` | client → server | `DeviceRegisterMessage` | valid | RockCast declares only player/playback, station, volume, Chromecast and relay support. |
| `rockcast-registered-server.json` | server → client | `DeviceRegisteredMessage` | valid | server-derived connection, identity and policy after registration. |
| `esp32-register-client.json` | client → server | `DeviceRegisterMessage` | valid | ESP32 declares player, display, voice and sensor roles. |
| `esp32-manifest-client.json` | client → server | `DeviceManifestMessage` | valid | revision 2 full replacement manifest. |
| `esp32-state-full-client.json` | client → server | `DeviceStateFullMessage` | valid | initial state revision 7. |
| `esp32-state-delta-client.json` | client → server | `DeviceStateDeltaMessage` | valid | delta 7 → 8. |
| `esp32-temperature-state-client.json` | client → server | `EntityStateMessage` | valid | temperature telemetry with observation and freshness deadline. |
| `esp32-humidity-state-client.json` | client → server | `EntityStateMessage` | valid | humidity telemetry with its own entity revision. |
| `directory-snapshot-server.json` | server → controller | `DirectorySnapshotMessage` | valid | RockMobile-readable projection of registered RockCast and ESP32. |
| `ha-normalized-entity-directory-entry.json` | HTTP/projection | `EntityDirectoryEntry` | valid | source-neutral allowlisted Home Assistant entity metadata; it deliberately carries no paired device or provider-native identity. |
| `ha-normalized-entity-state.json` | HTTP/projection | `EntityStateSnapshot` | valid | source-neutral normalized allowlisted Home Assistant state. |
| `display-sensor-grid-command-server.json` | server → ESP32 | `DeviceCommandMessage` | valid | explicit `display.main` sensor-grid presentation. |
| `station-command-server.json` | server → RockCast | `DeviceCommandMessage` | valid | explicit catalog station command begins lifecycle. |
| `station-command-received-server.json` | server → controller | `CommandReceivedMessage` | valid | receipt for the station command. |
| `station-command-accepted-rockcast.json` | RockCast → server | `CommandAcceptedMessage` | valid | target started processing. |
| `station-command-succeeded-rockcast.json` | RockCast → server | `CommandResultMessage` | valid | sole terminal successful result. |
| `station-command-failed-rockcast.json` | RockCast → server | `CommandResultMessage` | valid, semantic `command_timeout` | structured terminal failure. |
| `unknown-capability.json` | client → server | `DeviceCapability` | valid, hidden by older controllers | namespaced forward extension. |
| `unknown-command-client.json` | client → server | `DeviceCommandMessage` | valid, semantic `unsupported_command` | envelope is accepted; execution is not. |
| `unknown-command-error-server.json` | server → client | `ProtocolErrorMessage` | valid, semantic `unsupported_command` | structured reply to unknown command. |
| `stale-manifest-client.json` | client → server | `DeviceManifestMessage` | valid, semantic `stale_revision` | lower manifest revision is rejected. |
| `stale-manifest-error-server.json` | server → client | `ProtocolErrorMessage` | valid, semantic `stale_revision` | structured stale-revision reply. |
| `stale-device-state-client.json` | client → server | `DeviceStateFullMessage` | valid, semantic `stale_revision` | lower device state revision is rejected. |
| `stale-device-state-error-server.json` | server → client | `ProtocolErrorMessage` | valid, semantic `stale_revision` | structured stale device-state reply. |
| `stale-entity-state-client.json` | client → server | `EntityStateMessage` | valid, semantic `stale_revision` | lower entity revision is rejected. |
| `stale-entity-state-error-server.json` | server → client | `ProtocolErrorMessage` | valid, semantic `stale_revision` | structured stale entity-state reply. |
| `invalid-sensor-unit-value-client.json` | client → server | `EntityStateMessage` | valid, semantic `invalid_payload` | numeric unit paired with nonnumeric value is rejected by semantic normalization. |
| `invalid-sensor-unit-value-error-server.json` | server → client | `ProtocolErrorMessage` | valid, semantic `invalid_payload` | structured invalid telemetry reply. |
| `duplicate-command-received-server.json` | server → controller | `CommandReceivedMessage` | valid, semantic replay/no re-execution | same payload and command id within 86,400 seconds replays stored lifecycle. |
| `duplicate-command-result-server.json` | server → controller | `CommandResultMessage` | valid, semantic replay/no re-execution | stored terminal result is replayed. |
| `offline-target-command-client.json` | client → server | `DeviceCommandMessage` | valid, semantic `target_offline` | target is not queued while offline. |
| `offline-target-error-server.json` | server → client | `ProtocolErrorMessage` | valid, semantic `target_offline` | structured offline reply. |
| `missing-surface-command-server.json` | server → ESP32 | `DeviceCommandMessage` | valid, semantic `capability_not_supported` | undeclared display surface is rejected. |
| `missing-surface-error-server.json` | server → controller | `ProtocolErrorMessage` | valid, semantic `capability_not_supported` | structured missing-surface reply. |
| `invalid-frame-missing-message-id.json` | client → server | `ControlMessageEnvelope` | intentionally schema-invalid | required `message_id` is missing. |

Semantic outcomes are intentionally represented by schema-valid request/event
messages and their structured schema-valid reply where one is defined. JSON
Schema cannot infer live revisions, idempotency history, availability, or unit
dimensions, so the contract test makes those limited cross-fixture assertions
explicit instead of pretending they are schema keywords.
