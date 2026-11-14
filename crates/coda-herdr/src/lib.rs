//! Async Rust client bridging Coda Daemon to local `herdr-core` (`hc` OpenRPC 1.3.2).
//! Supports live `hello`, `terminal.list`, `terminal.run` (Claude Code, Codex, Grok Build,
//! Gemini CLI, shells), `terminal.read`, `terminal.send`, `terminal.keys`,
//! `terminal.interrupt`, and real-time `events.subscribe` (`OSC 7501` status tracking).

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use coda_protocol::{
    BranchWorktreeItem, HerdrAgentStatus, HerdrTerminalSummary, RepoWorkspaceSummary,
    VoiceModelConfig,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
    sync::{mpsc, oneshot, Mutex},
};
use tracing::{debug, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HerdrHelloInfo {
    pub core_id: String,
    pub core_name: String,
    pub version: String,
    pub generation: u32,
}

#[derive(Debug, Deserialize)]
struct RawStatus {
    state: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    app: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    progress: Option<u8>,
}

#[derive(Debug, Deserialize)]
struct RawTerminal {
    address: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    argv: Vec<String>,
    #[serde(default)]
    cwd: String,
    #[serde(default)]
    cols: u16,
    #[serde(default)]
    rows: u16,
    #[serde(default)]
    state: String,
    #[serde(default)]
    status: Option<RawStatus>,
}

impl From<RawTerminal> for HerdrTerminalSummary {
    fn from(raw: RawTerminal) -> Self {
        Self {
            address: raw.address,
            name: raw.name,
            argv: raw.argv,
            cwd: raw.cwd,
            cols: raw.cols,
            rows: raw.rows,
            alive: raw.state == "live",
            status: raw.status.map(|s| HerdrAgentStatus {
                state: s.state,
                kind: s.kind,
                app: s.app,
                title: s.title,
                msg: s.msg,
                progress: s.progress,
            }),
        }
    }
}

/// Locates the `herdr-core` / `hc` binary across standard macOS & Linux install paths.
///
/// Prefers `herdr-core` over bare `hc` to avoid collisions with unrelated binaries such as
/// the `herbstclient` (`hc`) window manager tool.
pub fn discover_hc_binary() -> Option<PathBuf> {
    for var in ["HERDR_CORE_BIN", "HC_BIN"] {
        if let Ok(custom) = std::env::var(var) {
            let p = PathBuf::from(custom);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let path_var = std::env::var_os("PATH");
    discover_hc_binary_in(home.as_deref(), path_var.as_deref())
}

fn discover_hc_binary_in(home: Option<&Path>, path_var: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let mut base_dirs: Vec<PathBuf> = Vec::new();
    if let Some(home) = home {
        base_dirs.push(home.join(".local/bin"));
        base_dirs.push(home.join(".cargo/bin"));
    }
    base_dirs.push(PathBuf::from("/opt/homebrew/bin"));
    base_dirs.push(PathBuf::from("/usr/local/bin"));
    base_dirs.push(PathBuf::from("/usr/bin"));
    if let Some(pv) = path_var {
        for entry in std::env::split_paths(pv) {
            if !entry.as_os_str().is_empty() && !base_dirs.contains(&entry) {
                base_dirs.push(entry);
            }
        }
    }

    for binary_name in ["herdr-core", "hc"] {
        for dir in &base_dirs {
            let candidate = dir.join(binary_name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Ensures `herdr-core` (`hc`) is installed locally, running the official install script if missing.
pub async fn ensure_or_install_hc_binary() -> Result<PathBuf> {
    if let Some(existing) = discover_hc_binary() {
        return Ok(existing);
    }
    if std::env::var("CODA_SKIP_HC_INSTALL").as_deref() == Ok("1") {
        bail!("herdr-core binary not found and CODA_SKIP_HC_INSTALL=1 is set");
    }

    tracing::info!("`herdr-core` binary not found; installing via https://releases.herdr.dev/core/install.sh");
    let output = Command::new("sh")
        .arg("-c")
        .arg("curl -fsSL https://releases.herdr.dev/core/install.sh | sh")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("failed to invoke shell installer for herdr-core")?;

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "herdr-core installation exited with {}: stdout=`{}` stderr=`{}`",
            output.status,
            stdout.trim(),
            stderr.trim()
        );
    }

    discover_hc_binary().ok_or_else(|| {
        anyhow!(
            "herdr-core installer finished, but `herdr-core`/`hc` was still not found in standard paths"
        )
    })
}

pub struct HerdrClient {
    hc_path: PathBuf,
    stdin_tx: mpsc::Sender<String>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>,
    next_id: AtomicU64,
    hello_info: HerdrHelloInfo,
    _child: Arc<Mutex<Child>>,
}

static HC_NAME_SEQ: AtomicU64 = AtomicU64::new(1);

/// Sanitizes an arbitrary label into a valid `herdr-core` terminal name (`1..=32` chars of `[a-z0-9._-]`).
pub fn sanitize_hc_terminal_name(raw: &str) -> Option<String> {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in raw.chars() {
        let lower = ch.to_ascii_lowercase();
        if matches!(lower, 'a'..='z' | '0'..='9' | '_' | '.') {
            out.push(lower);
            prev_dash = false;
        } else if (lower == '-' || !prev_dash) && !out.is_empty() && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
        if out.len() >= 32 {
            break;
        }
    }
    let trimmed = out.trim_matches(|c| matches!(c, '-' | '_' | '.')).to_string();
    if trimmed.is_empty() {
        None
    } else {
        let mut cut: String = trimmed.chars().take(32).collect();
        cut = cut
            .trim_matches(|c| matches!(c, '-' | '_' | '.'))
            .to_string();
        if cut.is_empty() { None } else { Some(cut) }
    }
}

fn next_unique_hc_name(prefix: Option<&str>) -> String {
    let seq = HC_NAME_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let suffix = format!(
        "{:06x}",
        (nanos ^ seq.wrapping_mul(0x9e37_79b9)) & 0xff_ffff
    );
    let clean_prefix = prefix
        .and_then(sanitize_hc_terminal_name)
        .unwrap_or_else(|| "coda".to_string());
    let mut base: String = clean_prefix.chars().take(23).collect();
    base = base
        .trim_end_matches(|c| matches!(c, '-' | '_' | '.'))
        .to_string();
    if base.is_empty() {
        base = "coda".to_string();
    }
    format!("{base}-{suffix}")
}

impl HerdrClient {
    pub async fn connect_default() -> Result<Self> {
        let hc = discover_hc_binary()
            .ok_or_else(|| anyhow!("`hc` (herdr-core) binary not found; install from https://releases.herdr.dev/core/install.sh"))?;
        Self::connect_with_bin(hc).await
    }

    pub async fn connect_with_bin(hc_path: impl AsRef<Path>) -> Result<Self> {
        let hc_path = hc_path.as_ref().to_path_buf();
        let mut child = Command::new(&hc_path)
            .arg("connect")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("failed to spawn `{} connect`", hc_path.display()))?;

        let mut stdin = child.stdin.take().context("missing child stdin")?;
        let stdout = child.stdout.take().context("missing child stdout")?;

        let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(256);
        tokio::spawn(async move {
            while let Some(mut line) = stdin_rx.recv().await {
                if !line.ends_with('\n') {
                    line.push('\n');
                }
                if stdin.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
                let _ = stdin.flush().await;
            }
        });

        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let pending_reader = Arc::clone(&pending);

        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    if let Some(id) = v.get("id").and_then(|x| x.as_u64()) {
                        if let Some(waiter) = pending_reader.lock().await.remove(&id) {
                            if let Some(err) = v.get("error") {
                                let _ = waiter.send(Err(anyhow!("herdr rpc error: {err}")));
                            } else {
                                let _ = waiter.send(Ok(v.get("result").cloned().unwrap_or(Value::Null)));
                            }
                        }
                    } else {
                        debug!("herdr notification: {line}");
                    }
                }
            }
        });

        let mut temp = Self {
            hc_path,
            stdin_tx,
            pending,
            next_id: AtomicU64::new(1),
            hello_info: HerdrHelloInfo {
                core_id: String::new(),
                core_name: String::new(),
                version: String::new(),
                generation: 1,
            },
            _child: Arc::new(Mutex::new(child)),
        };

        let res = temp
            .call_rpc("hello", json!({ "framed": false }))
            .await
            .context("herdr `hello` handshake failed")?;

        temp.hello_info = HerdrHelloInfo {
            core_id: res["core"]["id"].as_str().unwrap_or("unknown").to_string(),
            core_name: res["core"]["name"].as_str().unwrap_or("localhost").to_string(),
            version: res["version"].as_str().unwrap_or("0.1").to_string(),
            generation: res["generation"].as_u64().unwrap_or(1) as u32,
        };

        Ok(temp)
    }

    pub fn info(&self) -> &HerdrHelloInfo {
        &self.hello_info
    }

    pub fn hc_path(&self) -> &Path {
        &self.hc_path
    }

    pub async fn call_rpc(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let payload = serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))?;

        if self.stdin_tx.send(payload).await.is_err() {
            self.pending.lock().await.remove(&id);
            bail!("herdr connection closed");
        }

        tokio::time::timeout(std::time::Duration::from_secs(15), rx)
            .await
            .context("herdr rpc timeout")?
            .context("herdr waiter cancelled")?
    }

    pub async fn list_terminals(&self, include_finished: bool) -> Result<Vec<HerdrTerminalSummary>> {
        let res = self
            .call_rpc("terminal.list", json!({ "all": include_finished }))
            .await?;
        let raw_list: Vec<RawTerminal> =
            serde_json::from_value(res.get("terminals").cloned().unwrap_or_else(|| json!([])))?;
        Ok(raw_list.into_iter().map(Into::into).collect())
    }

    /// Launches an AI Coding Agent (`claude`, `codex`, `grok`, `gemini`) or shell inside `herdr-core`
    /// with `outlive_parent: true` and `keep: true` so sessions survive network/daemon restarts.
    pub async fn launch_agent(
        &self,
        agent: &str,
        name: Option<&str>,
        cwd: Option<&str>,
        initial_prompt: Option<&str>,
        cols: u16,
        rows: u16,
    ) -> Result<HerdrTerminalSummary> {
        let mut argv: Vec<String> = match agent {
            "claude" | "claude-code" => vec!["claude".into()],
            "codex" => vec!["codex".into()],
            "grok" | "grok-build" => vec!["grok".into()],
            "gemini" | "gemini-cli" => vec!["gemini".into()],
            "shell" | "" => vec![],
            other => vec![other.to_string()],
        };
        if let Some(prompt) = initial_prompt.filter(|s| !s.trim().is_empty()) {
            if !argv.is_empty() {
                argv.push(prompt.to_string());
            }
        }

        let mut params = json!({
            "argv": argv,
            "keep": true,
            "outlive_parent": true,
            "no_parent": true,
            "wait": false,
            "size": {
                "cols": cols.max(40),
                "rows": rows.max(12),
            },
            "labels": {
                "managed_by": "coda",
                "agent": agent,
            }
        });
        if let Some(n) = name.and_then(sanitize_hc_terminal_name) {
            params["name"] = json!(n);
        }
        if let Some(dir) = cwd.filter(|s| !s.trim().is_empty()) {
            params["cwd"] = json!(dir);
        }

        let res = match self.call_rpc("terminal.run", params.clone()).await {
            Ok(v) => v,
            Err(err) => {
                let msg = err.to_string().to_ascii_lowercase();
                if msg.contains("name")
                    || msg.contains("taken")
                    || msg.contains("exist")
                    || msg.contains("unique")
                    || msg.contains("invalid")
                {
                    let fallback_name = next_unique_hc_name(name.or(Some(agent)));
                    let mut retry_params = params;
                    retry_params["name"] = json!(fallback_name);
                    self.call_rpc("terminal.run", retry_params).await?
                } else {
                    return Err(err);
                }
            }
        };
        let raw: RawTerminal = serde_json::from_value(res)?;
        Ok(raw.into())
    }

    pub async fn read_screen(
        &self,
        terminal: &str,
        history_lines: u32,
    ) -> Result<(String, Option<HerdrAgentStatus>)> {
        let screen_res = self
            .call_rpc(
                "terminal.read",
                json!({
                    "terminal": terminal,
                    "lines": history_lines,
                }),
            )
            .await?;
        let text = screen_res["text"].as_str().unwrap_or("").to_string();

        let status = match self.call_rpc("terminal.get", json!({ "terminal": terminal })).await {
            Ok(term_val) => serde_json::from_value::<RawTerminal>(term_val)
                .ok()
                .and_then(|t| HerdrTerminalSummary::from(t).status),
            Err(_) => None,
        };
        Ok((text, status))
    }

    pub async fn send_text(&self, terminal: &str, text: &str, force: bool) -> Result<()> {
        let _ = self
            .call_rpc(
                "terminal.send",
                json!({
                    "terminal": terminal,
                    "text": text,
                    "force": force,
                    "wait": false,
                }),
            )
            .await?;
        Ok(())
    }

    pub async fn send_keys(&self, terminal: &str, keys: &[String]) -> Result<()> {
        let _ = self
            .call_rpc(
                "terminal.keys",
                json!({
                    "terminal": terminal,
                    "keys": keys,
                }),
            )
            .await?;
        Ok(())
    }

    pub async fn interrupt(&self, terminal: &str) -> Result<()> {
        let _ = self
            .call_rpc("terminal.interrupt", json!({ "terminal": terminal }))
            .await?;
        Ok(())
    }

    pub async fn kill_and_remove(&self, terminal: &str) -> Result<()> {
        // Clear `keep: true` first so `herdr-core` does not auto-respawn a shell on kill.
        let _ = self
            .call_rpc(
                "terminal.keep",
                json!({ "terminal": terminal, "keep": false }),
            )
            .await;
        let _ = self
            .call_rpc("terminal.kill", json!({ "terminal": terminal }))
            .await;

        for _ in 0..15 {
            if self
                .call_rpc("terminal.rm", json!({ "terminal": terminal }))
                .await
                .is_ok()
            {
                return Ok(());
            }
            match self
                .call_rpc("terminal.get", json!({ "terminal": terminal }))
                .await
            {
                Err(_) => return Ok(()),
                Ok(val) if val.get("state").and_then(|v| v.as_str()) == Some("finished") => {
                    let _ = self
                        .call_rpc("terminal.rm", json!({ "terminal": terminal }))
                        .await;
                    return Ok(());
                }
                Ok(_) => {}
            }
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        }
        Ok(())
    }

    /// Uploads a file or screenshot (up to 8 MiB) into the remote terminal workspace via `hc terminal.put`.
    /// Returns `(remote_path, bytes_written)`.
    pub async fn put_file(
        &self,
        terminal: &str,
        filename: &str,
        data_b64: &str,
    ) -> Result<(String, usize)> {
        let res = self
            .call_rpc(
                "terminal.put",
                json!({
                    "terminal": terminal,
                    "name": filename,
                    "data": data_b64,
                }),
            )
            .await?;
        let remote_path = res["path"]
            .as_str()
            .ok_or_else(|| anyhow!("terminal.put response missing path"))?
            .to_string();
        let bytes = res["bytes"].as_u64().unwrap_or(0) as usize;
        Ok((remote_path, bytes))
    }

    /// Stores a JSON document in `herdr-core` `doc.set` (up to 1 MiB) using optimistic CAS version.
    pub async fn doc_set(&self, name: &str, value: Value) -> Result<()> {
        let cur_ver = match self.call_rpc("doc.get", json!({ "name": name })).await {
            Ok(res) => res["version"].as_u64().unwrap_or(0),
            Err(_) => 0,
        };
        let _ = self
            .call_rpc(
                "doc.set",
                json!({
                    "name": name,
                    "version": cur_ver,
                    "value": value,
                }),
            )
            .await?;
        Ok(())
    }

    /// Retrieves a JSON document from `herdr-core` `doc.get`.
    pub async fn doc_get(&self, name: &str) -> Result<Option<Value>> {
        match self.call_rpc("doc.get", json!({ "name": name })).await {
            Ok(res) => {
                if let Some(v) = res.get("value") {
                    if v.is_null() {
                        Ok(None)
                    } else {
                        Ok(Some(v.clone()))
                    }
                } else if res.is_null() {
                    Ok(None)
                } else {
                    Ok(Some(res))
                }
            }
            Err(_) => Ok(None),
        }
    }

    pub async fn set_voice_config(&self, cfg: &VoiceModelConfig) -> Result<()> {
        self.doc_set("coda-voice-config", serde_json::to_value(cfg)?)
            .await
    }

    pub async fn get_voice_config(&self) -> Result<Option<VoiceModelConfig>> {
        if let Some(val) = self.doc_get("coda-voice-config").await? {
            if let Ok(cfg) = serde_json::from_value::<VoiceModelConfig>(val) {
                return Ok(Some(cfg));
            }
        }
        Ok(None)
    }

    /// Spawns a dedicated plain-mode `hc connect` subscriber streaming real-time `events.subscribe`
    /// notifications (`status`, `created`, `finished`, `title`, `size`, etc.).
    pub async fn subscribe_live_events(&self) -> Result<mpsc::Receiver<Value>> {
        let mut child = Command::new(&self.hc_path)
            .arg("connect")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;

        let mut stdin = child.stdin.take().context("missing sub stdin")?;
        let stdout = child.stdout.take().context("missing sub stdout")?;

        let init = format!(
            "{}\n{}\n",
            serde_json::to_string(&json!({"jsonrpc":"2.0","id":1,"method":"hello","params":{"framed":false}}))?,
            serde_json::to_string(&json!({"jsonrpc":"2.0","id":2,"method":"events.subscribe","params":{}}))?
        );
        stdin.write_all(init.as_bytes()).await?;
        stdin.flush().await?;

        let (tx, rx) = mpsc::channel(256);
        tokio::spawn(async move {
            let _keep_child = child;
            let _keep_stdin = stdin;
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(val) = serde_json::from_str::<Value>(&line) {
                    if val.get("method").and_then(|m| m.as_str()) == Some("event") {
                        if let Some(params) = val.get("params") {
                            if tx.send(params.clone()).await.is_err() {
                                break;
                            }
                        }
                    }
                } else {
                    warn!("unparsed herdr event line");
                }
            }
        });

        Ok(rx)
    }
}

