use super::*;

#[test]
fn frame_limit_follows_integration_identity_across_connections() {
    let state = Mutex::new(PublicLimitState::default());
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    for _ in 0..MAX_FRAMES_PER_MINUTE {
        assert!(admitted(&state, first));
    }
    assert!(!admitted(&state, first));
    assert!(admitted(&state, second));
}
