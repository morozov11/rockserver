//! Protocol identity, timestamps, and validation errors.

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use std::fmt;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

pub(super) const EXTENSION_NAME: &str = "names must be lowercase dotted namespaces";

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
