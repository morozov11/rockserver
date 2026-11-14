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

/// Ensures staging cleanup is shipped as a constrained one-shot operator path rather than a public route.
#[test]
fn staging_cleanup_is_preview_first_and_exact_target_only() {
    let dockerfile = std::fs::read_to_string(format!("{}/Dockerfile", env!("CARGO_MANIFEST_DIR")))
        .expect("server Dockerfile must be present");
    let remote = std::fs::read_to_string(format!(
        "{}/deploy/remote-ops-001-d.sh",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("remote deployment script must be present");
    assert!(dockerfile.contains("--bin account_cleanup"));
    assert!(dockerfile.contains("/usr/local/bin/account_cleanup"));
    assert!(remote.contains("cleanup) shift; cleanup_operator"));
    assert!(remote.contains("cleanup usage is preview or apply <account|device|credential>"));
    assert!(remote.contains("validate_uuid"));
    assert!(remote.contains("cleanup *"));
    assert!(remote.contains("DEACTIVATE ACCOUNT $id"));
    assert!(remote.contains("REVOKE DEVICE $id"));
    assert!(remote.contains("REVOKE CREDENTIAL $id"));
}

/// Ensures the deployment bundle does not package or require legacy ONNX assets.
#[test]
fn deploy_bundle_does_not_contain_onnx_assets() {
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

    assert!(
        !launcher.contains("onnx-assets"),
        "deploy launcher must not package onnx-assets"
    );
    assert!(
        !remote.contains("onnx-assets.json"),
        "remote deploy script must not require onnx-assets.json"
    );
    assert!(
        !remote.contains("download_onnx"),
        "remote deploy script must not contain download_onnx"
    );
    assert!(
        !std::path::Path::new(&format!(
            "{}/deploy/onnx-assets.lock.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .exists(),
        "onnx-assets.lock.json must be deleted"
    );
}

/// Ensures the owner.env allowlist rejects legacy ONNX runtime environment variables.
#[test]
fn owner_env_allowlist_rejects_onnx_variables() {
    let remote = std::fs::read_to_string(format!(
        "{}/deploy/remote-ops-001-d.sh",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("remote deployment script must be present");
    let psm1 = std::fs::read_to_string(format!(
        "{}/deploy/ops-001-d.psm1",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("deploy module must be present");

    assert!(
        !remote.contains("ROCKSERVER_ONNX"),
        "remote script allowlist must not allow ROCKSERVER_ONNX variables"
    );
    assert!(
        !remote.contains("ROCKSERVER_SEMANTIC_PROVIDER"),
        "remote script allowlist must not allow ROCKSERVER_SEMANTIC_PROVIDER"
    );
    assert!(
        !remote.contains("ORT_DYLIB_PATH"),
        "remote script allowlist must not allow ORT_DYLIB_PATH"
    );
    assert!(
        !psm1.contains("ROCKSERVER_ONNX"),
        "deploy module must not configure ROCKSERVER_ONNX variables"
    );
    assert!(
        !psm1.contains("ROCKSERVER_SEMANTIC_PROVIDER"),
        "deploy module must not configure ROCKSERVER_SEMANTIC_PROVIDER"
    );
    assert!(
        !psm1.contains("ORT_DYLIB_PATH"),
        "deploy module must not configure ORT_DYLIB_PATH"
    );
}

/// Ensures production Compose configurations omit legacy embedding backfill and ONNX runtime mounts.
#[test]
fn compose_omits_embedding_backfill_and_onnx() {
    let compose = std::fs::read_to_string(format!(
        "{}/deploy/compose.yaml",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("compose.yaml must be present");
    let compose_local = std::fs::read_to_string(format!(
        "{}/deploy/compose.local.yaml",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("compose.local.yaml must be present");

    assert!(
        !compose.contains("embedding_backfill:"),
        "compose.yaml must not contain embedding_backfill service"
    );
    assert!(
        !compose.contains("ROCKSERVER_ONNX_"),
        "compose.yaml must not reference ROCKSERVER_ONNX variables"
    );
    assert!(
        !compose.contains("ORT_DYLIB_PATH"),
        "compose.yaml must not reference ORT_DYLIB_PATH"
    );
    assert!(
        !compose.contains("ROCKSERVER_SEMANTIC_PROVIDER"),
        "compose.yaml must not reference ROCKSERVER_SEMANTIC_PROVIDER"
    );
    assert!(
        !compose_local.contains("ROCKSERVER_SEMANTIC_PROVIDER"),
        "compose.local.yaml must not reference ROCKSERVER_SEMANTIC_PROVIDER"
    );
}