// ============================================================================
// 2. Full-Duplex Unix PTY Bridge (`hc attach <terminal>`)
// ============================================================================

#[derive(Debug, Clone)]
pub struct PtyOutputEvent {
    pub terminal: String,
    pub seq: u64,
    pub data_b64: String,
}

pub struct PtyBridgeHandle {
    pub terminal: String,
    master_fd: std::os::unix::io::RawFd,
    _child_pid: u32,
}

impl Drop for PtyBridgeHandle {
    fn drop(&mut self) {
        unsafe {
            // Send 0x1c (Ctrl+\) so `hc attach` detaches cleanly without killing the terminal
            let detach_byte: [u8; 1] = [0x1c];
            let _ = libc::write(
                self.master_fd,
                detach_byte.as_ptr() as *const libc::c_void,
                1,
            );
            libc::close(self.master_fd);
        }
    }
}

impl PtyBridgeHandle {
    /// Writes raw bytes (keystrokes, Kitty Keyboard sequences, bracketed paste) into the attached PTY.
    pub fn write_raw(&self, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let n = unsafe {
            libc::write(
                self.master_fd,
                bytes.as_ptr() as *const libc::c_void,
                bytes.len(),
            )
        };
        if n < 0 {
            bail!("PTY write failed: {}", std::io::Error::last_os_error());
        }
        Ok(())
    }

