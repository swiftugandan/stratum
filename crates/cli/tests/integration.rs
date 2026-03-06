use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn test_cli_runs() {
    Command::cargo_bin("stratum-cli")
        .unwrap()
        .assert()
        .success();
}

#[test]
fn test_cli_with_env() {
    Command::cargo_bin("stratum-cli")
        .unwrap()
        .env("RUST_LOG", "info")
        .assert()
        .success()
        .stdout(predicate::str::contains("stratum starting"));
}
