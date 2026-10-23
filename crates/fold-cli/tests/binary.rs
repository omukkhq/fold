use std::io;
use std::process::{Command, Output};

fn fold(args: &[&str]) -> io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_fld")).args(args).output()
}

#[test]
fn version_flag_prints_the_version_line() -> io::Result<()> {
    let output = fold(&["--version"])?;
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "fld 0.5.1\n");
    Ok(())
}

#[test]
fn unknown_argument_fails_with_usage_status() -> io::Result<()> {
    let output = fold(&["--bogus"])?;
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn a_failed_write_exits_with_status_1() -> io::Result<()> {
    let output = Command::new(env!("CARGO_BIN_EXE_fld"))
        .stdout(std::fs::File::create("/dev/full")?)
        .output()?;
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).starts_with("Error: "));
    Ok(())
}
