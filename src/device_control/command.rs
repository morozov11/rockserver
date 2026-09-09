//! Typed command and presentation payloads with their wire serialization rules.

use std::net::{Ipv4Addr, Ipv6Addr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use serde_json::{Map, Value};

use super::{
    ActuatorAction, CommandId, DeviceId, Freshness, Quality, Timestamp, ValidationError,
    foundation::EXTENSION_NAME,
    validation::{bounded, namespaced},
};

/// `media.station` source string for streams resolved by RockServer from its catalog.
pub const CATALOG_STATION_SOURCE: &str = "rockserver_catalog";
/// `media.station` source string for controller-supplied direct stream URIs.
pub const DIRECT_STATION_SOURCE: &str = "direct_stream";
/// Maximum station stream URI length accepted by the v1 command vocabulary.
pub const MAX_STREAM_URI_LENGTH: usize = 2048;

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
    /// Server-resolved or controller-supplied stream playback for a player target.
    PlayStream {
        /// Origin of the stream; only `RockserverCatalog` is server-dispatchable.
        source: StreamSource,
        /// Echo of the resolved station; required for `RockserverCatalog`.
        station_id: Option<String>,
        /// Validated stream URI; never logged and never echoed in errors.
        stream_uri: String,
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
/// Origin of a `station.play_stream` command body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StreamSource {
    /// RockServer resolved the primary catalog stream; `station_id` echoes the resolution.
    RockserverCatalog,
    /// The controller supplied a direct URI; allowed only for targets advertising it.
    DirectStream,
}
impl StreamSource {
    /// Wire value of the `source` discriminator.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::RockserverCatalog => CATALOG_STATION_SOURCE,
            Self::DirectStream => DIRECT_STATION_SOURCE,
        }
    }
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
            CommandBody::PlayStream {
                source,
                station_id,
                stream_uri,
            } => {
                match (source, station_id) {
                    (StreamSource::RockserverCatalog, Some(station_id)) => {
                        bounded(station_id, 1, 128, "station_id")?
                    }
                    (StreamSource::RockserverCatalog, None) => {
                        return Err(ValidationError::InvalidPayload {
                            field: "station_id",
                        });
                    }
                    (StreamSource::DirectStream, Some(_)) => {
                        return Err(ValidationError::InvalidPayload {
                            field: "station_id",
                        });
                    }
                    (StreamSource::DirectStream, None) => {}
                }
                validate_stream_uri(stream_uri).map_err(|_| ValidationError::InvalidPayload {
                    field: "stream_uri",
                })
            }
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
            Self::PlayStream {
                source,
                station_id,
                stream_uri,
            } => match station_id {
                Some(station_id) => serde_json::json!({
                    "name":"station.play_stream",
                    "source":source.as_str(),
                    "station_id":station_id,
                    "stream_uri":stream_uri
                }),
                None => serde_json::json!({
                    "name":"station.play_stream",
                    "source":source.as_str(),
                    "stream_uri":stream_uri
                }),
            },
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
            "station.play_stream" => {
                let source = match m.get("source").and_then(Value::as_str) {
                    Some(CATALOG_STATION_SOURCE) => StreamSource::RockserverCatalog,
                    Some(DIRECT_STATION_SOURCE) => StreamSource::DirectStream,
                    _ => return Err(D::Error::custom("invalid stream source")),
                };
                let station_id = match source {
                    StreamSource::RockserverCatalog => {
                        Some(serde_json::from_value(get("station_id")?).map_err(D::Error::custom)?)
                    }
                    // The direct-stream variant must not claim a catalog station.
                    StreamSource::DirectStream if m.contains_key("station_id") => {
                        return Err(D::Error::custom("invalid station echo"));
                    }
                    StreamSource::DirectStream => None,
                };
                Self::PlayStream {
                    source,
                    station_id,
                    stream_uri: serde_json::from_value(get("stream_uri")?)
                        .map_err(D::Error::custom)?,
                }
            }
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

/// Why a station stream URI was rejected at admission.
///
/// Rejections never carry the URI itself; callers must map them to fixed messages.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamUriRejection {
    /// The URI is not an absolute lowercase `http://` or `https://` URL.
    NotAbsoluteHttp,
    /// The URI exceeds [`MAX_STREAM_URI_LENGTH`] characters.
    TooLong,
    /// The authority has no host component.
    MissingHost,
    /// The authority carries userinfo.
    Userinfo,
    /// A fragment component is present.
    Fragment,
    /// An explicit port is malformed, zero, or above the TCP range.
    InvalidPort,
    /// The literal address or always-local name is not a public destination.
    ForbiddenDestination,
}

