mod common;
use common::Sandbox;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[test]
fn privacy_rejects_rehashed_unsafe_history_without_publishing_or_disclosing_it() {
    let source = Sandbox::new_seeded("backup-unsafe-history");
    let outside = Sandbox::new_empty_in("backup-unsafe-history-target", source.global_home());
    let package = outside.0.join("package");
    source.ok(&["backup", "create", package.to_str().unwrap()]);
    let manifest_path = package.join("manifest.json");
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    let index = manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .position(|member| member["path"] == ".vivac/events")
        .unwrap();
    let payload_path = package.join(format!("data/{index:08}"));
    let original = std::fs::read(&payload_path).unwrap();
    let mut records: Vec<Value> = std::str::from_utf8(&original)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let secret = format!("ghp_{}", "k".repeat(32));
    records[0]["extra_unrecognized_field"] = json!(secret);
    let mut unsafe_bytes = Vec::new();
    for record in records {
        serde_json::to_writer(&mut unsafe_bytes, &record).unwrap();
        unsafe_bytes.push(b'\n');
    }
    std::fs::write(&payload_path, &unsafe_bytes).unwrap();
    manifest["files"][index]["size"] = json!(unsafe_bytes.len());
    manifest["files"][index]["digest"] = json!(format!("{:x}", Sha256::digest(&unsafe_bytes)));
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let destination = outside.0.join("recovered");
    for args in [
        vec!["backup", "verify", package.to_str().unwrap()],
        vec![
            "backup",
            "recover",
            package.to_str().unwrap(),
            "--to",
            destination.to_str().unwrap(),
            "--source-retired",
        ],
    ] {
        let (text, code) = outside.run(&args);
        assert_eq!(code, 3, "{text}");
        assert!(
            !text.contains(&secret),
            "a rejected value must not be echoed"
        );
        assert!(!destination.exists());
    }
    let source_log = source.0.join(".vivac/events");
    std::fs::write(&source_log, &unsafe_bytes).unwrap();
    let refused_package = outside.0.join("refused-package");
    let (text, code) = source.run(&["backup", "create", refused_package.to_str().unwrap()]);
    assert_eq!(code, 3, "{text}");
    assert!(
        !text.contains(&secret),
        "a rejected value must not be echoed"
    );
    assert!(!refused_package.exists());
    assert_eq!(std::fs::read(source_log).unwrap(), unsafe_bytes);
    assert_eq!(std::fs::read(payload_path).unwrap(), unsafe_bytes);
}

fn marker(c: &Sandbox) {
    let log = c.log();
    let first: Value = serde_json::from_str(log.lines().next().unwrap()).unwrap();
    std::fs::write(
        c.0.join(".vivac/lane"),
        json!({"version":1,"id":"main","project":first["id"]}).to_string(),
    )
    .unwrap();
}

#[test]
fn complete_backup_verifies_offline_and_recovers_only_after_source_loss() {
    let source = Sandbox::new_seeded("backup-roundtrip");
    marker(&source);
    source.ok(&[
        "push",
        "Recover this synthetic work",
        "--why",
        "Exercise recovery",
    ]);
    let offline = Sandbox::new_empty_in("backup-offline", source.global_home());
    let package = offline.0.join("package");
    let destination = offline.0.join("recovered");
    let events = std::fs::read(source.0.join(".vivac/events")).unwrap();
    let config = std::fs::read(source.0.join(".vivac/config")).unwrap();
    let lane = std::fs::read(source.0.join(".vivac/lane")).unwrap();
    source.ok(&["backup", "create", package.to_str().unwrap(), "--json"]);
    assert!(!package.join(".vivac").exists());
    offline.ok(&["backup", "verify", package.to_str().unwrap(), "--json"]);
    assert_ne!(
        offline
            .run(&[
                "backup",
                "recover",
                package.to_str().unwrap(),
                "--to",
                destination.to_str().unwrap(),
                "--source-retired"
            ])
            .1,
        0
    );
    std::fs::remove_dir_all(source.0.join(".vivac")).unwrap();
    offline.ok(&[
        "backup",
        "recover",
        package.to_str().unwrap(),
        "--to",
        destination.to_str().unwrap(),
        "--source-retired",
        "--json",
    ]);
    assert_eq!(
        std::fs::read(destination.join(".vivac/events")).unwrap(),
        events
    );
    assert_eq!(
        std::fs::read(destination.join(".vivac/config")).unwrap(),
        config
    );
    assert_eq!(
        std::fs::read(destination.join(".vivac/lane")).unwrap(),
        lane
    );
    assert_eq!(
        std::fs::read(destination.join(".gitignore")).unwrap(),
        b"*\n"
    );
}

