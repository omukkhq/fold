mod support;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fold_core::codes;
use fold_core::layout::ProjectPaths;
use serde_json::json;
use support::daemon::{Daemon, resident_memory};
use support::events::EventStream;
use support::fixture::{self, Sizes};
use support::host::{self, Host};
use support::report::{Report, utc_date};
use support::session::Session;
use support::targets::Target;
use support::{BenchResult, fail, integration};

/// Each timing target runs this many times; the reported value is the median.
const SAMPLES: usize = 5;
const SIZE_VARIABLE: &str = "FOLD_BENCH_SIZE";
const VISIBILITY_TIMEOUT: Duration = Duration::from_secs(30);

struct Mode {
    sizes: Sizes,
    idle_seconds: u64,
}

fn main() -> ExitCode {
    let outcome = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(Into::into)
        .and_then(|runtime| runtime.block_on(run()));
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bench: {error}");
            ExitCode::FAILURE
        }
    }
}

fn mode() -> BenchResult<Mode> {
    match std::env::var(SIZE_VARIABLE).ok().as_deref() {
        None | Some("full") => Ok(Mode {
            sizes: Sizes::FULL,
            idle_seconds: 30,
        }),
        Some("smoke") => Ok(Mode {
            sizes: Sizes::smoke(),
            idle_seconds: 3,
        }),
        Some(other) => fail(format!(
            "{}: {SIZE_VARIABLE} must be full or smoke; got {other:?}",
            codes::INVALID_CONFIG
        )),
    }
}

async fn run() -> BenchResult<()> {
    let mode = mode()?;
    let host = Host::new()?;
    let folder = host.path().display().to_string();
    let measured = measure(&host, &mode).await;
    let report = match (measured, host.close()) {
        (Ok(report), Ok(())) => report,
        (Err(error), Ok(())) => return Err(error),
        (Ok(_), Err(removal)) => return fail(format!("could not remove {folder}: {removal}")),
        (Err(error), Err(removal)) => {
            return fail(format!(
                "{error}; could not remove {folder} either: {removal}"
            ));
        }
    };

    let table = report.table();
    emit(&format!("{table}{} targets\n", report.entries().len()));
    write_results(&report)
}

async fn measure(host: &Host, mode: &Mode) -> BenchResult<Report> {
    let mut report = Report::default();
    let daemon = Daemon::start(host).await?;
    let measured = measure_with_daemon(host, &daemon, mode, &mut report).await;
    daemon.stop().await?;
    measured?;
    Ok(report)
}

#[expect(clippy::print_stdout, reason = "the bench prints its report")]
fn emit(text: &str) {
    println!("{text}");
}

async fn measure_with_daemon(
    host: &Host,
    daemon: &Daemon,
    mode: &Mode,
    report: &mut Report,
) -> BenchResult<()> {
    let sizes = mode.sizes;

    let idle_root = host.path().join("idle");
    fixture::write_repository(&idle_root, sizes.idle_files, 4.min(sizes.idle_files))?;
    init_project(host, &idle_root).await?;
    let idle = idle_memory(host, daemon, &idle_root, mode.idle_seconds).await?;
    report.record(Target::IdleMemory, idle as f64);

    let root = host.path().join("big");
    fixture::write_repository(&root, sizes.files, sizes.directories)?;
    init_project(host, &root).await?;

    let frontend = Session::open(host, &root, "frontend").await?;
    let backend = Session::open(host, &root, "backend").await?;
    let review = Session::open_main(host, &root, "review").await?;

    let agents = integration::Agents {
        frontend: &frontend,
        backend: &backend,
        review: &review,
    };
    let measured = measure_project(host, &agents, &root, sizes, report).await;
    for session in [frontend, backend, review] {
        session.close().await;
    }
    measured
}

async fn measure_project(
    host: &Host,
    agents: &integration::Agents<'_>,
    root: &Path,
    sizes: Sizes,
    report: &mut Report,
) -> BenchResult<()> {
    measure_workspace(host, agents.frontend, root, sizes, report).await?;
    let timings = integration::measure(agents, root, sizes, SAMPLES).await?;
    report.record(Target::Integration, median(timings));
    Ok(())
}

/// The folder's files become `r1`, as `fld init` makes a folder the project.
async fn init_project(host: &Host, root: &Path) -> BenchResult<()> {
    host.run(&["init"], root, &[]).await?;
    // Automatic integration would integrate a proposal as soon as the main agent approves it, before the bench's own `integrate_proposal` call.
    let config = ProjectPaths::new(&root.canonicalize()?).config();
    let mut text = std::fs::read_to_string(&config)?;
    text.push_str("\n[integration]\nautomatic = false\n");
    std::fs::write(&config, text)?;
    Ok(())
}

