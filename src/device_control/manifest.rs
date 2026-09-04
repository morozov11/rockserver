//! Static device declarations: roles, capabilities, entities, surfaces, and manifests.

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use serde_json::{Map, Value};

use super::{
    DeviceId, ValidationError,
    foundation::EXTENSION_NAME,
    validation::{bounded, list, namespaced, numbers, unique, valid_entity_id},
};

/// Functional device role; it is intentionally not an authorization scope.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceRole {
    Controller,
    Player,
    DisplaySurface,
    VoiceEndpoint,
    SensorSource,
    Actuator,
    IntegrationAdapter,
}

/// Server-computed authorization scope, separate from roles and capabilities.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum DeviceControlScope {
    #[serde(rename = "device.directory.read")]
    DirectoryRead,
    #[serde(rename = "device.presence.read")]
    PresenceRead,
    #[serde(rename = "entity.state.read")]
    EntityStateRead,
    #[serde(rename = "media.control")]
    MediaControl,
    #[serde(rename = "display.control")]
    DisplayControl,
    #[serde(rename = "actuator.control")]
    ActuatorControl,
}

/// Public device record without authentication, ownership, or provider credentials.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Device {
    pub device_id: DeviceId,
    pub display_name: String,
    pub device_type: String,
}

/// Public normalized entity metadata.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entity {
    pub entity_id: String,
    pub domain: EntityDomain,
    pub device_class: String,
    #[serde(rename = "display_name")]
    pub label: String,
    pub readable: bool,
    pub controllable: bool,
    pub unit: Option<String>,
    pub stale_after_seconds: u32,
    pub allowed_commands: Vec<ActuatorAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
}
impl Entity {
    /// Checks metadata and numeric constraints that do not require runtime policy.
    pub fn validate(&self) -> Result<(), ValidationError> {
        valid_entity_id(&self.entity_id)?;
        bounded(&self.label, 1, 128, "display_name")?;
        if self.stale_after_seconds == 0 || self.stale_after_seconds > 86_400 {
            return Err(ValidationError::InvalidPayload {
                field: "stale_after_seconds",
            });
        }
        unique(&self.allowed_commands, "allowed_commands")?;
        numbers(self.minimum, self.maximum, self.step)?;
        if self.controllable == self.allowed_commands.is_empty() {
            return Err(ValidationError::InvalidPayload {
                field: "controllable",
            });
        }
        Ok(())
    }
}

/// Public entity domains supported by v1.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityDomain {
    Sensor,
    Switch,
    Light,
    MediaPlayer,
}
/// Typed actuator actions accepted by v1.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum ActuatorAction {
    #[serde(rename = "entity.turn_on")]
    TurnOn,
    #[serde(rename = "entity.turn_off")]
    TurnOff,
    #[serde(rename = "entity.set_value")]
    SetValue,
}

/// A declared display, voice, or mobile surface.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Surface {
    pub surface_id: String,
    pub kind: SurfaceKind,
    #[serde(rename = "display_name")]
    pub label: String,
    pub views: Vec<ViewKind>,
}
/// Supported surface categories.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceKind {
    Display,
    Voice,
    Mobile,
}
/// Presentation views declared by a surface.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewKind {
    Text,
    NowPlaying,
    SensorGrid,
}
impl Surface {
    /// Checks identifiers, labels, and view cardinality.
    pub fn validate(&self) -> Result<(), ValidationError> {
        valid_entity_id(&self.surface_id)?;
        bounded(&self.label, 1, 128, "display_name")?;
        if self.views.len() > 3 {
            return Err(ValidationError::InvalidPayload { field: "views" });
        }
        unique(&self.views, "views")
    }
}

/// Stable capability declaration; runtime state is intentionally a separate type.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceCapabilities {
    pub revision: u64,
    pub items: Vec<DeviceCapability>,
}
impl DeviceCapabilities {
    /// Checks capability cardinality, names, and strict known payloads.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.revision == 0 || self.items.len() > 32 {
            return Err(ValidationError::InvalidPayload {
                field: "capabilities",
            });
        }
        let names: Vec<_> = self.items.iter().map(DeviceCapability::name).collect();
        unique(&names, "capability names")?;
        self.items.iter().try_for_each(DeviceCapability::validate)
    }
}