#[test]
fn backup_refuses_partial_logs_missing_configuration_and_occupied_destinations() {
    let source = Sandbox::new_seeded("backup-faults");
    marker(&source);
    let outside = Sandbox::new_empty_in("backup-faults-target", source.global_home());
    let package = outside.0.join("package");
    std::fs::create_dir(&package).unwrap();
    std::fs::write(package.join("keep"), b"Keep this file.").unwrap();
    assert_ne!(
        source
            .run(&["backup", "create", package.to_str().unwrap()])
            .1,
        0
    );
    assert_eq!(
        std::fs::read(package.join("keep")).unwrap(),
        b"Keep this file."
    );
    let partial = outside.0.join("partial");
    let original = std::fs::read(source.0.join(".vivac/events")).unwrap();
    let mut torn = original.clone();
    torn.extend_from_slice(b"{\"seq\":");
    std::fs::write(source.0.join(".vivac/events"), &torn).unwrap();
    assert_ne!(
        source
            .run(&["backup", "create", partial.to_str().unwrap()])
            .1,
        0
    );
    assert!(!partial.exists());
    assert_eq!(std::fs::read(source.0.join(".vivac/events")).unwrap(), torn);
    std::fs::write(source.0.join(".vivac/events"), original).unwrap();
    std::fs::remove_file(source.0.join(".vivac/config")).unwrap();
    assert_ne!(
        source
            .run(&["backup", "create", partial.to_str().unwrap()])
            .1,
        0
    );
    assert!(!source.0.join(".vivac/config").exists());
}

#[test]
fn verification_rejects_payload_tampering_extra_members_and_traversal() {
    let source = Sandbox::new_seeded("backup-integrity");
    let outside = Sandbox::new_empty_in("backup-integrity-target", source.global_home());
    let package = outside.0.join("package");
    source.ok(&["backup", "create", package.to_str().unwrap()]);
    outside.ok(&["backup", "verify", package.to_str().unwrap()]);
    let original = std::fs::read(package.join("manifest.json")).unwrap();
    let mut manifest: Value = serde_json::from_slice(&original).unwrap();
    manifest["files"][0]["path"] = json!("../escape");
    std::fs::write(
        package.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    assert_ne!(
        outside
            .run(&["backup", "verify", package.to_str().unwrap()])
            .1,
        0
    );
    std::fs::write(package.join("manifest.json"), original).unwrap();
    std::fs::write(package.join("extra"), b"Unexpected.").unwrap();
    assert_ne!(
        outside
            .run(&["backup", "verify", package.to_str().unwrap()])
            .1,
        0
    );
    std::fs::remove_file(package.join("extra")).unwrap();
    let payload = package.join("data/00000000");
    let mut bytes = std::fs::read(&payload).unwrap();
    bytes[0] ^= 1;
    std::fs::write(payload, bytes).unwrap();
    assert_ne!(
        outside
            .run(&["backup", "verify", package.to_str().unwrap()])
            .1,
        0
    );
    assert!(!outside.0.join("escape").exists());
}

fn authored_agent(source: &Sandbox) -> String {
    let definition = json!({"schema_version":1,"name":"reviewer","contract":{
        "purpose":"Review the scoped change.","duties":["Return evidence."],
        "limits":["Do not publish."],"acceptance":["Cite changed lines."]},
        "assignments":[{"harness":"codex","name":"reviewer","model":"inherit","effort":"high","settings":{}}]});
    let added: Value = serde_json::from_str(&source.ok(&[
        "agents",
        "add",
        "--definition",
        &definition.to_string(),
        "--why",
        "Exercise native custody",
    ]))
    .unwrap();
    let agent = added["agent"].as_str().unwrap().to_string();
    source.ok(&[
        "agents",
        "bind",
        &agent,
        "--harness",
        "codex",
        "--path",
        ".codex/agents/reviewer.toml",
    ]);
    source.ok(&["agents", "sync", &agent, "--yes"]);
    agent
}

fn artifact(source: &Sandbox) {
    std::fs::write(
        source.0.join("preview.html"),
        b"<html><body>Retained recovery fixture</body></html>",
    )
    .unwrap();
    source.ok(&[
        "artifacts",
        "capture",
        "preview.html",
        "--description",
        "Recovery fixture",
        "--from",
        "g1",
        "--json",
    ]);
}

#[test]
fn recovery_preserves_artifact_bytes_native_agents_and_safepoints() {
    let source = Sandbox::new_seeded("backup-custody");
    source.ok(&[
        "push",
        "Retain recovery evidence",
        "--why",
        "Verify complete custody",
    ]);
    artifact(&source);
    authored_agent(&source);
    source.ok(&["save", "Recovery fixture", "--next", "Verify recovery"]);
    let native = std::fs::read(source.0.join(".codex/agents/reviewer.toml")).unwrap();
    let events = std::fs::read(source.0.join(".vivac/events")).unwrap();
    let outside = Sandbox::new_empty_in("backup-custody-target", source.global_home());
    let package = outside.0.join("package");
    let destination = outside.0.join("recovered");
    source.ok(&["backup", "create", package.to_str().unwrap()]);
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(package.join("manifest.json")).unwrap()).unwrap();
    let mut artifacts = Vec::new();
    for member in manifest["files"].as_array().unwrap() {
        if member["kind"] == "artifact" {
            let path = member["path"].as_str().unwrap();
            artifacts.push((
                path.to_string(),
                std::fs::read(source.0.join(path)).unwrap(),
            ));
        }
    }
    assert!(!artifacts.is_empty());
    std::fs::remove_dir_all(source.0.join(".vivac")).unwrap();
    outside.ok(&[
        "backup",
        "recover",
        package.to_str().unwrap(),
        "--to",
        destination.to_str().unwrap(),
        "--source-retired",
    ]);
    assert_eq!(
        std::fs::read(destination.join(".vivac/events")).unwrap(),
        events
    );
    assert_eq!(
        std::fs::read(destination.join(".codex/agents/reviewer.toml")).unwrap(),
        native
    );
    assert!(!destination.join("preview.html").exists());
    assert!(
        !destination.join(".vivac/lane").exists(),
        "implicit main must remain implicit"
    );
    for (path, bytes) in artifacts {
        assert_eq!(std::fs::read(destination.join(path)).unwrap(), bytes);
    }
    outside.ok(&["backup", "verify", package.to_str().unwrap()]);
}