/// Three sessions with three active workspaces of about 100 files each and no edits.
async fn idle_memory(host: &Host, daemon: &Daemon, root: &Path, seconds: u64) -> BenchResult<u64> {
    let mut sessions = Vec::new();
    for name in ["frontend", "backend", "review"] {
        let session = Session::open(host, root, name).await?;
        let task = session.create_task(&format!("idle {name}")).await?;
        session.create_workspace(&task).await?;
        sessions.push(session);
    }
    let pid = daemon.pid()?;
    let mut system = sysinfo::System::new();
    let mut peak = 0;
    for _ in 0..seconds {
        tokio::time::sleep(Duration::from_secs(1)).await;
        peak = peak.max(resident_memory(&mut system, pid)?);
    }
    for session in sessions {
        session.close().await;
    }
    Ok(peak)
}

async fn measure_workspace(
    host: &Host,
    session: &Session,
    root: &Path,
    sizes: Sizes,
    report: &mut Report,
) -> BenchResult<()> {
    let mut creation = Vec::new();
    let mut kept: Option<(String, PathBuf)> = None;
    for sample in 0..SAMPLES {
        let task = session
            .create_task(&format!("bench creation {sample}"))
            .await?;
        let start = Instant::now();
        let (workspace, path) = session.create_workspace(&task).await?;
        creation.push(start.elapsed().as_secs_f64());
        let directory = root.join(path);
        if kept.is_none() {
            kept = Some((workspace, directory));
        } else {
            session
                .call(
                    "abandon_workspace",
                    json!({ "ws": workspace, "reason": "bench sample done" }),
                )
                .await?;
            std::fs::remove_dir_all(directory)?;
        }
    }
    report.record(Target::WorkspaceCreation, median(creation));
    let Some((workspace, directory)) = kept else {
        return fail("no workspace was created");
    };

    session
        .call("record_checkpoint", json!({ "ws": workspace }))
        .await?;
    let mut scans = Vec::new();
    for _ in 0..SAMPLES {
        let start = Instant::now();
        session
            .call("record_checkpoint", json!({ "ws": workspace }))
            .await?;
        scans.push(start.elapsed().as_secs_f64());
    }
    report.record(Target::FullStatScan, median(scans));

    let visibility = change_visibility(host, &workspace, &directory, sizes).await;
    session
        .call(
            "abandon_workspace",
            json!({ "ws": workspace, "reason": "bench done" }),
        )
        .await?;
    std::fs::remove_dir_all(&directory)?;
    report.record(Target::ChangeVisibility, median(visibility?));
    Ok(())
}

/// Appends one byte to a tracked file and waits for the `workspace.changed` frame that lists it. Each sample changes another file, so a frame of an earlier sample never matches.
async fn change_visibility(
    host: &Host,
    workspace: &str,
    directory: &Path,
    sizes: Sizes,
) -> BenchResult<Vec<f64>> {
    let mut stream = EventStream::open(host.port, "big").await?;
    stream
        .wait_for(VISIBILITY_TIMEOUT, |name, _| name == "snapshot")
        .await?;
    let mut timings = Vec::new();
    for sample in 0..SAMPLES {
        let relative = fixture::path_of(sample, sizes.directories);
        let start = Instant::now();
        {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(directory.join(&relative))?;
            file.write_all(b"x")?;
        }
        stream
            .wait_for(VISIBILITY_TIMEOUT, |name, data| {
                name == "workspace.changed"
                    && data["workspace"] == workspace
                    && data["files"].as_array().is_some_and(|files| {
                        files.iter().any(|file| file["path"] == relative.as_str())
                    })
            })
            .await?;
        timings.push(start.elapsed().as_secs_f64());
    }
    Ok(timings)
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    match values.len() {
        0 => 0.0,
        length if length % 2 == 1 => values[length / 2],
        length => (values[length / 2 - 1] + values[length / 2]) / 2.0,
    }
}

fn write_results(report: &Report) -> BenchResult<()> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/bench");
    std::fs::create_dir_all(&directory)?;
    std::fs::write(directory.join("results.json"), report.json()?)?;
    let seconds = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    std::fs::write(
        directory.join("results.md"),
        report.markdown(
            &host::describe(),
            &utc_date(seconds),
            env!("CARGO_PKG_VERSION"),
        ),
    )?;
    Ok(())
}
