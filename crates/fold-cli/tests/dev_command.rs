use std::process::Command;

#[test]
fn a_build_without_the_dev_seed_feature_has_no_dev_subcommand() {
    let output = Command::new(env!("CARGO_BIN_EXE_fold"))
        .args(["dev", "seed"])
        .env_clear()
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unrecognized subcommand 'dev'"), "{stderr}");
    let help = Command::new(env!("CARGO_BIN_EXE_fold"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(!String::from_utf8_lossy(&help.stdout).contains("dev"));
}