#[test]
fn additional_custody_lanes_require_explicit_roots_and_recover_separately() {
    let source = Sandbox::new_seeded("backup-lanes");
    source.ok(&[
        "push",
        "Retain lane evidence",
        "--why",
        "Verify explicit roots",
    ]);
    let lane = Sandbox::new_empty_in("backup-second-lane", source.global_home());
    lane.ok(&["init", "--yes", "--join", source.0.to_str().unwrap()]);
    artifact(&lane);
    let marker: Value =
        serde_json::from_slice(&std::fs::read(lane.0.join(".vivac/lane")).unwrap()).unwrap();
    let lane_id = marker["id"].as_str().unwrap();
    let outside = Sandbox::new_empty_in("backup-lanes-target", source.global_home());
    let package = outside.0.join("package");
    let destination = outside.0.join("recovered");
    assert_ne!(
        source
            .run(&["backup", "create", package.to_str().unwrap()])
            .1,
        0
    );
    assert!(!package.exists());
    let root = format!("{lane_id}={}", lane.0.display());
    source.ok(&[
        "backup",
        "create",
        package.to_str().unwrap(),
        "--lane-root",
        &root,
    ]);
    let unrelated = Sandbox::new_seeded_in("backup-unrelated-project", source.global_home());
    let unrelated_log = unrelated.log();
    let unrelated_event: Value =
        serde_json::from_str(unrelated_log.lines().next().unwrap()).unwrap();
    let unrelated_id = unrelated_event["id"].as_str().unwrap();
    let registry_path = source.global_home().join("projects");
    let before: Value = serde_json::from_slice(&std::fs::read(&registry_path).unwrap()).unwrap();
    let unrelated_before = before["projects"][unrelated_id].clone();
    assert!(!unrelated_before.is_null());
    std::fs::remove_dir_all(source.0.join(".vivac")).unwrap();
    outside.ok(&[
        "backup",
        "recover",
        package.to_str().unwrap(),
        "--to",
        destination.to_str().unwrap(),
        "--source-retired",
    ]);
    let restored = destination.join("lanes").join(lane_id);
    let restored_marker: Value =
        serde_json::from_slice(&std::fs::read(restored.join(".vivac/lane")).unwrap()).unwrap();
    assert_eq!(restored_marker, marker);
    assert!(restored.join(".vivac-artifacts/objects").is_dir());
    let after: Value = serde_json::from_slice(&std::fs::read(&registry_path).unwrap()).unwrap();
    let project_id = marker["project"].as_str().unwrap();
    let registered = &after["projects"][project_id];
    for (id, expected) in [("main", &destination), (lane_id, &restored)] {
        let actual = registered["lanes"][id].as_str().unwrap();
        assert_eq!(
            std::path::Path::new(actual).canonicalize().unwrap(),
            expected.canonicalize().unwrap()
        );
    }
    assert_eq!(after["projects"][unrelated_id], unrelated_before);
}

