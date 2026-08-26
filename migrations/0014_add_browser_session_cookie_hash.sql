-- Bind the HttpOnly browser cookie to a server-side opaque proof. Nullable keeps this
-- migration compatible with browser rows created by the B2 persistence-only release.
ALTER TABLE browser_sessions ADD COLUMN session_token_hash bytea;
CREATE UNIQUE INDEX browser_sessions_active_token_idx
    ON browser_sessions (session_token_hash) WHERE revoked_at IS NULL AND session_token_hash IS NOT NULL;