    /// Resizes the PTY window via `TIOCSWINSZ`, triggering instant reflow in `herdr-core` (`libghostty-vt`).
    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        let ws = libc::winsize {
            ws_row: rows.max(8),
            ws_col: cols.max(20),
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let rc = unsafe { libc::ioctl(self.master_fd, libc::TIOCSWINSZ, &ws) };
        if rc < 0 {
            bail!(
                "PTY TIOCSWINSZ resize failed: {}",
                std::io::Error::last_os_error()
            );
        }
        Ok(())
    }
}

/// Spawns `hc attach <terminal>` inside a real Unix pseudo-terminal (`openpty`),
/// streaming raw 24-bit ANSI/VT output chunks asynchronously into `out_tx`.
pub fn spawn_pty_attach(
    hc_path: &Path,
    terminal: &str,
    cols: u16,
    rows: u16,
    out_tx: mpsc::Sender<PtyOutputEvent>,
) -> Result<PtyBridgeHandle> {
    use std::os::unix::io::FromRawFd;

    let mut master_fd: libc::c_int = -1;
    let mut slave_fd: libc::c_int = -1;
    let mut ws = libc::winsize {
        ws_row: rows.max(8),
        ws_col: cols.max(20),
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    let rc = unsafe {
        libc::openpty(
            &mut master_fd,
            &mut slave_fd,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut ws,
        )
    };
    if rc != 0 {
        bail!("openpty failed: {}", std::io::Error::last_os_error());
    }

    let slave_in = unsafe { Stdio::from_raw_fd(slave_fd) };
    let slave_out = unsafe { Stdio::from_raw_fd(libc::dup(slave_fd)) };
    let slave_err = unsafe { Stdio::from_raw_fd(libc::dup(slave_fd)) };

    let mut cmd = std::process::Command::new(hc_path);
    cmd.args(["attach", terminal])
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor")
        .stdin(slave_in)
        .stdout(slave_out)
        .stderr(slave_err);

    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to spawn `hc attach {terminal}` in PTY"))?;
    let child_pid = child.id();

    let reader_fd = unsafe { libc::dup(master_fd) };
    let term_name = terminal.to_string();

    std::thread::spawn(move || {
        let mut seq = 1u64;
        let mut buf = [0u8; 8192];
        loop {
            let n = unsafe {
                libc::read(
                    reader_fd,
                    buf.as_mut_ptr() as *mut libc::c_void,
                    buf.len(),
                )
            };
            if n <= 0 {
                break;
            }
            let chunk_b64 = B64.encode(&buf[..n as usize]);
            if out_tx
                .blocking_send(PtyOutputEvent {
                    terminal: term_name.clone(),
                    seq,
                    data_b64: chunk_b64,
                })
                .is_err()
            {
                break;
            }
            seq += 1;
        }
        unsafe {
            libc::close(reader_fd);
        }
        let _ = child.wait();
    });

    Ok(PtyBridgeHandle {
        terminal: terminal.to_string(),
        master_fd,
        _child_pid: child_pid,
    })
}

// ============================================================================
// 3. Git Repository, Worktree & Diff Stat Scanner (`+added / -deleted`)
// ============================================================================

/// Scans candidate directories and active `herdr` terminal working directories
/// to build the grouped `RepoWorkspaceSummary` tree for the Left Sidebar.
pub async fn scan_git_workspaces(
    machine_name: &str,
    root_dirs: &[String],
    terminals: &[HerdrTerminalSummary],
) -> Vec<RepoWorkspaceSummary> {
    let mut candidate_dirs: Vec<PathBuf> = Vec::new();
    for r in root_dirs {
        let p = PathBuf::from(r);
        if p.is_dir() {
            candidate_dirs.push(p);
        }
    }
    if let Ok(env_roots) = std::env::var("CODA_WORKSPACE_ROOTS") {
        for r in env_roots.split(':').filter(|s| !s.trim().is_empty()) {
            let p = PathBuf::from(r.trim());
            if p.is_dir() {
                candidate_dirs.push(p);
            }
        }
    }
    for t in terminals {
        if !t.cwd.is_empty() {
            let p = PathBuf::from(&t.cwd);
            if p.is_dir() {
                candidate_dirs.push(p);
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        if cwd.is_dir() {
            candidate_dirs.push(cwd);
        }
    }
    collect_home_git_candidates(&mut candidate_dirs, 16);

    let mut seen_repo_roots: HashMap<String, RepoWorkspaceSummary> = HashMap::new();

    for dir in candidate_dirs {
        let Some(toplevel) = git_capture(&dir, &["rev-parse", "--show-toplevel"]).await else {
            continue;
        };
        let toplevel = toplevel.trim().to_string();
        if toplevel.is_empty() || seen_repo_roots.contains_key(&toplevel) {
            continue;
        }

        let repo_path = PathBuf::from(&toplevel);
        let repo_name = repo_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("workspace")
            .to_string();

        // Parse `git worktree list --porcelain`
        let worktree_out = git_capture(&repo_path, &["worktree", "list", "--porcelain"])
            .await
            .unwrap_or_default();

        let mut worktrees: Vec<(String, String)> = Vec::new();
        let mut cur_path: Option<String> = None;
        let mut cur_branch: Option<String> = None;

        for line in worktree_out.lines() {
            if let Some(wt) = line.strip_prefix("worktree ") {
                if let Some(prev_p) = cur_path.take() {
                    worktrees.push((
                        prev_p,
                        cur_branch.take().unwrap_or_else(|| "HEAD".into()),
                    ));
                }
                cur_path = Some(wt.trim().to_string());
            } else if let Some(b) = line.strip_prefix("branch ") {
                let short = b.trim().strip_prefix("refs/heads/").unwrap_or(b.trim());
                cur_branch = Some(short.to_string());
            }
        }
        if let Some(prev_p) = cur_path.take() {
            worktrees.push((prev_p, cur_branch.take().unwrap_or_else(|| "main".into())));
        }
        if worktrees.is_empty() {
            let b = git_capture(&repo_path, &["rev-parse", "--abbrev-ref", "HEAD"])
                .await
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|| "main".into());
            worktrees.push((toplevel.clone(), b));
        }

        let mut branches = Vec::new();
        for (wt_path, branch_name) in worktrees {
            let (added, deleted) = git_diff_numstat(Path::new(&wt_path)).await;
            let active_terminals: Vec<HerdrTerminalSummary> = terminals
                .iter()
                .filter(|t| t.alive && (t.cwd == wt_path || t.cwd.starts_with(&format!("{wt_path}/"))))
                .cloned()
                .collect();

            let status_summary = compute_branch_status_summary(&active_terminals);
            branches.push(BranchWorktreeItem {
                branch: branch_name,
                worktree_path: wt_path,
                diff_added: added,
                diff_deleted: deleted,
                active_terminals,
                status_summary,
            });
        }

        seen_repo_roots.insert(
            toplevel.clone(),
            RepoWorkspaceSummary {
                repo_name,
                root_path: toplevel,
                machine_name: machine_name.to_string(),
                branches,
            },
        );
    }

    let mut list: Vec<RepoWorkspaceSummary> = seen_repo_roots.into_values().collect();
    list.sort_by(|a, b| a.repo_name.cmp(&b.repo_name));
    list
}

fn collect_home_git_candidates(candidate_dirs: &mut Vec<PathBuf>, max_total: usize) {
    let Ok(home) = std::env::var("HOME") else {
        return;
    };
    if home.trim().is_empty() {
        return;
    }
    let home_path = PathBuf::from(home);
    let subdirs = [
        "",
        "projects",
        "Projects",
        "workspace",
        "Developer",
        "Code",
        "code",
        "github",
        "personal",
        "personal/github",
        "repos",
        "src",
    ];
    for sub in subdirs {
        if candidate_dirs.len() >= max_total {
            break;
        }
        let base = if sub.is_empty() {
            home_path.clone()
        } else {
            home_path.join(sub)
        };
        if base.is_dir() {
            scan_dir_git_roots(&base, 0, 2, candidate_dirs, max_total);
        }
    }
}

fn scan_dir_git_roots(
    dir: &Path,
    depth: usize,
    max_depth: usize,
    out: &mut Vec<PathBuf>,
    max_total: usize,
) {
    if out.len() >= max_total {
        return;
    }
    if dir.join(".git").exists() {
        let canonical = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        if !out.contains(&canonical) {
            out.push(canonical);
        }
        return;
    }
    if depth >= max_depth {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name();
            let s = name.to_string_lossy();
            let p = e.path();
            if !s.starts_with('.') && s != "node_modules" && s != "target" && p.is_dir() {
                Some(p)
            } else {
                None
            }
        })
        .collect();
    dirs.sort();
    for d in dirs {
        scan_dir_git_roots(&d, depth + 1, max_depth, out, max_total);
        if out.len() >= max_total {
            break;
        }
    }
}

fn compute_branch_status_summary(terminals: &[HerdrTerminalSummary]) -> String {
    if terminals.is_empty() {
        return "暂无终端".into();
    }
    let working_count = terminals
        .iter()
        .filter(|t| {
            t.status
                .as_ref()
                .map(|s| s.state == "working" || s.state == "blocked")
                .unwrap_or(false)
        })
        .count();
    let done_count = terminals
        .iter()
        .filter(|t| t.status.as_ref().map(|s| s.state == "done").unwrap_or(false))
        .count();

    if working_count > 0 {
        format!("{} 个终端运行中", terminals.len())
    } else if done_count > 0 {
        "本轮完成".into()
    } else {
        format!("{} 个终端 · 空闲", terminals.len())
    }
}

async fn git_capture(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

async fn git_diff_numstat(cwd: &Path) -> (u32, u32) {
    let Some(out) = git_capture(cwd, &["diff", "--numstat", "HEAD"]).await else {
        return (0, 0);
    };
    let mut added = 0u32;
    let mut deleted = 0u32;
    for line in out.lines() {
        let mut parts = line.split_whitespace();
        if let (Some(a), Some(d)) = (parts.next(), parts.next()) {
            added += a.parse::<u32>().unwrap_or(0);
            deleted += d.parse::<u32>().unwrap_or(0);
        }
    }
    (added, deleted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_live_herdr_core_handshake_and_ephemeral_terminal() {
        let Some(hc_bin) = discover_hc_binary() else {
            eprintln!("skipping live hc test: hc not installed");
            return;
        };
        let client = HerdrClient::connect_with_bin(hc_bin).await.unwrap();
        assert!(!client.info().core_id.is_empty());

        // Spawn a quick shell command in herdr-core, read its screen, and clean it up
        let unique_name = format!("coda-test-{}", std::process::id());
        let term = client
            .launch_agent("shell", Some(&unique_name), None, None, 100, 30)
            .await
            .unwrap();
        assert!(term.alive);

        client
            .send_text(&term.address, "echo CODA_HERDR_OK", true)
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(350)).await;

        let (screen, _) = client.read_screen(&term.address, 20).await.unwrap();
        assert!(
            screen.contains("CODA_HERDR_OK"),
            "expected screen output to contain marker, got: {screen}"
        );

        // Test terminal.put file upload
        let sample_b64 = B64.encode(b"Coda E2EE Attachment Payload");
        let (remote_path, written) = client
            .put_file(&term.address, "coda-attach.txt", &sample_b64)
            .await
            .unwrap();
        assert!(remote_path.contains("coda-attach.txt"));
        assert_eq!(written, 28);

        // Test doc.set / doc.get voice config persistence
        let vcfg = VoiceModelConfig {
            provider: "openai".into(),
            base_url: "https://api.openai.com/v1".into(),
            api_key: "sk-test-voice".into(),
            model: "gpt-4o-mini-transcribe".into(),
        };
        client.set_voice_config(&vcfg).await.unwrap();
        let loaded = client.get_voice_config().await.unwrap().unwrap();
        assert_eq!(loaded.provider, "openai");
        assert_eq!(loaded.model, "gpt-4o-mini-transcribe");

        // Test Full-Duplex PTY Attach (`hc attach`) streaming + resize + clean detach
        let (pty_tx, mut pty_rx) = mpsc::channel(64);
        let pty = spawn_pty_attach(client.hc_path(), &term.address, 96, 28, pty_tx).unwrap();
        pty.resize(120, 36).unwrap();
        pty.write_raw(b"echo PTY_FULL_DUPLEX_OK\r").unwrap();

        let mut collected = String::new();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            if let Ok(Some(ev)) =
                tokio::time::timeout(std::time::Duration::from_millis(250), pty_rx.recv()).await
            {
                let bytes = B64.decode(&ev.data_b64).unwrap_or_default();
                collected.push_str(&String::from_utf8_lossy(&bytes));
                if collected.contains("PTY_FULL_DUPLEX_OK") {
                    break;
                }
            }
        }
        assert!(
            collected.contains("PTY_FULL_DUPLEX_OK"),
            "expected live PTY stream to contain marker, got len={}",
            collected.len()
        );
        drop(pty); // Sends 0x1c Ctrl+\ to detach cleanly without killing terminal

        // Verify Git workspace & worktree scanner
        let terms = client.list_terminals(false).await.unwrap();
        let repos = scan_git_workspaces("Home", &[], &terms).await;
        assert!(!repos.is_empty(), "expected at least current git repository in scan");

        client.kill_and_remove(&term.address).await.unwrap();
    }

    #[test]
    fn test_discover_prefers_herdr_core_over_hc() {
        let dir = std::env::temp_dir().join(format!("coda-herdr-test-{}", std::process::id()));
        let bin_dir = dir.join(".local/bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let hc_path = bin_dir.join("hc");
        std::fs::write(&hc_path, "#!/bin/sh\n").unwrap();
        let herdr_path = bin_dir.join("herdr-core");
        std::fs::write(&herdr_path, "#!/bin/sh\n").unwrap();

        let found = discover_hc_binary_in(Some(&dir), Some(std::ffi::OsStr::new("")));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(found.as_deref(), Some(herdr_path.as_path()));
    }

    #[tokio::test]
    async fn test_launch_with_spaces_unicode_duplicate_names_and_clean_kill_remove() {
        assert_eq!(sanitize_hc_terminal_name("coda · hc").as_deref(), Some("coda-hc"));
        assert_eq!(
            sanitize_hc_terminal_name("  My Space / Claude:v2  ").as_deref(),
            Some("my-space-claude-v2")
        );
        assert_eq!(sanitize_hc_terminal_name("···"), None);

        let Some(hc_bin) = discover_hc_binary() else {
            eprintln!("skipping live duplicate/kill test: hc not installed");
            return;
        };
        let client = HerdrClient::connect_with_bin(hc_bin).await.unwrap();

        let term1 = client
            .launch_agent("shell", Some("coda · hc"), None, None, 100, 28)
            .await
            .expect("first launch with spaces+unicode name must succeed");
        let term2 = client
            .launch_agent("shell", Some("coda · hc"), None, None, 100, 28)
            .await
            .expect("second launch with duplicate sanitized name must auto-fallback and succeed");

        assert_ne!(term1.address, term2.address);
        assert!(term1.alive && term2.alive);

        client
            .send_text(&term2.address, "echo CODA_DUP_LAUNCH_OK", true)
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let (scr, _) = client.read_screen(&term2.address, 20).await.unwrap();
        assert!(
            scr.contains("CODA_DUP_LAUNCH_OK"),
            "expected output in duplicate-named shell, got: {scr}"
        );

        client.kill_and_remove(&term1.address).await.unwrap();
        client.kill_and_remove(&term2.address).await.unwrap();

        let remaining = client.list_terminals(true).await.unwrap();
        assert!(
            !remaining
                .iter()
                .any(|t| t.address == term1.address || t.address == term2.address),
            "killed+removed terminals must not remain in list_terminals(true): {:?}",
            remaining.iter().map(|t| &t.address).collect::<Vec<_>>()
        );
    }
}