/// Capability union with lossless storage for valid unknown extensions.
#[derive(Clone, Debug, PartialEq)]
pub enum DeviceCapability {
    Playback {
        actions: Vec<String>,
    },
    Station {
        sources: Vec<String>,
    },
    Volume {
        step: u8,
        mute: bool,
    },
    Chromecast {
        actions: Vec<String>,
        discovery_ttl_seconds: u16,
    },
    Relay {
        actions: Vec<String>,
        modes: Vec<String>,
    },
    Display {
        views: Vec<ViewKind>,
        max_items: u8,
        max_text_length: u16,
    },
    Sensor {
        entity_classes: Vec<String>,
    },
    Actuator {
        commands: Vec<ActuatorAction>,
    },
    VoiceInput {
        formats: Vec<String>,
    },
    Unknown {
        name: String,
        version: u8,
        extra: Map<String, Value>,
    },
}
impl DeviceCapability {
    pub fn name(&self) -> String {
        match self {
            Self::Playback { .. } => "media.playback",
            Self::Station { .. } => "media.station",
            Self::Volume { .. } => "media.volume",
            Self::Chromecast { .. } => "media.chromecast",
            Self::Relay { .. } => "media.relay",
            Self::Display { .. } => "display.presentation",
            Self::Sensor { .. } => "entity.sensor",
            Self::Actuator { .. } => "entity.actuator",
            Self::VoiceInput { .. } => "voice.input",
            Self::Unknown { name, .. } => name,
        }
        .into()
    }
    fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::Playback { actions } => list(actions, 5, "actions"),
            Self::Station { sources } => list(sources, 2, "sources"),
            Self::Volume { step, .. } if *step == 0 => {
                Err(ValidationError::InvalidPayload { field: "step" })
            }
            Self::Chromecast {
                actions,
                discovery_ttl_seconds,
            } if !(5..=300).contains(discovery_ttl_seconds) => {
                Err(ValidationError::InvalidPayload {
                    field: "discovery_ttl_seconds",
                })
            }
            Self::Chromecast { actions, .. } => list(actions, 3, "actions"),
            Self::Relay { actions, modes } => {
                list(actions, 3, "actions")?;
                list(modes, 8, "modes")
            }
            Self::Display {
                views,
                max_items,
                max_text_length,
            } if views.len() > 3
                || *max_items == 0
                || *max_items > 32
                || *max_text_length == 0
                || *max_text_length > 1024 =>
            {
                Err(ValidationError::InvalidPayload {
                    field: "display capability",
                })
            }
            Self::Sensor { entity_classes } => list(entity_classes, 32, "entity_classes"),
            Self::Actuator { commands } => list(commands, 3, "commands"),
            Self::VoiceInput { formats } => list(formats, 4, "formats"),
            Self::Unknown { name, version, .. } if *version == 0 || !namespaced(name) => {
                Err(ValidationError::InvalidPayload {
                    field: EXTENSION_NAME,
                })
            }
            _ => Ok(()),
        }
    }
}
impl Serialize for DeviceCapability {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = Map::new();
        match self {
            Self::Playback { actions } => {
                m.insert("name".into(), Value::String("media.playback".into()));
                m.insert("version".into(), Value::from(1));
                m.insert("actions".into(), serde_json::to_value(actions).unwrap());
            }
            Self::Station { sources } => {
                m.insert("name".into(), Value::String("media.station".into()));
                m.insert("version".into(), Value::from(1));
                m.insert("sources".into(), serde_json::to_value(sources).unwrap());
            }
            Self::Volume { step, mute } => {
                m.insert("name".into(), Value::String("media.volume".into()));
                m.insert("version".into(), Value::from(1));
                m.insert("minimum".into(), Value::from(0));
                m.insert("maximum".into(), Value::from(100));
                m.insert("step".into(), Value::from(*step));
                m.insert("mute".into(), Value::from(*mute));
            }
            Self::Display {
                views,
                max_items,
                max_text_length,
            } => {
                m.insert("name".into(), Value::String("display.presentation".into()));
                m.insert("version".into(), Value::from(1));
                m.insert("views".into(), serde_json::to_value(views).unwrap());
                m.insert("max_items".into(), Value::from(*max_items));
                m.insert("max_text_length".into(), Value::from(*max_text_length));
            }
            Self::Sensor { entity_classes } => {
                m.insert("name".into(), Value::String("entity.sensor".into()));
                m.insert("version".into(), Value::from(1));
                m.insert(
                    "entity_classes".into(),
                    serde_json::to_value(entity_classes).unwrap(),
                );
            }
            Self::VoiceInput { formats } => {
                m.insert("name".into(), Value::String("voice.input".into()));
                m.insert("version".into(), Value::from(1));
                m.insert("formats".into(), serde_json::to_value(formats).unwrap());
            }
            Self::Chromecast {
                actions,
                discovery_ttl_seconds,
            } => {
                m.insert("name".into(), Value::String("media.chromecast".into()));
                m.insert("version".into(), Value::from(1));
                m.insert("actions".into(), serde_json::to_value(actions).unwrap());
                m.insert(
                    "discovery_ttl_seconds".into(),
                    Value::from(*discovery_ttl_seconds),
                );
            }
            Self::Relay { actions, modes } => {
                m.insert("name".into(), Value::String("media.relay".into()));
                m.insert("version".into(), Value::from(1));
                m.insert("actions".into(), serde_json::to_value(actions).unwrap());
                m.insert("modes".into(), serde_json::to_value(modes).unwrap());
            }
            Self::Actuator { commands } => {
                m.insert("name".into(), Value::String("entity.actuator".into()));
                m.insert("version".into(), Value::from(1));
                m.insert("commands".into(), serde_json::to_value(commands).unwrap());
            }
            Self::Unknown {
                name,
                version,
                extra,
            } => {
                m = extra.clone();
                m.insert("name".into(), Value::String(name.clone()));
                m.insert("version".into(), Value::from(*version));
            }
        };
        Value::Object(m).serialize(s)
    }
}
impl<'de> Deserialize<'de> for DeviceCapability {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let m = Map::<String, Value>::deserialize(d)?;
        let name = m
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| D::Error::custom("missing capability name"))?;
        let version = m
            .get("version")
            .and_then(Value::as_u64)
            .ok_or_else(|| D::Error::custom("missing capability version"))?;
        if version > 255 {
            return Err(D::Error::custom("invalid capability version"));
        }
        let a = |k| {
            m.get(k)
                .cloned()
                .ok_or_else(|| D::Error::custom(format!("missing {k}")))
        };
        let result = match name {
            "media.playback" => Self::Playback {
                actions: serde_json::from_value(a("actions")?).map_err(D::Error::custom)?,
            },
            "media.station" => Self::Station {
                sources: serde_json::from_value(a("sources")?).map_err(D::Error::custom)?,
            },
            "media.volume" => Self::Volume {
                step: serde_json::from_value(a("step")?).map_err(D::Error::custom)?,
                mute: serde_json::from_value(a("mute")?).map_err(D::Error::custom)?,
            },
            "display.presentation" => Self::Display {
                views: serde_json::from_value(a("views")?).map_err(D::Error::custom)?,
                max_items: serde_json::from_value(a("max_items")?).map_err(D::Error::custom)?,
                max_text_length: serde_json::from_value(a("max_text_length")?)
                    .map_err(D::Error::custom)?,
            },
            "entity.sensor" => Self::Sensor {
                entity_classes: serde_json::from_value(a("entity_classes")?)
                    .map_err(D::Error::custom)?,
            },
            "voice.input" => Self::VoiceInput {
                formats: serde_json::from_value(a("formats")?).map_err(D::Error::custom)?,
            },
            "media.chromecast" => Self::Chromecast {
                actions: serde_json::from_value(a("actions")?).map_err(D::Error::custom)?,
                discovery_ttl_seconds: serde_json::from_value(a("discovery_ttl_seconds")?)
                    .map_err(D::Error::custom)?,
            },
            "media.relay" => Self::Relay {
                actions: serde_json::from_value(a("actions")?).map_err(D::Error::custom)?,
                modes: serde_json::from_value(a("modes")?).map_err(D::Error::custom)?,
            },
            "entity.actuator" => Self::Actuator {
                commands: serde_json::from_value(a("commands")?).map_err(D::Error::custom)?,
            },
            _ => Self::Unknown {
                name: name.to_owned(),
                version: version as u8,
                extra: m.clone(),
            },
        };
        result.validate().map_err(D::Error::custom)?;
        Ok(result)
    }
}

