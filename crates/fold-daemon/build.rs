use std::path::PathBuf;

/// The embedded folder has to exist for the crate to compile; an empty one serves `web_not_built`.
fn main() -> std::io::Result<()> {
    let dist = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default())
        .join("../../web/dist");
    std::fs::create_dir_all(&dist)?;
    println!("cargo:rerun-if-changed={}", dist.display());
    Ok(())
}
