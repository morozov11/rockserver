//! Private protocol-value validation primitives shared by domain model modules.

use std::collections::BTreeSet;

use super::ValidationError;

pub(super) fn namespaced(value: &str) -> bool {
    let parts: Vec<_> = value.split('.').collect();
    parts.len() >= 2
        && value.len() <= 96
        && parts.iter().all(|part| {
            let mut chars = part.chars();
            matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
                && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        })
}
pub(super) fn valid_entity_id(value: &str) -> Result<(), ValidationError> {
    if !(3..=128).contains(&value.len()) || !namespaced(value) {
        Err(ValidationError::InvalidPayload { field: "id" })
    } else {
        Ok(())
    }
}
pub(super) fn bounded(
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
pub(super) fn unique<T: Ord>(values: &[T], field: &'static str) -> Result<(), ValidationError> {
    if values.iter().collect::<BTreeSet<_>>().len() == values.len() {
        Ok(())
    } else {
        Err(ValidationError::InvalidPayload { field })
    }
}
pub(super) fn list<T: Ord>(
    values: &[T],
    maximum: usize,
    field: &'static str,
) -> Result<(), ValidationError> {
    if values.len() > maximum {
        return Err(ValidationError::InvalidPayload { field });
    }
    unique(values, field)
}
pub(super) fn numbers(
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