/// Full replacement capability/entity/surface declaration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceManifest {
    pub manifest_revision: u64,
    pub roles: Vec<DeviceRole>,
    pub capabilities: DeviceCapabilities,
    pub entities: Vec<Entity>,
    pub surfaces: Vec<Surface>,
}
impl DeviceManifest {
    /// Checks bounded and internally consistent static device declarations.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.manifest_revision == 0
            || self.roles.len() > 8
            || self.entities.len() > 256
            || self.surfaces.len() > 16
        {
            return Err(ValidationError::InvalidPayload {
                field: "manifest limits",
            });
        }
        unique(&self.roles, "roles")?;
        self.capabilities.validate()?;
        self.entities.iter().try_for_each(Entity::validate)?;
        self.surfaces.iter().try_for_each(Surface::validate)?;
        unique(
            &self
                .entities
                .iter()
                .map(|e| &e.entity_id)
                .collect::<Vec<_>>(),
            "entity ids",
        )?;
        unique(
            &self
                .surfaces
                .iter()
                .map(|s| &s.surface_id)
                .collect::<Vec<_>>(),
            "surface ids",
        )?;
        if self.surfaces.iter().any(|s| s.kind == SurfaceKind::Display)
            && !self.roles.contains(&DeviceRole::DisplaySurface)
        {
            return Err(ValidationError::InvalidPayload {
                field: "display role",
            });
        }
        Ok(())
    }
}
