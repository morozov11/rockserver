-- The pure-Rust verifier needs its opaque challenge state to survive restarts and replicas.
ALTER TABLE webauthn_challenges ADD COLUMN state_blob bytea;
