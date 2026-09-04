//! Monotonic revision ordering shared by manifest and state persistence.

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
