//! Typed command and presentation payloads with their wire serialization rules.

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use serde_json::{Map, Value};

use super::{
    ActuatorAction, CommandId, DeviceId, Freshness, Quality, Timestamp, ValidationError,
    foundation::EXTENSION_NAME,
    validation::{bounded, namespaced},
};

/// Explicit device, entity, or surface command target.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandTarget {
    pub device_id: DeviceId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_id: Option<String>,
}
impl CommandTarget {
    /// Checks that an explicit target has at most one subtarget dimension.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.entity_id.is_some() && self.surface_id.is_some() {
            Err(ValidationError::InvalidPayload { field: "target" })
        } else {
            Ok(())
        }
    }
}
/// Typed presentation payloads.
#[derive(Clone, Debug, PartialEq)]
pub enum Presentation {
    Text {
        text: String,
    },
    NowPlaying {
        station_id: String,
        title: String,
        subtitle: Option<String>,
    },
    SensorGrid {
        title: String,
        items: Vec<SensorCard>,
    },
}
/// One normalized card in a sensor grid.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SensorCard {
    pub entity_id: String,
    pub label: String,
    pub value: Value,
    pub unit: Option<String>,
    pub quality: Quality,
    pub freshness: Freshness,
}
impl Presentation {
    /// Checks display bounds and explicit stale/unavailable semantics.
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::Text { text } => bounded(text, 1, 1024, "text"),
            Self::NowPlaying {
                station_id,
                title,
                subtitle,
            } => {
                bounded(station_id, 1, 128, "station_id")?;
                bounded(title, 1, 128, "title")?;
                if let Some(s) = subtitle {
                    bounded(s, 0, 256, "subtitle")?;
                }
                Ok(())
            }
            Self::SensorGrid { title, items } => {
                bounded(title, 1, 128, "title")?;
                if items.len() > 32 {
                    return Err(ValidationError::InvalidPayload { field: "items" });
                }
                for item in items {
                    bounded(&item.label, 1, 64, "label")?;
                    if item.quality == Quality::Unavailable && item.freshness == Freshness::Fresh {
                        return Err(ValidationError::InvalidPayload {
                            field: "quality/freshness",
                        });
                    }
                }
                Ok(())
            }
        }
    }
}

