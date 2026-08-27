//! Regression checks for security-critical deployment headers.

/// Ensures an approval QR URL cannot become a same-origin request referrer.
#[test]
fn caddy_disables_referrers_for_pairing_approval_urls() {
    for caddyfile in [
        "deploy/Caddyfile.local",
        "deploy/Caddyfile.production.template",
    ] {
        let contents =
            std::fs::read_to_string(format!("{}/{}", env!("CARGO_MANIFEST_DIR"), caddyfile))
                .expect("Caddyfile must be present");
        assert!(
            contents.contains("Referrer-Policy \"no-referrer\""),
            "{caddyfile} must not expose a QR approval secret as a request referrer"
        );
        assert!(
            contents.contains("handle @api {"),
            "{caddyfile} must proxy API routes before the SPA fallback"
        );
    }
}

/// Ensures bootstrap creates the Caddy-to-server proof instead of leaving production Compose invalid.
#[test]
fn bootstrap_provisions_the_trusted_proxy_proof() {
    let script = std::fs::read_to_string(format!(
        "{}/deploy/remote-ops-001-d.sh",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("deployment script must be present");
    assert!(script.contains("write_or_keep_secret ROCKSERVER_TRUSTED_PROXY_TOKEN"));
}

/// Ensures production deployment binds the immutable Caddy bundle to the same source commit.
#[test]
fn deploy_transfers_a_commit_bound_caddy_image() {
    let launcher = std::fs::read_to_string(format!(
        "{}/deploy/ops-001-d.ps1",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("deployment launcher must be present");
    let remote = std::fs::read_to_string(format!(
        "{}/deploy/remote-ops-001-d.sh",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("remote deployment script must be present");
    assert!(launcher.contains("rockserver-caddy:sha-$commit"));
    assert!(launcher.contains("rockserver-caddy-image.tar"));
    assert!(remote.contains("validate_caddy_artifact"));
}

/// Ensures the clean Caddy image build permits only the bundled esbuild binary script.
#[test]
fn web_build_policy_allows_esbuild() {
    let policy = std::fs::read_to_string(format!(
        "{}/web/pnpm-workspace.yaml",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("web build policy must be present");
    assert!(policy.contains("esbuild: true"));
}

/// Ensures the clean Caddy image build never waits for an interactive pnpm prompt.
#[test]
fn caddy_image_build_is_non_interactive() {
    let dockerfile = std::fs::read_to_string(format!(
        "{}/deploy/Dockerfile.caddy",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("Caddy Dockerfile must be present");
    assert!(dockerfile.contains("RUN CI=true pnpm run build"));
}
