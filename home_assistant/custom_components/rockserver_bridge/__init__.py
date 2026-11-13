"""Push selected local HA observations to an owner-bound RockServer integration."""

import asyncio
import logging
import random
import time
from datetime import datetime, timezone
from urllib.parse import urlsplit

from aiohttp import WSServerHandshakeError
import voluptuous as vol
from homeassistant.core import HomeAssistant, Event, callback
from homeassistant.helpers import config_validation as cv
from homeassistant.helpers.aiohttp_client import async_get_clientsession
from homeassistant.const import EVENT_HOMEASSISTANT_STOP

from .normalization import sensor_reading, weather_reading

DOMAIN = "rockserver_bridge"
_LOGGER = logging.getLogger(__name__)

CONFIG_SCHEMA = vol.Schema({
    vol.Required(DOMAIN): vol.Schema({
        vol.Required("server_url"): cv.string,
        vol.Required("credential"): cv.string,
        vol.Required("sensors"): vol.All(cv.ensure_list, [cv.string], vol.Length(max=128)),
        vol.Required("weather_entity"): cv.string,
    })
}, extra=vol.ALLOW_EXTRA)


async def async_setup(hass: HomeAssistant, config: dict) -> bool:
    """Start one bounded outbound bridge task after validating local configuration."""
    options = config.get(DOMAIN)
    if options is None:
        return True
    url = urlsplit(options["server_url"])
    if url.scheme != "wss" or not url.hostname or url.username or url.password or url.query or url.fragment:
        _LOGGER.error("RockServer bridge requires a plain WSS endpoint URL")
        return False
    sensors = set(options["sensors"])
    weather_entity = options["weather_entity"]
    if (len(sensors) != len(options["sensors"]) or not all(s.startswith("sensor.") for s in sensors)
            or not weather_entity.startswith("weather.")):
        _LOGGER.error("RockServer bridge entity selection is invalid")
        return False
    task = hass.async_create_background_task(
        _run(hass, options["server_url"], options["credential"], sensors, weather_entity),
        "RockServer Home Assistant bridge",
    )
    hass.bus.async_listen_once(EVENT_HOMEASSISTANT_STOP, lambda _: task.cancel())
    return True


async def async_setup_entry(hass, entry) -> bool:
    """Start an outbound bridge configured entirely through the HA integration UI."""
    options = {**entry.data, **entry.options}
    task = hass.async_create_background_task(
        _run(hass, options["server_url"], options["credential"], None, None),
        "RockServer Home Assistant bridge",
    )
    hass.data.setdefault(DOMAIN, {})[entry.entry_id] = task
    entry.async_on_unload(entry.add_update_listener(_reload_entry))
    return True


async def _reload_entry(hass, entry):
    """Apply a visually entered replacement key without restarting HA."""
    await hass.config_entries.async_reload(entry.entry_id)


async def async_unload_entry(hass, entry) -> bool:
    """Stop this entry's outbound connection and its event subscription."""
    task = hass.data.get(DOMAIN, {}).pop(entry.entry_id, None)
    if task is not None:
        task.cancel()
    return True


async def _run(hass, server_url, credential, local_sensors, local_weather):
    """Reconnect with bounded jitter and resync from HA state on every connection."""
    pending = {}
    changed = asyncio.Event()

    @callback
    def state_changed(event: Event):
        entity_id = event.data.get("entity_id")
        relevant = (entity_id in local_sensors or entity_id == local_weather) if local_sensors is not None else (
            isinstance(entity_id, str) and (entity_id.startswith("sensor.") or entity_id.startswith("weather.")))
        if relevant:
            pending[entity_id] = event.data.get("new_state")
            changed.set()

    unsubscribe = hass.bus.async_listen("state_changed", state_changed)
    session = async_get_clientsession(hass)
    delay = 1.0
    try:
        while True:
            try:
                async with session.ws_connect(server_url, headers={"Authorization": f"Bearer {credential}"},
                                              heartbeat=30, max_msg_size=65536, timeout=15) as ws:
                    await _connected(hass, ws, pending, changed, local_sensors, local_weather)
                delay = 1.0
            except asyncio.CancelledError:
                raise
            except WSServerHandshakeError as error:
                _LOGGER.warning("RockServer bridge connection refused (HTTP %s)", error.status)
                delay = 300.0 if error.status == 401 else min(delay * 2, 60.0)
            except Exception:
                _LOGGER.warning("RockServer bridge disconnected; retrying")
                delay = min(delay * 2, 60.0)
            await asyncio.sleep(delay + random.uniform(0, min(delay / 4, 5)))
    finally:
        unsubscribe()


