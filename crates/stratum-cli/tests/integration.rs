#![allow(deprecated)]

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn cli_help_shows_all_subcommands() {
    Command::cargo_bin("stratum-cli")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("run"))
        .stdout(predicate::str::contains("resume"))
        .stdout(predicate::str::contains("status"))
        .stdout(predicate::str::contains("trajectory"))
        .stdout(predicate::str::contains("export"))
        .stdout(predicate::str::contains("gates"))
        .stdout(predicate::str::contains("decide"))
        .stdout(predicate::str::contains("queue"))
        .stdout(predicate::str::contains("metrics"))
        .stdout(predicate::str::contains("dashboard"))
        .stdout(predicate::str::contains("serve"));
}

#[test]
fn cli_run_without_api_key_fails() {
    Command::cargo_bin("stratum-cli")
        .unwrap()
        .arg("run")
        .arg("build a thing")
        .env_remove("STRATUM_API_KEY")
        .assert()
        .failure()
        .stderr(predicate::str::contains("API key required"));
}

#[test]
fn cli_status_works_without_api_key() {
    let dir = tempfile::tempdir().unwrap();
    Command::cargo_bin("stratum-cli")
        .unwrap()
        .arg("status")
        .env_remove("STRATUM_API_KEY")
        .env("STRATUM_DATA_DIR", dir.path().to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("No runs found"));
}

#[test]
fn cli_gates_works_without_api_key() {
    let dir = tempfile::tempdir().unwrap();
    Command::cargo_bin("stratum-cli")
        .unwrap()
        .arg("gates")
        .env_remove("STRATUM_API_KEY")
        .env("STRATUM_DATA_DIR", dir.path().to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("No pending gates"));
}

#[test]
fn cli_queue_depth_works_without_api_key() {
    let dir = tempfile::tempdir().unwrap();
    Command::cargo_bin("stratum-cli")
        .unwrap()
        .args(["queue", "depth"])
        .env_remove("STRATUM_API_KEY")
        .env("STRATUM_DATA_DIR", dir.path().to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("Queue depth: 0"));
}

#[test]
fn cli_trajectory_invalid_run_id_fails() {
    let dir = tempfile::tempdir().unwrap();
    Command::cargo_bin("stratum-cli")
        .unwrap()
        .args(["trajectory", "not-a-uuid"])
        .env_remove("STRATUM_API_KEY")
        .env("STRATUM_DATA_DIR", dir.path().to_str().unwrap())
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid run ID"));
}

#[test]
fn cli_metrics_works_without_api_key() {
    let dir = tempfile::tempdir().unwrap();
    Command::cargo_bin("stratum-cli")
        .unwrap()
        .arg("metrics")
        .env_remove("STRATUM_API_KEY")
        .env("STRATUM_DATA_DIR", dir.path().to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("stratum_hitl_queue_depth"));
}

#[test]
fn cli_decide_invalid_decision_fails() {
    let dir = tempfile::tempdir().unwrap();
    Command::cargo_bin("stratum-cli")
        .unwrap()
        .args(["decide", "00000000-0000-0000-0000-000000000000", "unknown"])
        .env_remove("STRATUM_API_KEY")
        .env("STRATUM_DATA_DIR", dir.path().to_str().unwrap())
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid decision"));
}