#[test]
fn backup_refuses_missing_custody_and_changed_native_agents() {
    let source = Sandbox::new_seeded("backup-stale-custody");
    source.ok(&[
        "push",
        "Retain recovery inputs",
        "--why",
        "Verify required bytes",
    ]);
    artifact(&source);
    let outside = Sandbox::new_empty_in("backup-stale-target", source.global_home());
    let package = outside.0.join("package");
    let objects = source.0.join(".vivac-artifacts/objects");
    let object = std::fs::read_dir(objects)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let bytes = std::fs::read(&object).unwrap();
    std::fs::remove_file(&object).unwrap();
    assert_ne!(
        source
            .run(&["backup", "create", package.to_str().unwrap()])
            .1,
        0
    );
    assert!(!package.exists());
    std::fs::write(&object, bytes).unwrap();
    authored_agent(&source);
    std::fs::write(
        source.0.join(".codex/agents/reviewer.toml"),
        b"Changed managed native file.",
    )
    .unwrap();
    assert_ne!(
        source
            .run(&["backup", "create", package.to_str().unwrap()])
            .1,
        0
    );
    assert!(!package.exists());
}

#[test]
fn hostile_package_paths_are_rejected_before_preparing_git_or_harness_settings() {
    let source = Sandbox::new_seeded("backup-hostile-paths");
    let outside = Sandbox::new_empty_in("backup-hostile-target", source.global_home());
    let package = outside.0.join("package");
    source.ok(&["backup", "create", package.to_str().unwrap()]);
    let original: Value =
        serde_json::from_slice(&std::fs::read(package.join("manifest.json")).unwrap()).unwrap();
    for (kind, path, harness) in [
        ("artifact", ".git/config", Value::Null),
        ("agent", ".mcp.json", json!("codex")),
        ("agent", ".codex/settings.json", json!("codex")),
    ] {
        let mut manifest = original.clone();
        manifest["files"][0]["kind"] = json!(kind);
        manifest["files"][0]["path"] = json!(path);
        manifest["files"][0]["lane"] = json!("main");
        manifest["files"][0]["harness"] = harness;
        std::fs::write(
            package.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let (text, code) = outside.run(&["backup", "verify", package.to_str().unwrap()]);
        assert_ne!(code, 0, "{text}");
        assert!(
            text.contains("path leaves") || text.contains("adapter directory"),
            "{text}"
        );
        assert!(!outside.0.join(path).exists());
    }
}

#[test]
fn verification_rejects_hard_linked_payloads_and_oversized_metadata() {
    let source = Sandbox::new_seeded("backup-link-metadata");
    let outside = Sandbox::new_empty_in("backup-link-target", source.global_home());
    let package = outside.0.join("package");
    source.ok(&["backup", "create", package.to_str().unwrap()]);
    let alias = outside.0.join("linked-payload");
    std::fs::hard_link(package.join("data/00000000"), &alias).unwrap();
    assert_ne!(
        outside
            .run(&["backup", "verify", package.to_str().unwrap()])
            .1,
        0
    );
    std::fs::remove_file(alias).unwrap();
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(package.join("manifest.json")).unwrap()).unwrap();
    manifest["files"][0]["size"] = json!(u64::MAX);
    std::fs::write(
        package.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    assert_ne!(
        outside
            .run(&["backup", "verify", package.to_str().unwrap()])
            .1,
        0
    );
}

#[test]
fn backup_help_works_offline_and_does_not_swallow_values() {
    let outside = Sandbox::new_empty("backup-help");
    let text = outside.ok(&["backup", "--help"]);
    assert!(text.contains("backup create"), "{text}");
    assert_ne!(outside.run(&["backup", "--help", "unexpected"]).1, 0);
    assert_ne!(outside.run(&["backup", "--help=unexpected"]).1, 0);
    assert!(!outside.0.join(".vivac").exists());
}

#[cfg(unix)]
#[test]
fn backup_rejects_source_overlap_through_a_parent_alias_and_linked_lane_roots() {
    let source = Sandbox::new_seeded("backup-parent-alias");
    let outside = Sandbox::new_empty_in("backup-parent-alias-target", source.global_home());
    let alias = outside.0.join("alias");
    std::os::unix::fs::symlink(&source.0, &alias).unwrap();
    let target = alias.join("package");
    let before = std::fs::read(source.0.join(".vivac/events")).unwrap();
    assert_ne!(
        source
            .run(&["backup", "create", target.to_str().unwrap()])
            .1,
        0
    );
    assert!(!source.0.join("package").exists());
    let target = outside.0.join("package");
    let root = format!("main={}", alias.display());
    assert_ne!(
        source
            .run(&[
                "backup",
                "create",
                target.to_str().unwrap(),
                "--lane-root",
                &root
            ])
            .1,
        0
    );
    assert!(!target.exists());
    assert_eq!(
        std::fs::read(source.0.join(".vivac/events")).unwrap(),
        before
    );
}

#[test]
fn backup_and_recovery_destinations_must_be_outside_git_working_trees() {
    let source = Sandbox::new_seeded("backup-git-privacy");
    let outside = Sandbox::new_empty_in("backup-git-privacy-target", source.global_home());
    let package = outside.0.join("package");
    source.ok(&["backup", "create", package.to_str().unwrap()]);
    let git_parent = outside.0.join("working-copy");
    std::fs::create_dir(&git_parent).unwrap();
    std::fs::write(
        git_parent.join(".git"),
        b"Synthetic Git marker; never interpret this file.",
    )
    .unwrap();
    let attempted_backup = git_parent.join("package");
    let (text, code) = source.run(&["backup", "create", attempted_backup.to_str().unwrap()]);
    assert_ne!(code, 0, "{text}");
    assert!(text.contains("outside Git"), "{text}");
    assert!(!attempted_backup.exists());
    let recovered = git_parent.join("recovered");
    let (text, code) = outside.run(&[
        "backup",
        "recover",
        package.to_str().unwrap(),
        "--to",
        recovered.to_str().unwrap(),
        "--source-retired",
    ]);
    assert_ne!(code, 0, "{text}");
    assert!(text.contains("outside Git"), "{text}");
    assert!(!recovered.exists());
    assert_eq!(std::fs::read_dir(&git_parent).unwrap().count(), 1);
    std::fs::rename(&package, git_parent.join("existing-package")).unwrap();
    outside.ok(&[
        "backup",
        "verify",
        git_parent.join("existing-package").to_str().unwrap(),
    ]);
    assert_eq!(std::fs::read_dir(&git_parent).unwrap().count(), 2);
}

#[test]
fn backup_refuses_unknown_configuration_and_lane_fields_without_disclosing_them() {
    let source = Sandbox::new_seeded("backup-closed-config");
    let outside = Sandbox::new_empty_in("backup-closed-config-target", source.global_home());
    let package = outside.0.join("package");
    let path = source.0.join(".vivac/config");
    let original = std::fs::read(&path).unwrap();
    let mut config: Value = serde_json::from_slice(&original).unwrap();
    config["synthetic_unrelated_field"] = json!("Synthetic unsupported content");
    let changed = serde_json::to_vec(&config).unwrap();
    std::fs::write(&path, &changed).unwrap();
    let (text, code) = source.run(&["backup", "create", package.to_str().unwrap()]);
    assert_ne!(code, 0, "{text}");
    assert!(
        !text.contains("synthetic_unrelated_field")
            && !text.contains("Synthetic unsupported content"),
        "{text}"
    );
    assert_eq!(std::fs::read(&path).unwrap(), changed);
    assert!(!package.exists());
    std::fs::write(&path, original).unwrap();
    marker(&source);
    let path = source.0.join(".vivac/lane");
    let mut lane: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    lane["synthetic_unrelated_field"] = json!("Synthetic unsupported content");
    let changed = serde_json::to_vec(&lane).unwrap();
    std::fs::write(&path, &changed).unwrap();
    let (text, code) = source.run(&["backup", "create", package.to_str().unwrap()]);
    assert_ne!(code, 0, "{text}");
    assert!(
        !text.contains("synthetic_unrelated_field")
            && !text.contains("Synthetic unsupported content"),
        "{text}"
    );
    assert_eq!(std::fs::read(&path).unwrap(), changed);
    assert!(!package.exists());
}
