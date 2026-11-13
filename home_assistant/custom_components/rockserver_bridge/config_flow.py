"""Visual setup for the outbound RockServer Home Assistant bridge."""

from urllib.parse import urlsplit

import voluptuous as vol
from homeassistant.config_entries import ConfigFlow, OptionsFlow
from homeassistant.helpers.selector import TextSelector, TextSelectorConfig, TextSelectorType

from . import DOMAIN

DEFAULT_URL = "wss://rockplatform.win/api/v1/integrations/home-assistant/connect"


def _valid(data):
    """Accept only a TLS bridge URL and a correctly shaped bearer."""
    try:
        url = urlsplit(data["server_url"])
        hostname = url.hostname
    except ValueError:
        return False
    credential = data["credential"]
    return (url.scheme == "wss" and bool(hostname) and not (url.username or url.password or url.query or url.fragment)
            and url.path == "/api/v1/integrations/home-assistant/connect"
            and credential.startswith("ha1.") and len(credential) == 68
            and all(char in "0123456789abcdef" for char in credential[4:]))


def _schema(default_url=DEFAULT_URL):
    return vol.Schema({
        vol.Required("server_url", default=default_url): str,
        vol.Required("credential"): TextSelector(TextSelectorConfig(type=TextSelectorType.PASSWORD)),
    })


class RockServerConfigFlow(ConfigFlow, domain=DOMAIN):
    """Create one account-owned bridge connection from a pasted cabinet key."""

    VERSION = 1

    async def async_step_user(self, user_input=None):
        """Show the visual form and accept a newly issued RockServer key."""
        errors = {}
        if user_input is not None:
            if not _valid(user_input):
                errors["base"] = "invalid_credentials"
            else:
                await self.async_set_unique_id("rockserver_bridge")
                self._abort_if_unique_id_configured()
                return self.async_create_entry(title="RockServer", data=user_input)
        return self.async_show_form(step_id="user", data_schema=_schema(), errors=errors)

    @staticmethod
    def async_get_options_flow(config_entry):
        """Allow the HA owner to paste a rotated key in the HA UI."""
        return RockServerOptionsFlow()


class RockServerOptionsFlow(OptionsFlow):
    """Update the stored credential and reload the outbound bridge."""

    async def async_step_init(self, user_input=None):
        """Show the replacement-key form without redisplaying the saved secret."""
        errors = {}
        if user_input is not None:
            if not _valid(user_input):
                errors["base"] = "invalid_credentials"
            else:
                return self.async_create_entry(title="", data=user_input)
        url = self.config_entry.options.get("server_url", self.config_entry.data["server_url"])
        return self.async_show_form(step_id="init", data_schema=_schema(url), errors=errors)
