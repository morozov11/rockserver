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
vol.Required = lambda value, **_: value
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
ha_entries = types.ModuleType("homeassistant.config_entries")
class FakeConfigFlow:
    def __init_subclass__(cls, **kwargs):
        pass
    async def async_set_unique_id(self, value):
        self.unique_id = value
    def _abort_if_unique_id_configured(self):
        pass
    def async_create_entry(self, **kwargs):
        return {"type": "create_entry", **kwargs}
    def async_show_form(self, **kwargs):
        return {"type": "form", **kwargs}
ha_entries.ConfigFlow = FakeConfigFlow
ha_entries.OptionsFlow = FakeConfigFlow
ha_selector = types.ModuleType("homeassistant.helpers.selector")
ha_selector.TextSelector = lambda config: config
ha_selector.TextSelectorConfig = lambda **kwargs: kwargs
ha_selector.TextSelectorType = types.SimpleNamespace(PASSWORD="password")
sys.modules.update({"homeassistant": ha, "homeassistant.core": ha_core,
                    "homeassistant.helpers": ha_helpers,
                    "homeassistant.helpers.config_validation": ha_cv,
                    "homeassistant.helpers.aiohttp_client": ha_aio,
                    "homeassistant.const": ha_const,
                    "homeassistant.config_entries": ha_entries,
                    "homeassistant.helpers.selector": ha_selector})

bridge = importlib.import_module("custom_components.rockserver_bridge")
normalize = importlib.import_module("custom_components.rockserver_bridge.normalization")
config_flow = importlib.import_module("custom_components.rockserver_bridge.config_flow")


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
        for local_sensors, local_weather in (({"sensor.room_temperature"}, "weather.home"), (None, None)):
            server = FakeRockServer()
            with self.assertRaises(EndSession):
                await asyncio.wait_for(
                    bridge._connected(ha, server, {}, asyncio.Event(),
                                      local_sensors, local_weather), 5)
            self.assertEqual([message["type"] for message in server.sent],
                             ["integration.hello", "integration.snapshot", "integration.weather"])
            self.assertAlmostEqual(server.sent[1]["sensors"][0]["value"], 20)
            self.assertEqual(server.sent[2]["weather"]["forecast"][0]["temperature"], 9)

    def test_missing_unknown_and_unsupported_sensor_values(self):
        self.assertEqual(normalize.sensor_reading("sensor.missing", None)["quality"], "unavailable")
        self.assertIsNone(normalize.sensor_reading("sensor.x", State("unknown"))["value"])
        unsupported = State("12", {"device_class": "pressure", "unit_of_measurement": "psi"})
        self.assertEqual(normalize.sensor_reading("sensor.x", unsupported)["quality"], "unknown")

    async def test_visual_config_flow_accepts_only_wss_and_bridge_key(self):
        flow = config_flow.RockServerConfigFlow()
        invalid = await flow.async_step_user({"server_url": "http://homeassist/api", "credential": "ha1.invalid"})
        self.assertEqual(invalid["errors"]["base"], "invalid_credentials")
        malformed = await flow.async_step_user({"server_url": "wss://[broken", "credential": "ha1." + "a" * 64})
        self.assertEqual(malformed["errors"]["base"], "invalid_credentials")
        valid = await flow.async_step_user({"server_url": config_flow.DEFAULT_URL, "credential": "ha1." + "a" * 64})
        self.assertEqual(valid["type"], "create_entry")


if __name__ == "__main__":
    unittest.main()
