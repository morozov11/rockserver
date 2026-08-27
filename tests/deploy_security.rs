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
