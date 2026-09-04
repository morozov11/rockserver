//! Transport-independent domain model for device-control protocol v1.
//!
//! The module deliberately owns no connection, HTTP, persistence, authorization, or executor
//! concerns.  It validates the protocol values which are safe to validate without those layers.

use async_trait::async_trait;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use serde_json::{Map, Value};
use std::{collections::BTreeSet, fmt};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

const EXTENSION_NAME: &str = "names must be lowercase dotted namespaces";

macro_rules! uuid_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);
    };
}
uuid_id!(DeviceId, "Stable account-owned device identity.");
uuid_id!(CommandId, "Idempotency identity for one command lifecycle.");
uuid_id!(
    MessageId,
    "Per-sender diagnostic identity for one protocol message."
);
uuid_id!(ConnectionId, "Ephemeral identity for one live connection.");
uuid_id!(EventId, "Identity for a future directory event lifecycle.");
uuid_id!(
    OperationId,
    "Identity for a future durable operation lifecycle."
);
uuid_id!(
    DeliveryId,
    "Identity for a future presentation delivery lifecycle."
);

/// A checked RFC3339 timestamp kept in canonical protocol text form.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Timestamp(String);
impl Timestamp {
    /// Parses and validates an RFC3339 timestamp.
    // Parses an RFC3339 instant rather than comparing timestamp text.
    pub fn parse(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        OffsetDateTime::parse(&value, &Rfc3339)
            .map_err(|_| ValidationError::InvalidPayload { field: "timestamp" })?;
        Ok(Self(value))
    }
    /// Returns the parsed UTC instant.
    // Returns the instant represented by this timestamp.
    pub fn instant(&self) -> OffsetDateTime {
        OffsetDateTime::parse(&self.0, &Rfc3339).expect("checked timestamp")
    }
    /// Returns the validated RFC3339 representation for persistence bindings.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}
impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::parse(String::deserialize(d)?).map_err(D::Error::custom)
    }
}

/// Stable validation errors which never include credentials or raw stream URLs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ValidationError {
    // A wire-compatible value violates a domain invariant.
    InvalidPayload { field: &'static str },
    // A revision was lower than the accepted revision.
    StaleRevision,
    // An equal revision had different content.
    ConflictingRevision,
    // A delta cannot apply without a full resync.
    ResyncRequired,
    // A required declared capability, entity, or surface was absent.
    CapabilityNotSupported,
    // A forward extension is valid but has no v1 executor.
    UnsupportedCommand,
}
impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for ValidationError {}
impl ValidationError {
    /// Returns the safe, stable protocol error code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidPayload { .. } => "invalid_payload",
            Self::StaleRevision | Self::ConflictingRevision | Self::ResyncRequired => {
                "stale_revision"
            }
            Self::CapabilityNotSupported => "capability_not_supported",
            Self::UnsupportedCommand => "unsupported_command",
        }
    }
}

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

/// Observed device runtime state, distinct from declared capabilities and commands.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct DeviceRuntimeState {
    pub playback: Option<PlaybackState>,
    pub volume: Option<VolumeState>,
    pub display: Option<DisplayState>,
}
/// Playback's current observed state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlaybackState {
    pub status: String,
    pub station_id: Option<String>,
}
/// Volume's current observed state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VolumeState {
    pub level: u8,
    pub muted: bool,
}
/// Display's current observed state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DisplayState {
    pub surface_id: String,
    pub view: String,
}
/// Complete runtime-state observation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceStateSnapshot {
    pub state_revision: u64,
    pub observed_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received_at: Option<Timestamp>,
    pub state: DeviceRuntimeState,
}
/// Incremental runtime-state observation requiring a known base revision.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceStateDelta {
    pub base_revision: u64,
    pub state_revision: u64,
    pub observed_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received_at: Option<Timestamp>,
    pub changes: DeviceRuntimeState,
}

