use anyhow::Result;
use clap::Parser;
use coda_desktop::config::{ensure_agents_md_manual, ensure_macos_gui_path, MalleableLayoutConfig};
use coda_desktop::git_inspector::compute_repo_snapshot;
use coda_desktop::pty::LocalPtyManager;
use coda_desktop::{launch_gpui_desktop_app, CodaDesktopState};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

#[derive(Debug, Parser)]
#[command(
    name = "coda-desktop",
    about = "Coda Native 120Hz GPU-Accelerated Workbench (Rust + GPUI for macOS, Windows & Linux)"
)]
struct Cli {
    /// Run headless self-test of the real workspace inspector, local PTY engine, and layout config
    #[arg(long)]
    self_test: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.self_test {
        ensure_macos_gui_path();
        MalleableLayoutConfig::save_default_if_missing();
        ensure_agents_md_manual();

        let dirty = Arc::new(AtomicBool::new(false));
        let mut pty = LocalPtyManager::new(dirty);
        let cwd = std::env::current_dir()?;
        pty.spawn_session("self-1", &cwd, None, 96, 24)
            .map_err(anyhow::Error::msg)?;
        std::thread::sleep(std::time::Duration::from_millis(150));
        pty.send_command_line("self-1", "echo CODA_SELF_TEST_PTY_OK")
            .map_err(anyhow::Error::msg)?;
        std::thread::sleep(std::time::Duration::from_millis(250));
        let snaps = pty.snapshot_all(40);
        let pty_ok = snaps
            .get("self-1")
            .map(|s| s.lines.iter().any(|l| l.contains("CODA_SELF_TEST_PTY_OK")))
            .unwrap_or(false);
        pty.kill_session("self-1");

        let git_snap = compute_repo_snapshot(&cwd, None);
        let st = CodaDesktopState::default();
        anyhow::ensure!(st.sessions.is_empty(), "default state must not seed fake sessions");
        anyhow::ensure!(st.spaces.is_empty(), "default state must not seed fake spaces");
        anyhow::ensure!(pty_ok, "real local PTY self-test failed to observe echoed marker");

        println!(
            "[OK] coda-desktop self-test passed: pty=live, git_repo={}, branch={}, diff_len={}, layout={}",
            git_snap.is_git_repo,
            git_snap.branch,
            git_snap.diff_text.len(),
            st.layout.layout.as_slug(),
        );
        return Ok(());
    }

    launch_gpui_desktop_app();
    Ok(())
}
