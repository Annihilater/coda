use anyhow::Result;
use clap::Parser;
use coda_desktop::config::{
    ensure_agents_md_manual, ensure_macos_gui_path, set_config_root_override, MalleableLayoutConfig,
};
use coda_desktop::git_inspector::compute_repo_snapshot;
use coda_desktop::pty::LocalPtyManager;
use coda_desktop::{launch_gpui_desktop_app, CodaDesktopState};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

#[derive(Debug, Parser)]
#[command(
    name = "coda-desktop",
    version,
    about = "Coda Native 120Hz GPU-Accelerated Workbench (Rust + GPUI for macOS, Windows & Linux)"
)]
struct Cli {
    /// Run headless self-test of the real workspace inspector, local PTY engine, and layout config
    #[arg(long)]
    self_test: bool,
}

fn filtered_cli_args() -> Vec<std::ffi::OsString> {
    let mut out = Vec::new();
    let mut iter = std::env::args_os();
    if let Some(bin) = iter.next() {
        out.push(bin);
    }
    while let Some(arg) = iter.next() {
        let s = arg.to_string_lossy();
        // #16: macOS LaunchServices passes `-psn_0_...` and `-NS*` / `-Apple*` defaults flags to `.app` bundles.
        if s.starts_with("-psn_") {
            continue;
        }
        if s.starts_with("-NS") || s.starts_with("-Apple") {
            let _ = iter.next();
            continue;
        }
        out.push(arg);
    }
    out
}

fn main() -> Result<()> {
    let cli = Cli::parse_from(filtered_cli_args());
    if cli.self_test {
        ensure_macos_gui_path();
        let tmp_cfg = std::env::temp_dir().join(format!("coda-desktop-selftest-{}", std::process::id()));
        set_config_root_override(Some(tmp_cfg.clone()));
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
        let starter_snippets = coda_desktop::config::load_snippets_from_disk_or_default();
        coda_desktop::config::save_snippets_to_disk(&starter_snippets)?;
        let reloaded_snippets = coda_desktop::config::load_snippets_from_disk_or_default();
        set_config_root_override(None);
        let _ = std::fs::remove_dir_all(&tmp_cfg);
        anyhow::ensure!(st.sessions.is_empty(), "default state must not seed fake sessions");
        anyhow::ensure!(st.spaces.is_empty(), "default state must not seed fake spaces");
        anyhow::ensure!(pty_ok, "real local PTY self-test failed to observe echoed marker");
        anyhow::ensure!(
            reloaded_snippets.len() == 4,
            "snippets.json cache roundtrip self-test failed"
        );
        anyhow::ensure!(
            coda_desktop::self_update::is_newer_version("v1.1.0", "1.0.0")
                && !coda_desktop::self_update::is_newer_version("v1.0.0", "1.0.0"),
            "self-update semver comparator self-test failed"
        );
        anyhow::ensure!(
            coda_desktop::self_update::detect_macos_app_bundle_root(std::path::Path::new(
                "/Applications/Coda.app/Contents/MacOS/coda-desktop"
            )) == Some(std::path::PathBuf::from("/Applications/Coda.app")),
            "macOS .app bundle root detection self-test failed"
        );
        anyhow::ensure!(
            coda_desktop::helpers::build_invite_registration_url("https://codahub.dev/", "CODA-XYZ")
                == "https://codahub.dev/register?invite=CODA-XYZ",
            "invite registration URL helper self-test failed"
        );
        anyhow::ensure!(
            coda_desktop::helpers::extract_reset_token_and_origin(
                "https://codahub.dev/reset-password?token=rst_selftest123"
            )
            .0 == "rst_selftest123",
            "password reset token extraction self-test failed"
        );
        anyhow::ensure!(
            coda_desktop::helpers::format_pasted_terminal_path("/tmp/coda-paste-demo.png")
                == "/tmp/coda-paste-demo.png ",
            "clipboard path helper self-test failed"
        );
        anyhow::ensure!(
            !coda_desktop::voice_dock::trigger_macos_system_dictation(),
            "headless macOS dictation fallback must be a deterministic no-op"
        );
        let voice_mode = coda_desktop::voice_dock::resolve_voice_dictation_mode_for_os(
            "https://api.openai.com/v1",
            "",
            true,
        )
        .map_err(anyhow::Error::msg)?;
        anyhow::ensure!(
            voice_mode == coda_desktop::voice_dock::VoiceDictationMode::MacosSystemDictation,
            "empty Whisper API key on macOS must select MacosSystemDictation fallback"
        );

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
