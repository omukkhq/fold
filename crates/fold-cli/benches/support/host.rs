use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use tempfile::TempDir;
use tokio::process::Command;

use super::{BenchResult, fail};

/// Where the run keeps its daemon data, config and runtime folders. `FOLD_BENCH_DIR` picks the disk; `close` removes the folder at the end of the run.
pub struct Host {
    dir: TempDir,
    pub port: u16,
}

impl Host {
    pub fn new() -> BenchResult<Self> {
        let base =
            std::env::var_os("FOLD_BENCH_DIR").map_or_else(std::env::temp_dir, PathBuf::from);
        std::fs::create_dir_all(&base)?;
        let dir = tempfile::Builder::new()
            .prefix("fold-bench-")
            .tempdir_in(base)?;
        for sub in ["config", "data", "run", "home"] {
            std::fs::create_dir_all(dir.path().join(sub))?;
        }
        set_private(&dir.path().join("run"))?;
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        Ok(Self { dir, port })
    }

    /// Removes the run's folder and returns the error when that fails, which dropping the host would hide. A directory without its write bit, such as a read-only `main/` folder, makes the removal fail, so the write bits come back first.
    pub fn close(self) -> io::Result<()> {
        let restored = restore_write_bits(self.dir.path());
        let removed = self.dir.close();
        restored.and(removed)
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn config_home(&self) -> PathBuf {
        self.path().join("config")
    }

    pub fn data_home(&self) -> PathBuf {
        self.path().join("data")
    }

    pub fn runtime_base(&self) -> PathBuf {
        self.path().join("run")
    }

    /// `fold <args>` with the isolated folders, run in `cwd`.
    pub fn fold(&self, args: &[&str], cwd: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fold"));
        command
            .args(args)
            .current_dir(cwd)
            .env("HOME", self.path().join("home"))
            .env("XDG_CONFIG_HOME", self.config_home())
            .env("XDG_DATA_HOME", self.data_home())
            .env("XDG_RUNTIME_DIR", self.runtime_base())
            .env_remove("FOLD_AGENT")
            .env_remove("FOLD_HUMAN")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command
    }

    /// Runs `fold <args>` to the end and returns its standard output.
    pub async fn run(
        &self,
        args: &[&str],
        cwd: &Path,
        envs: &[(&str, &str)],
    ) -> BenchResult<String> {
        let mut command = self.fold(args, cwd);
        command.envs(envs.iter().copied());
        let output = command.output().await?;
        if !output.status.success() {
            return fail(format!(
                "fold {} failed: {}",
                args.first().copied().unwrap_or_default(),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

/// Gives the owner full access to `path` and every directory below it. Symbolic links are not followed.
fn restore_write_bits(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::symlink_metadata(path)?.permissions().mode();
    if mode & 0o700 != 0o700 {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode | 0o700))?;
    }
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            restore_write_bits(&entry.path())?;
        }
    }
    Ok(())
}

fn set_private(path: &Path) -> BenchResult<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

/// The operating system, kernel, CPU and memory of this machine, as a Markdown list.
pub fn describe() -> String {
    use sysinfo::System;
    let mut system = System::new();
    system.refresh_cpu_all();
    system.refresh_memory();
    let cpu = system.cpus().first().map_or("unknown", |cpu| cpu.brand());
    format!(
        "- Name: {}\n- Operating system: {} {}\n- Kernel: {}\n- CPU: {} ({} cores)\n- Memory: {:.1} GiB",
        System::host_name().unwrap_or_else(|| "unknown".to_owned()),
        System::name().unwrap_or_else(|| "unknown".to_owned()),
        System::os_version().unwrap_or_default(),
        System::kernel_version().unwrap_or_else(|| "unknown".to_owned()),
        cpu.trim(),
        system.cpus().len(),
        system.total_memory() as f64 / (1024.0 * 1024.0 * 1024.0),
    )
}