/// Known command payloads plus a lossless unexecutable forward extension.
#[derive(Clone, Debug, PartialEq)]
pub enum CommandBody {
    PlayStation {
        station_id: String,
    },
    Display {
        presentation: Presentation,
    },
    Playback {
        action: String,
    },
    /// A bounded media-volume operation supported by the v1 command vocabulary.
    Volume {
        command: VolumeCommand,
    },
    /// An allowlisted actuator action for one explicit entity target.
    Actuator {
        action: ActuatorAction,
        value: Option<f64>,
    },
    Unknown {
        name: String,
        payload: Map<String, Value>,
    },
}
/// Typed volume and mute operations accepted by the v1 command vocabulary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VolumeCommand {
    /// Sets the absolute output level between zero and one hundred.
    SetLevel { level: u8 },
    /// Adjusts the output level by a non-zero bounded delta.
    Change { delta: i8 },
    /// Enables or disables mute explicitly.
    SetMute { muted: bool },
}
/// A command with explicit target and optional bounded deadline.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceCommand {
    pub command_id: CommandId,
    pub target: CommandTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_at: Option<Timestamp>,
    pub body: CommandBody,
}
impl DeviceCommand {
    /// Checks syntactic v1 command invariants at a caller-supplied receipt time.
    pub fn validate_at(&self, received_at: &Timestamp) -> Result<(), ValidationError> {
        self.target.validate()?;
        if let Some(deadline) = &self.deadline_at {
            let seconds = (deadline.instant() - received_at.instant()).whole_seconds();
            if !(0..=30).contains(&seconds) {
                return Err(ValidationError::InvalidPayload {
                    field: "deadline_at",
                });
            }
        }
        match &self.body {
            CommandBody::Unknown { .. } => Ok(()),
            CommandBody::PlayStation { station_id } => bounded(station_id, 1, 128, "station_id"),
            CommandBody::Display { presentation } => presentation.validate(),
            CommandBody::Playback { action }
                if ["play", "pause", "stop", "next", "previous"].contains(&action.as_str()) =>
            {
                Ok(())
            }
            CommandBody::Volume { command } => match command {
                VolumeCommand::SetLevel { .. } | VolumeCommand::SetMute { .. } => Ok(()),
                VolumeCommand::Change { delta } if *delta != 0 => Ok(()),
                VolumeCommand::Change { .. } => {
                    Err(ValidationError::InvalidPayload { field: "command" })
                }
            },
            CommandBody::Actuator { action, value } => match (action, value) {
                (ActuatorAction::SetValue, Some(value)) if value.is_finite() => Ok(()),
                (ActuatorAction::SetValue, _) | (_, Some(_)) => {
                    Err(ValidationError::InvalidPayload { field: "command" })
                }
                _ => Ok(()),
            },
            _ => Err(ValidationError::InvalidPayload { field: "command" }),
        }
    }
    /// Returns an explicit unsupported outcome for opaque extensions.
    pub fn executable(&self) -> Result<(), ValidationError> {
        if matches!(self.body, CommandBody::Unknown { .. }) {
            Err(ValidationError::UnsupportedCommand)
        } else {
            Ok(())
        }
    }
}
impl Serialize for CommandBody {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let value = match self {
            Self::PlayStation { station_id } => {
                serde_json::json!({"name":"station.play_station","station_id":station_id})
            }
            Self::Playback { action } => serde_json::json!({"name":format!("playback.{action}")}),
            Self::Volume { command } => match command {
                VolumeCommand::SetLevel { level } => {
                    serde_json::json!({"name":"volume.set_volume","level":level})
                }
                VolumeCommand::Change { delta } => {
                    serde_json::json!({"name":"volume.change_volume","delta":delta})
                }
                VolumeCommand::SetMute { muted } => {
                    serde_json::json!({"name":"volume.set_mute","muted":muted})
                }
            },
            Self::Actuator { action, value } => match action {
                ActuatorAction::TurnOn | ActuatorAction::TurnOff => {
                    serde_json::json!({"name": action})
                }
                ActuatorAction::SetValue => serde_json::json!({"name": action, "value": value}),
            },
            Self::Display { presentation } => match presentation {
                Presentation::Text { text } => {
                    serde_json::json!({"name":"display.show_text","text":text})
                }
                Presentation::NowPlaying {
                    station_id,
                    title,
                    subtitle,
                } => {
                    serde_json::json!({"name":"display.show_view","view":"now_playing","station_id":station_id,"title":title,"subtitle":subtitle})
                }
                Presentation::SensorGrid { title, items } => {
                    serde_json::json!({"name":"display.show_view","view":"sensor_grid","title":title,"items":items})
                }
            },
            Self::Unknown { payload, .. } => Value::Object(payload.clone()),
        };
        value.serialize(s)
    }
}
impl<'de> Deserialize<'de> for CommandBody {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let m = Map::<String, Value>::deserialize(d)?;
        let name = m
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| D::Error::custom("missing command name"))?;
        let get = |k| {
            m.get(k)
                .cloned()
                .ok_or_else(|| D::Error::custom(format!("missing {k}")))
        };
        let body = match name {
            "station.play_station" => Self::PlayStation {
                station_id: serde_json::from_value(get("station_id")?).map_err(D::Error::custom)?,
            },
            "display.show_text" => Self::Display {
                presentation: Presentation::Text {
                    text: serde_json::from_value(get("text")?).map_err(D::Error::custom)?,
                },
            },
            "display.show_view" => match m.get("view").and_then(Value::as_str) {
                Some("sensor_grid") => Self::Display {
                    presentation: Presentation::SensorGrid {
                        title: serde_json::from_value(get("title")?).map_err(D::Error::custom)?,
                        items: serde_json::from_value(get("items")?).map_err(D::Error::custom)?,
                    },
                },
                Some("now_playing") => Self::Display {
                    presentation: Presentation::NowPlaying {
                        station_id: serde_json::from_value(get("station_id")?)
                            .map_err(D::Error::custom)?,
                        title: serde_json::from_value(get("title")?).map_err(D::Error::custom)?,
                        subtitle: m
                            .get("subtitle")
                            .cloned()
                            .map(serde_json::from_value)
                            .transpose()
                            .map_err(D::Error::custom)?,
                    },
                },
                _ => return Err(D::Error::custom("invalid display view")),
            },
            n if n.starts_with("playback.") => Self::Playback {
                action: n.trim_start_matches("playback.").into(),
            },
            "volume.set_volume" => Self::Volume {
                command: VolumeCommand::SetLevel {
                    level: serde_json::from_value(get("level")?).map_err(D::Error::custom)?,
                },
            },
            "volume.change_volume" => Self::Volume {
                command: VolumeCommand::Change {
                    delta: serde_json::from_value(get("delta")?).map_err(D::Error::custom)?,
                },
            },
            "volume.set_mute" => Self::Volume {
                command: VolumeCommand::SetMute {
                    muted: serde_json::from_value(get("muted")?).map_err(D::Error::custom)?,
                },
            },
            "entity.turn_on" => Self::Actuator {
                action: ActuatorAction::TurnOn,
                value: None,
            },
            "entity.turn_off" => Self::Actuator {
                action: ActuatorAction::TurnOff,
                value: None,
            },
            "entity.set_value" => Self::Actuator {
                action: ActuatorAction::SetValue,
                value: Some(serde_json::from_value(get("value")?).map_err(D::Error::custom)?),
            },
            _ if namespaced(name) => Self::Unknown {
                name: name.into(),
                payload: m,
            },
            _ => return Err(D::Error::custom(EXTENSION_NAME)),
        };
        Ok(body)
    }
}
