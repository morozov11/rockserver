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