/// Quality of an observed entity value.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    Ok,
    Degraded,
    Unavailable,
    Unknown,
}
/// Derived freshness state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    Fresh,
    Stale,
    Unknown,
}
/// Online/offline presence without a connection registry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Presence {
    pub status: PresenceStatus,
    pub last_seen_at: Option<Timestamp>,
}
/// Presence status.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresenceStatus {
    Online,
    Offline,
}
/// Normalized latest entity value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EntityState {
    pub entity_id: String,
    pub entity_revision: u64,
    pub value: Value,
    pub unit: Option<String>,
    pub quality: Quality,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub freshness: Option<Freshness>,
    pub observed_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received_at: Option<Timestamp>,
    pub stale_after: Timestamp,
}
impl EntityState {
    /// Derives freshness without consulting a wall clock.
    pub fn freshness_at(&self, now: &Timestamp) -> Freshness {
        if self.received_at.is_none() {
            Freshness::Unknown
        } else if now.instant() > self.stale_after.instant() {
            Freshness::Stale
        } else {
            Freshness::Fresh
        }
    }
    /// Checks normalized unit/value compatibility against declared entity metadata.
    pub fn validate_for(&self, entity: &Entity) -> Result<(), ValidationError> {
        valid_entity_id(&self.entity_id)?;
        if self.entity_revision == 0 {
            return Err(ValidationError::InvalidPayload {
                field: "entity_revision",
            });
        }
        if let Some(n) = self.value.as_f64()
            && !n.is_finite()
        {
            return Err(ValidationError::InvalidPayload { field: "value" });
        }
        if let Some(s) = self.value.as_str() {
            bounded(s, 0, 256, "value")?;
        }
        let numeric = self.value.is_number();
        let unit = self.unit.as_deref();
        let expected = match entity.device_class.as_str() {
            "temperature" => Some("°C"),
            "humidity" => Some("%"),
            "co2" => Some("ppm"),
            _ => None,
        };
        if expected.is_some() && (!numeric || unit != expected) {
            return Err(ValidationError::InvalidPayload {
                field: "unit/value",
            });
        }
        if let Some(n) = self.value.as_f64() {
            if let Some(min) = entity.minimum
                && n < min
            {
                return Err(ValidationError::InvalidPayload { field: "minimum" });
            }
            if let Some(max) = entity.maximum
                && n > max
            {
                return Err(ValidationError::InvalidPayload { field: "maximum" });
            }
        }
        Ok(())
    }
}

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

/// Receipt acknowledgement for a command.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommandReceived {
    pub command_id: CommandId,
    pub received_at: Timestamp,
    pub duplicate: bool,
}
/// Target acknowledgement that work started.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommandAccepted {
    pub command_id: CommandId,
    pub accepted_at: Timestamp,
}
/// Terminal command result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommandResult {
    pub command_id: CommandId,
    pub status: CommandStatus,
    pub completed_at: Timestamp,
    pub error: Option<DomainError>,
}
/// Terminal command status.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandStatus {
    Succeeded,
    Failed,
}
/// Safe structured domain error without HTTP coupling.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DomainError {
    pub code: String,
    pub message: String,
}
impl CommandResult {
    /// Checks the one terminal outcome invariant.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if matches!(
            (&self.status, &self.error),
            (CommandStatus::Succeeded, None) | (CommandStatus::Failed, Some(_))
        ) {
            Ok(())
        } else {
            Err(ValidationError::InvalidPayload {
                field: "command result",
            })
        }
    }
}

/// Revision comparison result shared by manifests, full states and entity states.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RevisionOrder {
    Next,
    Stale,
    Replay,
    Conflict,
    Gap,
}
/// Compares monotonic revisions and equality without attaching storage or transport policy.
pub fn revision_order<T: PartialEq>(
    accepted_revision: u64,
    accepted: &T,
    incoming_revision: u64,
    incoming: &T,
    required_base: Option<u64>,
) -> RevisionOrder {
    if incoming_revision < accepted_revision {
        RevisionOrder::Stale
    } else if incoming_revision == accepted_revision {
        if accepted == incoming {
            RevisionOrder::Replay
        } else {
            RevisionOrder::Conflict
        }
    } else if required_base.is_some_and(|base| base != accepted_revision)
        || incoming_revision != accepted_revision + 1
    {
        RevisionOrder::Gap
    } else {
        RevisionOrder::Next
    }
}

/// Safe persistence errors; database details are intentionally not exposed to protocol callers.
#[derive(Debug)]
pub enum StoreError {
    /// The input failed existing domain validation.
    Validation(ValidationError),
    /// The persistence backend failed without exposing its raw error string.
    Database,
}
impl StoreError {
    /// Converts a database-layer failure into the safe store error.
    pub fn database<E>(_error: E) -> Self {
        Self::Database
    }
    /// Converts a validation failure into the safe store error.
    pub fn validation(error: ValidationError) -> Self {
        Self::Validation(error)
    }
}
impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Validation(error) => error.code(),
            Self::Database => "persistence_unavailable",
        })
    }
}
impl std::error::Error for StoreError {}

/// Result of an ownership-scoped persistence mutation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreOutcome {
    Accepted,
    Replay,
    Stale,
    Conflict,
    Resync,
    NotOwned,
}