async def _exchange(ws, payload, expected):
    """Send one bounded frame and require the matching server response."""
    await ws.send_json(payload)
    reply = await asyncio.wait_for(ws.receive_json(), 15)
    if reply.get("type") != expected:
        raise RuntimeError("unexpected RockServer bridge reply")
    if expected == "integration.ack" and reply.get("revision") != payload["revision"]:
        raise RuntimeError("RockServer bridge revision mismatch")
    return reply


async def _connected(hass, ws, pending, changed, local_sensors, local_weather):
    """Send a full snapshot, then coalesced changes, weather and heartbeat."""
    welcome = await _exchange(ws, {"type": "integration.hello", "protocol_version": 1}, "integration.welcome")
    sensors = set(welcome["allowed_sensors"]) if local_sensors is None else local_sensors.intersection(welcome["allowed_sensors"])
    weather_entity = welcome["weather_entity"] if local_weather is None else (local_weather if welcome["weather_entity"] == local_weather else None)
    revision = 1
    pending.clear()
    changed.clear()
    await _exchange(ws, {"type": "integration.snapshot", "revision": revision,
                         "sensors": [sensor_reading(entity_id, hass.states.get(entity_id))
                                     for entity_id in sorted(sensors)]}, "integration.ack")
    last_forecast = None
    forecast = []
    forecast_observed_at = None
    last_sent = time.monotonic()
    while True:
        weather_changed = weather_entity in pending
        due = last_forecast is None or time.monotonic() - last_forecast >= 900
        if weather_entity and (weather_changed or due):
            if due:
                try:
                    result = await hass.services.async_call("weather", "get_forecasts", {"type": "daily"},
                                                            target={"entity_id": weather_entity},
                                                            blocking=True, return_response=True)
                    fresh = result.get(weather_entity, {}).get("forecast", [])
                    if isinstance(fresh, list):
                        forecast = fresh
                        forecast_observed_at = datetime.now(timezone.utc).isoformat()
                except asyncio.CancelledError:
                    raise
                except Exception:
                    _LOGGER.warning("RockServer bridge forecast refresh failed")
                last_forecast = time.monotonic()
            revision += 1
            await asyncio.sleep(max(0, last_sent + 1.1 - time.monotonic()))
            await _exchange(ws, {"type": "integration.weather", "revision": revision,
                                 "weather": weather_reading(weather_entity, hass.states.get(weather_entity), forecast, forecast_observed_at)},
                            "integration.ack")
            last_sent = time.monotonic()
        changes = {entity: state for entity, state in pending.items() if entity in sensors}
        pending.clear()
        changed.clear()
        if changes:
            revision += 1
            await asyncio.sleep(max(0, last_sent + 1.1 - time.monotonic()))
            await _exchange(ws, {"type": "integration.state", "revision": revision,
                                 "sensors": [sensor_reading(entity, state) for entity, state in sorted(changes.items())]},
                            "integration.ack")
            last_sent = time.monotonic()
        try:
            await asyncio.wait_for(changed.wait(), timeout=30)
        except asyncio.TimeoutError:
            await _exchange(ws, {"type": "integration.heartbeat"}, "integration.heartbeat")
