use std::process::Command;

#[test]
fn a_build_without_the_dev_seed_feature_has_no_dev_subcommand() {
    let mut command = Command::new(env!("CARGO_BIN_EXE_fld"));
    command.args(["dev", "seed"]).env_clear();
    // Coverage runs name the profile file through this variable. `env_clear` drops every variable set before it, so it comes after.
    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
    let output = command.output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unrecognized subcommand 'dev'"), "{stderr}");
    let help = Command::new(env!("CARGO_BIN_EXE_fld"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(!String::from_utf8_lossy(&help.stdout).contains("dev"));
}
