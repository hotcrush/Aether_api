fn main() {
    // Inject git commit hash (short) for version display.
    let commit = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=AETHER_GIT_COMMIT={commit}");

    // Inject build timestamp (UTC, RFC 3339).
    let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    println!("cargo:rustc-env=AETHER_BUILD_TIME={timestamp}");

    // Re-run build script when git HEAD changes.
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/refs");

    let attributes = tauri_build::Attributes::new().plugin(
        "webview-audit",
        tauri_build::InlinedPlugin::new().commands(&["report_copy"]),
    );

    tauri_build::try_build(attributes).expect("failed to build Tauri application")
}
