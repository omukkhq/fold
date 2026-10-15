use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use tokio::process::Child;

use super::host::Host;
use super::{BenchResult, fail};

/// The `fld serve` process of the run, on a free loopback port.
pub struct Daemon {
    child: Child,
}

impl Daemon {
    pub async fn start(host: &Host) -> BenchResult<Self> {
        let listen = format!("127.0.0.1:{}", host.port);
        let mut command = host.fold(&["serve", "--listen", &listen], host.path());
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let child = command.spawn()?;
        let daemon = Self { child };
        let deadline = Instant::now() + Duration::from_secs(60);
        while tokio::net::TcpStream::connect(("127.0.0.1", host.port))
            .await
            .is_err()
        {
            if Instant::now() > deadline {
                return fail("the daemon did not listen within 60 seconds");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Ok(daemon)
    }

    pub fn pid(&self) -> BenchResult<u32> {
        match self.child.id() {
            Some(pid) => Ok(pid),
            None => fail("the daemon has exited"),
        }
    }

    /// Stops the daemon with SIGTERM, so it shuts down cleanly, and waits for it.
    pub async fn stop(mut self) -> BenchResult<()> {
        if let Some(pid) = self
            .child
            .id()
            .and_then(|pid| i32::try_from(pid).ok())
            .and_then(rustix::process::Pid::from_raw)
        {
            rustix::process::kill_process(pid, rustix::process::Signal::TERM)?;
        }
        match tokio::time::timeout(Duration::from_secs(60), self.child.wait()).await {
            Ok(status) => {
                status?;
                Ok(())
            }
            Err(_) => fail("the daemon did not stop within 60 seconds"),
        }
    }
}

/// The resident memory of a process in bytes.
pub fn resident_memory(system: &mut System, pid: u32) -> BenchResult<u64> {
    let pid = Pid::from_u32(pid);
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing().with_memory(),
    );
    match system.process(pid) {
        Some(process) => Ok(process.memory()),
        None => fail("the daemon process is gone"),
    }
}
