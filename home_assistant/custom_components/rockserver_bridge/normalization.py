"""Bounded, secret-free Home Assistant values for the RockServer wire contract."""

from datetime import datetime, timezone
import math


def stamp(value):
    """Convert HA datetimes to RFC 3339 UTC without inventing a source update."""
    if not isinstance(value, datetime) or value.tzinfo is None:
        return datetime.now(timezone.utc).isoformat()
    return value.astimezone(timezone.utc).isoformat()


def sensor_reading(entity_id, state):
    """Normalize a chosen sensor; bad or missing readings never become zero."""
    if state is None:
        return dict(entity_id=entity_id, display_name=entity_id.split(".", 1)[-1].replace("_", " "), value=None, unit=None, device_class=None,
                    quality="unavailable", observed_at=stamp(None))
    raw = state.state
    attrs = state.attributes
    unit = attrs.get("unit_of_measurement")
    kind = attrs.get("device_class")
    quality = "ok"
    value = None
    if raw in ("unknown", "unavailable"):
        quality = raw
    else:
        try:
            value = float(raw)
            if not math.isfinite(value):
                raise ValueError
        except (ValueError, TypeError):
            quality = "unknown"
        if quality == "ok":
            if kind == "temperature" and unit == "°F":
                value, unit = (value - 32) * 5 / 9, "°C"
            elif (kind, unit) not in (("temperature", "°C"), ("humidity", "%"),
                                      ("battery", "%"), ("carbon_dioxide", "ppm"),
                                      ("co2", "ppm"), ("pressure", "hPa"),
                                      ("illuminance", "lx"), ("power", "W"),
                                      ("energy", "kWh"), ("voltage", "V"), ("current", "A")):
                value, quality = None, "unknown"
    name = attrs.get("friendly_name")
    name = name[:128] if isinstance(name, str) and name else entity_id.split(".", 1)[-1].replace("_", " ")
    return dict(entity_id=entity_id, display_name=name, value=value, unit=unit if isinstance(unit, str) and len(unit) <= 16 else None,
                device_class=kind if isinstance(kind, str) and len(kind) <= 64 else None,
                quality=quality, observed_at=stamp(state.last_updated))


def number(value):
    """Keep finite numeric HA fields and preserve absent values as null."""
    if isinstance(value, bool):
        return None
    try:
        result = float(value)
        return result if math.isfinite(result) else None
    except (TypeError, ValueError):
        return None


def weather_reading(entity_id, state, forecast, forecast_observed_at=None):
    """Select current weather plus at most seven daily forecast periods."""
    if state is None or state.state in ("unknown", "unavailable"):
        return dict(entity_id=entity_id, condition=None, temperature=None,
                    temperature_unit=None, humidity=None, wind_speed=None,
                    wind_speed_unit=None, quality="unavailable", observed_at=stamp(state.last_updated if state else None),
                    forecast_observed_at=forecast_observed_at, forecast=[])
    attrs = state.attributes
    periods = []
    for item in forecast[:7]:
        when = item.get("datetime")
        if not isinstance(when, str) or len(when) > 64:
            continue
        try:
            parsed = datetime.fromisoformat(when.replace("Z", "+00:00"))
            if parsed.tzinfo is None:
                continue
        except ValueError:
            continue
        condition = item.get("condition")
        periods.append(dict(datetime=when, condition=condition[:64] if isinstance(condition, str) else None,
                            temperature=number(item.get("temperature")), templow=number(item.get("templow")),
                            precipitation=number(item.get("precipitation"))))
    temp_unit = attrs.get("temperature_unit")
    wind_unit = attrs.get("wind_speed_unit")
    return dict(entity_id=entity_id, condition=state.state[:64], temperature=number(attrs.get("temperature")),
                temperature_unit=temp_unit if temp_unit in ("°C", "°F") else None,
                humidity=number(attrs.get("humidity")), wind_speed=number(attrs.get("wind_speed")),
                wind_speed_unit=wind_unit if wind_unit in ("km/h", "m/s", "mph") else None,
                quality="ok", observed_at=stamp(state.last_updated),
                forecast_observed_at=forecast_observed_at, forecast=periods)
