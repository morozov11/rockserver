# Home Assistant custom integration: operator setup

This bridge runs inside the owner's Home Assistant process. It reads only explicitly selected
`sensor.*` entities and one `weather.*` entity, then opens an outbound TLS WebSocket connection to
RockServer. The home router needs no inbound port, public HA address or HA long-lived API token.
The custom integration has HA process privileges, so install it only from reviewed source.

## Server provisioning

1. Find the RockServer owner's `users.id` through the protected account administration path.
2. From a trusted operator shell with `DATABASE_URL` set, run:

   ```text
   cargo run --bin ha_bridge_admin -- provision USER_UUID weather.home sensor.room_temperature sensor.room_humidity
   ```

   Replace every entity ID with the selected actual HA IDs. The command accepts up to 128
   sensors, requires exactly one weather entity and replaces the entire approved allowlist. It
   prints a new `ha1.` credential once. Capture it into a protected HA secret; never put it in
   Git, a URL, or a log. Provisioning an existing owner clears its former observations and
   invalidates its old connection. On the production host, run the deployed binary with the
   protected Compose environment:

   ```text
   cd /opt/rockserver
   sudo env ROCKSERVER_IMAGE=rockserver:sha-RELEASE_SHA ROCKSERVER_CADDY_IMAGE=rockserver-caddy:sha-RELEASE_SHA docker compose --project-name rockserver --env-file release.env -f compose.yaml -f compose.production.yaml run --rm --no-deps --entrypoint /usr/local/bin/ha_bridge_admin rockserver provision USER_UUID weather.home sensor.room_temperature sensor.room_humidity
   ```

   Capture the printed credential privately; this output is sensitive.
3. To rotate the credential without changing the allowlist:

   ```text
   cargo run --bin ha_bridge_admin -- rotate USER_UUID
   ```

   Update the HA secret and restart HA. The prior socket loses its epoch.
4. To revoke access:

   ```text
   cargo run --bin ha_bridge_admin -- revoke USER_UUID
   ```

   Revocation rejects new connections and writes. Owner weather reads no longer return the row.

`ha_bridge_admin` is an operator command, not a public provisioning API. It runs SQL migrations
through the same migrations directory as the service. Keep a restorable PostgreSQL backup before
applying production migrations.

## Home Assistant configuration

Copy `home_assistant/custom_components/rockserver_bridge` into the HA config directory as
`custom_components/rockserver_bridge`. In the HA config's `secrets.yaml`, add:

```yaml
rockserver_bridge_credential: "ha1.REPLACE_WITH_PROVISIONED_CREDENTIAL"
```

In `configuration.yaml`, add:

```yaml
rockserver_bridge:
  server_url: "wss://rockplatform.win/api/v1/integrations/home-assistant/connect"
  credential: !secret rockserver_bridge_credential
  sensors:
    - sensor.room_temperature
    - sensor.room_humidity
  weather_entity: weather.home
```

The local list and server-approved list are intersected. The selected weather entity must match
the server setting exactly. Restart Home Assistant after installation or secret changes. Its
normal outbound HTTPS certificate validation remains enabled. Weather forecast refresh uses
`weather.get_forecasts` with daily periods at most once every 15 minutes. Current weather is read
from the selected entity state. Unsupported sensor units, unknown/unavailable states and deleted
entities are sent without invented numeric values. A connection loss causes a complete fresh
snapshot after reconnect, with bounded exponential backoff and jitter.

An authenticated first-party browser session can read
`GET /api/v1/browser/home-assistant/weather`. `status` describes current conditions and
`forecast_status` describes the forecast separately. Both can be `fresh`, `stale`, or
`unavailable`; `online` becomes false after 60 seconds without accepted traffic. Clients must
inspect these fields before displaying a value as current. The response uses a stable
integration-scoped public entity ID rather than the HA native entity ID.

The account's **Devices → Home Assistant** card reads `GET /api/v1/browser/home-assistant`.
It shows only the server-approved sensors and weather, labels stale or unavailable values, and
offers a refresh button. The card shows **Not connected** until an operator provisions the
credential and the HA component starts sending observations.

## Pilot checks still required

No custom integration has been installed on `alex-center`. Before the first real connection, verify the HA config mount,
selected entity IDs and their unit attributes, daily forecast support, WSS reachability and
certificate, restart/reconnect behaviour, credential rotation/revocation, and stale results after
disconnect. The exact HA installation type was inferred from a read-only SSH process listing and
is not yet independently confirmed.
