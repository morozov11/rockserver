"""Deterministic fake HA and RockServer checks; no network or HA install required."""

import asyncio
from datetime import datetime, timezone
import importlib
from pathlib import Path
import sys
import types
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

# The integration's transport logic uses these HA symbols; fakes keep this test offline.
vol = types.ModuleType("voluptuous")
vol.ALLOW_EXTRA = object()
vol.Schema = lambda value, **_: value
vol.Required = lambda value: value
vol.All = lambda *values: values
vol.Length = lambda **values: values
sys.modules["voluptuous"] = vol

ha = types.ModuleType("homeassistant")
ha_core = types.ModuleType("homeassistant.core")
ha_core.HomeAssistant = object
ha_core.Event = object
ha_core.callback = lambda func: func
ha_helpers = types.ModuleType("homeassistant.helpers")
ha_cv = types.ModuleType("homeassistant.helpers.config_validation")
ha_cv.string = str
ha_cv.ensure_list = list
ha_aio = types.ModuleType("homeassistant.helpers.aiohttp_client")
ha_aio.async_get_clientsession = lambda _: None
ha_const = types.ModuleType("homeassistant.const")
ha_const.EVENT_HOMEASSISTANT_STOP = "stop"
sys.modules.update({"homeassistant": ha, "homeassistant.core": ha_core,
                    "homeassistant.helpers": ha_helpers,
                    "homeassistant.helpers.config_validation": ha_cv,
                    "homeassistant.helpers.aiohttp_client": ha_aio,
                    "homeassistant.const": ha_const})

bridge = importlib.import_module("custom_components.rockserver_bridge")
normalize = importlib.import_module("custom_components.rockserver_bridge.normalization")


class State:
    def __init__(self, state, attributes=None):
        self.state = state
        self.attributes = attributes or {}
        self.last_updated = datetime.now(timezone.utc)


class FakeStates:
    def __init__(self):
        self.values = {
            "sensor.room_temperature": State("68", {"device_class": "temperature", "unit_of_measurement": "°F"}),
            "weather.home": State("cloudy", {"temperature": 8, "temperature_unit": "°C"}),
        }

    def get(self, entity):
        return self.values.get(entity)


class FakeServices:
    async def async_call(self, domain, action, data, **kwargs):
        assert (domain, action, data) == ("weather", "get_forecasts", {"type": "daily"})
        assert kwargs["return_response"]
        return {"weather.home": {"forecast": [{"datetime": "2026-11-13T00:00:00Z", "temperature": 9}]}}


class FakeHA:
    def __init__(self):
        self.states = FakeStates()
        self.services = FakeServices()


class EndSession(Exception):
    pass


class FakeRockServer:
    def __init__(self):
        self.sent = []

    async def send_json(self, payload):
        self.sent.append(payload)

    async def receive_json(self):
        if len(self.sent) == 1:
            return {"type": "integration.welcome", "allowed_sensors": ["sensor.room_temperature"],
                    "weather_entity": "weather.home"}
        if len(self.sent) == 2:
            return {"type": "integration.ack", "revision": 1}
        raise EndSession


class BridgeTests(unittest.IsolatedAsyncioTestCase):
    async def test_fake_ha_resyncs_to_fake_server_after_reconnect(self):
        ha = FakeHA()
        for _ in range(2):
            server = FakeRockServer()
            with self.assertRaises(EndSession):
                await asyncio.wait_for(
                    bridge._connected(ha, server, {}, asyncio.Event(),
                                      {"sensor.room_temperature"}, "weather.home"), 5)
            self.assertEqual([message["type"] for message in server.sent],
                             ["integration.hello", "integration.snapshot", "integration.weather"])
            self.assertAlmostEqual(server.sent[1]["sensors"][0]["value"], 20)
            self.assertEqual(server.sent[2]["weather"]["forecast"][0]["temperature"], 9)

    def test_missing_unknown_and_unsupported_sensor_values(self):
        self.assertEqual(normalize.sensor_reading("sensor.missing", None)["quality"], "unavailable")
        self.assertIsNone(normalize.sensor_reading("sensor.x", State("unknown"))["value"])
        unsupported = State("12", {"device_class": "pressure", "unit_of_measurement": "psi"})
        self.assertEqual(normalize.sensor_reading("sensor.x", unsupported)["quality"], "unknown")


if __name__ == "__main__":
    unittest.main()