/// Validates the literal form of a station stream URI per the `StationStreamUri` contract.
///
/// Enforced: absolute lowercase `http(s)`, a non-empty host, no userinfo, no fragment, an
/// explicit port in 1..=65535 or the scheme default, at most [`MAX_STREAM_URI_LENGTH`]
/// characters, and literal IPv4/IPv6 or always-local host names limited to public
/// destinations (loopback, private, shared, link-local, unique-local, multicast, benchmarking,
/// documentation and reserved ranges are rejected).
///
/// DNS resolution and per-redirect checks are deliberately not performed here: the frozen
/// contract assigns them to the validating egress layers, and today only the target's bounded
/// stream client (`max_stream_redirects`) performs them. Hostnames that only resolve to
/// non-public addresses therefore pass this gate; that gap is a documented RS-3 limitation.
pub fn validate_stream_uri(uri: &str) -> Result<(), StreamUriRejection> {
    if uri.chars().count() > MAX_STREAM_URI_LENGTH {
        return Err(StreamUriRejection::TooLong);
    }
    let Some(rest) = uri
        .strip_prefix("https://")
        .or_else(|| uri.strip_prefix("http://"))
    else {
        return Err(StreamUriRejection::NotAbsoluteHttp);
    };
    if rest.contains('#') {
        return Err(StreamUriRejection::Fragment);
    }
    let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.contains('@') {
        return Err(StreamUriRejection::Userinfo);
    }
    let (host, port, literal_v6) = split_stream_authority(authority)?;
    if host.is_empty() {
        return Err(StreamUriRejection::MissingHost);
    }
    if let Some(port) = port {
        match port.parse::<u32>() {
            // Port zero is not a usable TCP destination; the contract range is 1..=65535.
            Ok(value) if (1..=u32::from(u16::MAX)).contains(&value) => {}
            _ => return Err(StreamUriRejection::InvalidPort),
        }
    }
    validate_stream_host(host, literal_v6)
}

/// Splits an authority into host, optional port, and whether the host was a bracketed literal.
///
/// Bracketed hosts must close with `]` before an optional `:port`; an unbracketed host may not
/// contain colons at all, which also rejects raw IPv6 literals.
fn split_stream_authority(
    authority: &str,
) -> Result<(&str, Option<&str>, bool), StreamUriRejection> {
    if let Some(bracketed) = authority.strip_prefix('[') {
        let (host, tail) = bracketed
            .split_once(']')
            .ok_or(StreamUriRejection::MissingHost)?;
        let port = tail.strip_prefix(':');
        if !tail.is_empty() && port.is_none() {
            return Err(StreamUriRejection::InvalidPort);
        }
        Ok((host, port, true))
    } else {
        match authority.split_once(':') {
            Some((host, port)) => Ok((host, Some(port), false)),
            None => Ok((authority, None, false)),
        }
    }
}

/// Classifies one host against the public-destination rules.
fn validate_stream_host(host: &str, literal_v6: bool) -> Result<(), StreamUriRejection> {
    if literal_v6 {
        let address = host
            .parse::<Ipv6Addr>()
            .map_err(|_| StreamUriRejection::ForbiddenDestination)?;
        return validate_stream_ipv6(&address);
    }
    if let Ok(address) = host.parse::<Ipv4Addr>() {
        return validate_stream_ipv4(&address);
    }
    if host
        .chars()
        .any(|character| character.is_ascii_whitespace() || character.is_ascii_control() || character == '%')
        || host.eq_ignore_ascii_case("localhost")
        // A purely numeric host can only be a non-dotted IPv4 shorthand such as 2130706433,
        // which some resolvers turn into a loopback address.
        || host.chars().all(|character| character.is_ascii_digit())
    {
        return Err(StreamUriRejection::ForbiddenDestination);
    }
    Ok(())
}

/// Rejects every IPv4 range that is not global unicast.
fn validate_stream_ipv4(address: &Ipv4Addr) -> Result<(), StreamUriRejection> {
    let [a, b, c, _] = address.octets();
    let forbidden = matches!(a, 0 | 10 | 127)
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 198 && matches!(b, 18 | 19))
        || matches!(
            (a, b, c),
            (192, 0, 0) | (192, 0, 2) | (192, 88, 99) | (198, 51, 100) | (203, 0, 113)
        )
        || (224..=255).contains(&a);
    if forbidden {
        return Err(StreamUriRejection::ForbiddenDestination);
    }
    Ok(())
}

/// Rejects IPv6 loopback/unspecified/multicast/link-local/unique-local and classifies
/// IPv4-mapped or IPv4-compatible literals through the IPv4 rules.
fn validate_stream_ipv6(address: &Ipv6Addr) -> Result<(), StreamUriRejection> {
    let segments = address.segments();
    // ::, ::1, ::a.b.c.d (96 zero bits) and ::ffff:a.b.c.d (IPv4-mapped) all reduce to the
    // IPv4 classification; unspecified and loopback land on forbidden 0.0.0.0/8 anyway.
    if segments[..5].iter().all(|&segment| segment == 0) && matches!(segments[5], 0 | 0xffff) {
        let embedded = (u32::from(segments[6]) << 16) | u32::from(segments[7]);
        return validate_stream_ipv4(&Ipv4Addr::from(embedded));
    }
    if address.is_multicast()
        || (segments[0] & 0xffc0) == 0xfe80
        || (segments[0] & 0xfe00) == 0xfc00
    {
        return Err(StreamUriRejection::ForbiddenDestination);
    }
    Ok(())
}