/// Canonical command reservation payload; the caller supplies a SHA-256 request fingerprint.
#[derive(Clone, Debug)]
pub struct CommandReservation {
    /// Validated request which supplies the idempotency key and explicit target.
    pub command: DeviceCommand,
    /// Fixed-size canonical request fingerprint.
    pub fingerprint: [u8; 32],
    /// RFC3339 command deadline.
    pub deadline_at: Timestamp,
}

/// Persisted command projection with an optional terminal result.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandLifecycle {
    /// Original validated request.
    pub command: DeviceCommand,
    /// Terminal result, when the lifecycle has completed.
    pub result: Option<CommandResult>,
}

/// Owner-scoped durable projections used by future transport and directory layers.
#[async_trait]
pub trait DeviceControlStore: Send + Sync {
    /// Atomically accepts a full manifest or reports its revision outcome.
    async fn apply_manifest(
        &self,
        user_id: Uuid,
        device_id: DeviceId,
        manifest: DeviceManifest,
    ) -> Result<StoreOutcome, StoreError>;
    /// Loads the accepted full manifest for an owned active device.
    async fn load_manifest(
        &self,
        user_id: Uuid,
        device_id: DeviceId,
    ) -> Result<Option<DeviceManifest>, StoreError>;
    /// Lists current, non-tombstoned entity projections.
    async fn list_entities(
        &self,
        user_id: Uuid,
        device_id: DeviceId,
    ) -> Result<Vec<Entity>, StoreError>;
    /// Lists current, non-tombstoned surface projections.
    async fn list_surfaces(
        &self,
        user_id: Uuid,
        device_id: DeviceId,
    ) -> Result<Vec<Surface>, StoreError>;
    /// Lists current, non-tombstoned capability projections.
    async fn list_capabilities(
        &self,
        user_id: Uuid,
        device_id: DeviceId,
    ) -> Result<Vec<DeviceCapability>, StoreError>;
    /// Stores one latest complete device-state snapshot.
    async fn store_device_state(
        &self,
        user_id: Uuid,
        device_id: DeviceId,
        state: DeviceStateSnapshot,
    ) -> Result<StoreOutcome, StoreError>;
    /// Loads the latest complete device-state snapshot for an owned active device.
    async fn load_device_state(
        &self,
        user_id: Uuid,
        device_id: DeviceId,
    ) -> Result<Option<DeviceStateSnapshot>, StoreError>;
    /// Stores one latest entity-state snapshot.
    async fn store_entity_state(
        &self,
        user_id: Uuid,
        device_id: DeviceId,
        state: EntityState,
    ) -> Result<StoreOutcome, StoreError>;
    /// Loads the latest entity-state snapshot when the entity belongs to the owned active device.
    async fn load_entity_state(
        &self,
        user_id: Uuid,
        device_id: DeviceId,
        entity_id: &str,
    ) -> Result<Option<EntityState>, StoreError>;
    /// Atomically reserves a bounded idempotency command record.
    async fn reserve_command(
        &self,
        user_id: Uuid,
        device_id: DeviceId,
        request: CommandReservation,
    ) -> Result<StoreOutcome, StoreError>;
    /// Returns a bounded command lifecycle for its owner and target device.
    async fn load_command(
        &self,
        user_id: Uuid,
        device_id: DeviceId,
        command_id: CommandId,
    ) -> Result<Option<CommandLifecycle>, StoreError>;
    /// Writes a terminal command result once.
    async fn complete_command(
        &self,
        user_id: Uuid,
        device_id: DeviceId,
        result: CommandResult,
    ) -> Result<StoreOutcome, StoreError>;
    /// Deletes at most `limit` expired terminal command records.
    async fn prune_commands(&self, limit: u32) -> Result<u64, StoreError>;
}

