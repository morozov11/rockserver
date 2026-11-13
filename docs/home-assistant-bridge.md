# Home Assistant custom integration: account setup

This bridge runs inside the owner's Home Assistant process. It reads only explicitly selected
`sensor.*` entities and one `weather.*` entity, then opens an outbound TLS WebSocket connection to
RockServer. The home router needs no inbound port, public HA address or HA long-lived API token.
The custom integration has HA process privileges, so install it only from reviewed source.

## Connect from the RockServer account

1. Sign in to RockServer and open **Devices → Home Assistant**. In Home Assistant's
   **Developer Tools → States**, copy the `sensor.*` IDs you want to publish and one
   `weather.*` ID. Enter them in the RockServer card and select **Get key**. The browser request
   uses the current account session and CSRF proof, so no account UUID or server shell is needed.
2. Copy the newly displayed `ha1.` key. It is shown only in that browser view; RockServer stores
   its hash. Keep the tab open until the next step is complete. Reissuing the key immediately
   invalidates the previous key and clears old observations.
3. Install the `home_assistant/custom_components/rockserver_bridge` directory once under the HA
   configuration directory's `custom_components/` folder, then restart HA. In the HA browser UI,
   open **Settings → Devices & services → Add integration → RockServer Bridge**. Paste the key
   into the password field and leave the WSS address at the displayed RockServer value. No HA
   YAML or long-lived HA API token is needed for this UI setup.
4. Return to the RockServer card and refresh. It will show connection status, approved sensor
   readings, current weather and a short forecast. To change sensors or the weather entity, edit
   the IDs and select **Save list**. The existing key remains valid; the bridge reconnects and
   sends a fresh snapshot. Old observations are cleared during the change. **Replace key** is a
   separate action and requires pasting the new key into HA's **Configure** options.
   **Disconnect** in RockServer revokes the key and hides its observations; remove the HA
   integration as well if you no longer want the local task to retry.

RockServer cannot install a custom component into a Home Assistant instance reachable only on
the home network. That one local installation remains the owner's step; all subsequent account
authorization, selection, key replacement and revocation are in the RockServer cabinet. The HA
browser is used only to enter the key locally. No inbound port or public HA address is required.

## Operator fallback

1. Find the RockServer owner's `users.id` through the protected account administration path.
2. From a trusted operator shell with `DATABASE_URL` set, run:

   ```text
   cargo run --bin ha_bridge_admin -- provision USER_UUID weather.home sensor.room_temperature sensor.room_humidity
   ```

   Replace every entity ID with the selected actual HA IDs. The command accepts up to 128
   sensors, requires exactly one weather entity and replaces the entire approved allowlist. It
   prints a new `ha1.` credential once. Capture it privately; never put it in
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

   Update the key through HA's integration options. The prior socket loses its epoch.
4. To revoke access:

   ```text
   cargo run --bin ha_bridge_admin -- revoke USER_UUID
   ```

   Revocation rejects new connections and writes. Owner weather reads no longer return the row.

`ha_bridge_admin` is an operator command, not a public provisioning API. It runs SQL migrations
through the same migrations directory as the service. Keep a restorable PostgreSQL backup before
applying production migrations.

## Legacy YAML configuration

Existing YAML installations remain supported. In the HA config's `secrets.yaml`, add:

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
the server setting exactly; a legacy YAML weather change therefore also needs a YAML edit and HA
restart. For the HA UI flow, the component receives the approved list
from the RockServer welcome frame. Its normal outbound HTTPS certificate validation remains
enabled. Weather forecast refresh uses
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
offers a refresh button and account-owned key/allowlist controls. The card shows **Not connected**
until the account issues a key and the HA component starts sending observations.

## Pilot checks still required

The custom integration is installed on `alex-center`, and the HA UI config entry now loads.
The bridge reached the forecast step after the server acknowledged its initial snapshot. Live
cabinet readings, daily forecast support for the chosen entity, restart/reconnect behaviour,
credential rotation/revocation, and stale results after disconnect still need verification.
