fn main() {
    let attributes = tauri_build::Attributes::new()
        .app_manifest(tauri_build::AppManifest::new().commands(&[
            "start_install",
            "get_manager_state",
            "set_manager_state",
            "list_installed_versions",
            "check_prerequisites",
            "inspect_source",
            "prepare_release",
            "check_latest_release",
            "launch_omniscience",
            "open_github_browser",
            "navigate_github_browser",
            "open_github_devtools",
            "is_github_artifact_installed",
            "install_github_artifact",
        ]));
    tauri_build::try_build(attributes).expect("failed to run tauri-build");
}