fn namespaced(value: &str) -> bool {
    let parts: Vec<_> = value.split('.').collect();
    parts.len() >= 2
        && value.len() <= 96
        && parts.iter().all(|part| {
            let mut chars = part.chars();
            matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
                && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        })
}
fn valid_entity_id(value: &str) -> Result<(), ValidationError> {
    if !(3..=128).contains(&value.len()) || !namespaced(value) {
        Err(ValidationError::InvalidPayload { field: "id" })
    } else {
        Ok(())
    }
}
fn bounded(
    value: &str,
    min: usize,
    max: usize,
    field: &'static str,
) -> Result<(), ValidationError> {
    if !(min..=max).contains(&value.len()) {
        Err(ValidationError::InvalidPayload { field })
    } else {
        Ok(())
    }
}
fn unique<T: Ord>(values: &[T], field: &'static str) -> Result<(), ValidationError> {
    if values.iter().collect::<BTreeSet<_>>().len() == values.len() {
        Ok(())
    } else {
        Err(ValidationError::InvalidPayload { field })
    }
}
fn list<T: Ord>(values: &[T], maximum: usize, field: &'static str) -> Result<(), ValidationError> {
    if values.len() > maximum {
        return Err(ValidationError::InvalidPayload { field });
    }
    unique(values, field)
}
fn numbers(
    minimum: Option<f64>,
    maximum: Option<f64>,
    step: Option<f64>,
) -> Result<(), ValidationError> {
    for n in [minimum, maximum, step].into_iter().flatten() {
        if !n.is_finite() {
            return Err(ValidationError::InvalidPayload {
                field: "numeric bounds",
            });
        }
    }
    if step.is_some_and(|s| s <= 0.0) || matches!((minimum, maximum), (Some(a), Some(b)) if a > b) {
        Err(ValidationError::InvalidPayload {
            field: "numeric bounds",
        })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(name: &str) -> Value {
        serde_json::from_str(match name {
            "rockcast" => {
                include_str!("../tests/fixtures/device-control/v1/rockcast-register-client.json")
            }
            "esp32" => {
                include_str!("../tests/fixtures/device-control/v1/esp32-manifest-client.json")
            }
            "grid" => include_str!(
                "../tests/fixtures/device-control/v1/display-sensor-grid-command-server.json"
            ),
            "unknown" => {
                include_str!("../tests/fixtures/device-control/v1/unknown-command-client.json")
            }
            "invalid" => include_str!(
                "../tests/fixtures/device-control/v1/invalid-sensor-unit-value-client.json"
            ),
            _ => unreachable!(),
        })
        .unwrap()
    }
    #[test]
    fn fixtures_round_trip_domain() {
        for name in ["rockcast", "esp32"] {
            let v = fixture(name);
            let manifest: DeviceManifest =
                serde_json::from_value(v["payload"]["manifest"].clone()).unwrap();
            manifest.validate().unwrap();
            assert_eq!(
                serde_json::to_value(&manifest).unwrap(),
                v["payload"]["manifest"]
            );
        }
    }
    #[test]
    fn extension_and_command_are_safe() {
        let cap: DeviceCapability = serde_json::from_str(include_str!(
            "../tests/fixtures/device-control/v1/unknown-capability.json"
        ))
        .unwrap();
        assert_eq!(serde_json::to_value(&cap).unwrap()["metric"], "pm25");
        let command: DeviceCommand =
            serde_json::from_value(fixture("unknown")["payload"].clone()).unwrap();
        assert_eq!(
            command.executable(),
            Err(ValidationError::UnsupportedCommand)
        );
        assert!(serde_json::from_str::<DeviceCapability>(r#"{"name":"bad","version":1}"#).is_err());
    }
    #[test]
    fn state_freshness_units_and_revision_are_deterministic() {
        let manifest: DeviceManifest =
            serde_json::from_value(fixture("esp32")["payload"]["manifest"].clone()).unwrap();
        let entity = &manifest.entities[0];
        let state: EntityState =
            serde_json::from_value(fixture("invalid")["payload"]["state"].clone()).unwrap();
        assert_eq!(
            state.validate_for(entity).unwrap_err().code(),
            "invalid_payload"
        );
        let valid: EntityState = serde_json::from_str(include_str!(
            "../tests/fixtures/device-control/v1/ha-normalized-entity-state.json"
        ))
        .unwrap();
        assert_eq!(
            valid.freshness_at(&Timestamp::parse("2026-09-02T12:05:00Z").unwrap()),
            Freshness::Fresh
        );
        assert_eq!(
            valid.freshness_at(&Timestamp::parse("2026-09-02T12:07:00Z").unwrap()),
            Freshness::Stale
        );
        let x = 1;
        assert_eq!(revision_order(2, &x, 2, &x, None), RevisionOrder::Replay);
        assert_eq!(revision_order(2, &x, 4, &x, Some(2)), RevisionOrder::Gap);
    }
    #[test]
    fn presentation_command_and_terminal_invariants() {
        let command: DeviceCommand =
            serde_json::from_value(fixture("grid")["payload"].clone()).unwrap();
        command
            .validate_at(&Timestamp::parse("2026-09-02T12:02:00Z").unwrap())
            .unwrap();
        let id = CommandId(Uuid::nil());
        let t = Timestamp::parse("2026-09-02T12:02:00Z").unwrap();
        assert!(
            CommandResult {
                command_id: id,
                status: CommandStatus::Succeeded,
                completed_at: t,
                error: Some(DomainError {
                    code: "x".into(),
                    message: "x".into()
                })
            }
            .validate()
            .is_err()
        );
    }
}
