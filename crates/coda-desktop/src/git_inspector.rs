//! Real local Git repository inspector (`status`, `diff`, `log`) and workspace file browser.
//! All commands execute on background worker threads and never block the GPUI render loop.

use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

const MAX_PREVIEW_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, Default)]
pub struct ChangedFileEntry {
    pub status: String,
    pub path: String,
}

#[derive(Debug, Clone, Default)]
pub struct CommitEntry {
    pub short_sha: String,
    pub rel_time: String,
    pub author: String,
    pub subject: String,
}

#[derive(Debug, Clone, Default)]
pub struct RepoInspectorSnapshot {
    pub repo_path: String,
    pub repo_root: Option<String>,
    pub repo_name: String,
    pub is_git_repo: bool,
    pub branch: String,
    pub head_summary: String,
    pub diff_added: u32,
    pub diff_deleted: u32,
    pub has_cargo_toml: bool,
    pub has_package_json: bool,
    pub changed_files: Vec<ChangedFileEntry>,
    pub recent_commits: Vec<CommitEntry>,
    pub selected_item: Option<String>,
    pub diff_title: String,
    pub diff_text: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct WorkspaceFileEntry {
    pub name: String,
    pub full_path: String,
    pub rel_path: String,
    pub is_dir: bool,
    pub size_bytes: u64,
}

/// Resolves `rel` strictly inside `root` without escaping via `..`, absolute paths, or symlinks.
pub fn resolve_contained_path(root: &Path, rel: &str) -> Option<PathBuf> {
    let trimmed = rel.trim();
    if trimmed.is_empty() {
        return None;
    }
    let rel_path = Path::new(trimmed);
    for comp in rel_path.components() {
        match comp {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    let canon_root = std::fs::canonicalize(root).ok()?;
    let joined = canon_root.join(rel_path);
    let canon_target = std::fs::canonicalize(&joined).ok()?;
    if canon_target.starts_with(&canon_root) {
        Some(canon_target)
    } else {
        None
    }
}

fn read_bounded_preview_bytes(path: &Path, max_bytes: usize) -> std::io::Result<(Vec<u8>, u64, bool)> {
    let f = std::fs::File::open(path)?;
    let total_len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let mut buf = Vec::with_capacity(max_bytes.min(total_len as usize).saturating_add(1));
    f.take((max_bytes + 1) as u64).read_to_end(&mut buf)?;
    let truncated = buf.len() > max_bytes;
    if truncated {
        buf.truncate(max_bytes);
    }
    Ok((buf, total_len, truncated))
}

fn run_git(repo_path: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-c")
        .arg("core.quotePath=false")
        .arg("-c")
        .arg("core.fsmonitor=false")
        .arg("-C")
        .arg(repo_path)
        .args(args)
        .env("PATH", crate::config::augmented_gui_path_string())
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Computes a real Git working tree, commit history, and unified diff snapshot for `repo_path`.
pub fn compute_repo_snapshot(
    repo_path: &Path,
    requested_item: Option<&str>,
) -> RepoInspectorSnapshot {
    let repo_name = repo_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("workspace")
        .to_string();

    if !repo_path.is_dir() {
        let msg = format!("Path does not exist: {}", repo_path.display());
        return RepoInspectorSnapshot {
            repo_path: repo_path.to_string_lossy().to_string(),
            repo_name,
            diff_title: "Directory not found".into(),
            diff_text: msg.clone(),
            error: Some(msg),
            ..Default::default()
        };
    }

    let has_cargo_toml = repo_path.join("Cargo.toml").is_file();
    let has_package_json = repo_path.join("package.json").is_file();

    let Some(branch_raw) = run_git(repo_path, &["rev-parse", "--abbrev-ref", "HEAD"]) else {
        return RepoInspectorSnapshot {
            repo_path: repo_path.to_string_lossy().to_string(),
            repo_name,
            is_git_repo: false,
            has_cargo_toml,
            has_package_json,
            diff_title: "Not a Git repository".into(),
            diff_text: "Open a Git repository or initialize Git here (`git init`) to inspect changes and diffs.".into(),
            ..Default::default()
        };
    };

    let branch = branch_raw.trim().to_string();
    let head_summary = run_git(repo_path, &["log", "-n", "1", "--pretty=format:%h · %s"])
        .unwrap_or_default()
        .trim()
        .to_string();

    let (diff_added, diff_deleted) = compute_numstat(repo_path);

    let status_raw = run_git(repo_path, &["status", "--porcelain=v1", "-uall"]).unwrap_or_default();
    let mut changed_files = Vec::new();
    for line in status_raw.lines() {
        let Some(code_raw) = line.get(..2) else {
            continue;
        };
        let Some(rest) = line.get(3..) else {
            continue;
        };
        let code = code_raw.trim().to_string();
        let raw_path = rest.trim();
        let target_raw = raw_path.rsplit(" -> ").next().unwrap_or(raw_path);
        let path = if target_raw.starts_with('"') && target_raw.ends_with('"') && target_raw.len() >= 2 {
            target_raw[1..target_raw.len() - 1].to_string()
        } else {
            target_raw.to_string()
        };
        changed_files.push(ChangedFileEntry {
            status: if code.is_empty() { "M".into() } else { code },
            path,
        });
    }

    let log_raw = run_git(
        repo_path,
        &["log", "-n", "12", "--pretty=format:%h\t%cr\t%an\t%s"],
    )
    .unwrap_or_default();
    let mut recent_commits = Vec::new();
    for line in log_raw.lines() {
        let mut parts = line.splitn(4, '\t');
        if let (Some(sha), Some(rel), Some(author), Some(subj)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        {
            recent_commits.push(CommitEntry {
                short_sha: sha.to_string(),
                rel_time: rel.to_string(),
                author: author.to_string(),
                subject: subj.to_string(),
            });
        }
    }

    let selected_item = requested_item.map(String::from).or_else(|| {
        if !changed_files.is_empty() {
            Some("all".to_string())
        } else {
            recent_commits
                .first()
                .map(|c| format!("commit:{}", c.short_sha))
        }
    });

    let (diff_title, diff_text) = match selected_item.as_deref() {
        Some("all") => {
            let mut patch = run_git(repo_path, &["diff", "HEAD"]).unwrap_or_default();
            if patch.trim().is_empty() {
                patch = run_git(repo_path, &["diff", "--cached"]).unwrap_or_default();
            }
            if patch.trim().is_empty() {
                patch = run_git(repo_path, &["diff"]).unwrap_or_default();
            }
            if patch.trim().is_empty() {
                let mut combined = String::new();
                for entry in changed_files.iter().take(8) {
                    if let Some(abs) = resolve_contained_path(repo_path, &entry.path) {
                        if !combined.is_empty() {
                            combined.push('\n');
                        }
                        combined.push_str(&read_untracked_diff_preview(&abs, &entry.path));
                    }
                }
                patch = combined;
            }
            (
                format!("Working Tree ({branch})"),
                truncate_lines(&patch, 450),
            )
        }
        Some(sel) if sel.starts_with("file:") => {
            let rel = &sel["file:".len()..];
            let mut patch = run_git(repo_path, &["diff", "HEAD", "--", rel]).unwrap_or_default();
            if patch.trim().is_empty() {
                patch = run_git(repo_path, &["diff", "--cached", "--", rel]).unwrap_or_default();
            }
            if patch.trim().is_empty() {
                // Untracked or binary fallback: safely read bounded preview inside repo root.
                if let Some(abs) = resolve_contained_path(repo_path, rel) {
                    patch = read_untracked_diff_preview(&abs, rel);
                } else {
                    patch = format!("(Blocked path outside workspace: {rel})");
                }
            }
            (format!("Working Tree: {rel}"), truncate_lines(&patch, 400))
        }
        Some(sel) if sel.starts_with("commit:") => {
            let sha = sel["commit:".len()..].trim();
            if sha.is_empty()
                || sha.starts_with('-')
                || sha.len() > 64
                || !sha.chars().all(|c| c.is_ascii_hexdigit())
            {
                (
                    "Invalid commit ref".into(),
                    "Refusing invalid or ambiguous commit selector.".into(),
                )
            } else {
                let show = run_git(
                    repo_path,
                    &[
                        "show",
                        "--stat",
                        "--patch",
                        "--max-count=1",
                        "--end-of-options",
                        sha,
                        "--",
                    ],
                )
                .unwrap_or_else(|| "Unable to load commit diff.".into());
                (format!("Commit {sha}"), truncate_lines(&show, 400))
            }
        }
        _ => (
            format!("{repo_name} ({branch})"),
            "Working tree is clean and no commits were found yet.".into(),
        ),
    };

    let resolved_root = run_git(repo_path, &["rev-parse", "--show-toplevel"])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| repo_path.to_string_lossy().to_string());

    RepoInspectorSnapshot {
        repo_path: repo_path.to_string_lossy().to_string(),
        repo_root: Some(resolved_root),
        repo_name,
        is_git_repo: true,
        branch,
        head_summary,
        diff_added,
        diff_deleted,
        has_cargo_toml,
        has_package_json,
        changed_files,
        recent_commits,
        selected_item,
        diff_title,
        diff_text,
        error: None,
    }
}

fn compute_numstat(repo_path: &Path) -> (u32, u32) {
    let Some(out) = run_git(repo_path, &["diff", "--numstat", "HEAD"]) else {
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

fn read_untracked_diff_preview(path: &Path, rel: &str) -> String {
    let Ok((bytes, total_len, truncated_bytes)) = read_bounded_preview_bytes(path, MAX_PREVIEW_BYTES) else {
        return format!("(Cannot read file: {rel})");
    };
    let sample = &bytes[..bytes.len().min(4096)];
    if sample.contains(&0) {
        return format!("Binary file ({total_len} bytes): {rel}");
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut out = format!("+++ b/{rel} (untracked)\n");
    for (idx, line) in text.lines().take(300).enumerate() {
        out.push('+');
        out.push_str(line);
        out.push('\n');
        if idx == 299 {
            out.push_str("... (truncated at 300 lines)\n");
        }
    }
    if truncated_bytes {
        out.push_str(&format!("... (preview capped at {MAX_PREVIEW_BYTES} bytes)\n"));
    }
    out
}

fn truncate_lines(input: &str, max_lines: usize) -> String {
    let mut lines = Vec::new();
    let mut truncated = false;
    for (i, line) in input.lines().enumerate() {
        if i >= max_lines {
            truncated = true;
            break;
        }
        lines.push(line);
    }
    let mut out = lines.join("\n");
    if truncated {
        out.push_str(&format!("\n... (truncated after {max_lines} lines)"));
    }
    out
}

/// Lists non-hidden entries (plus `.github` / `.env.example` excluding `.git`) inside `dir`.
pub fn list_workspace_dir(repo_root: &Path, dir: &Path) -> Vec<WorkspaceFileEntry> {
    let Some(canon_root) = std::fs::canonicalize(repo_root).ok() else {
        return Vec::new();
    };
    let Ok(canon_dir) = std::fs::canonicalize(dir) else {
        return Vec::new();
    };
    if !canon_dir.starts_with(&canon_root) {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(&canon_dir) else {
        return Vec::new();
    };
    let mut items: Vec<WorkspaceFileEntry> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name == ".git" || name == ".DS_Store" || name == "target" || name == "node_modules" {
                return None;
            }
            let raw_full = e.path();
            let Ok(canon_full) = std::fs::canonicalize(&raw_full) else {
                return None;
            };
            if !canon_full.starts_with(&canon_root) {
                return None;
            }
            let meta = std::fs::metadata(&canon_full).ok();
            let is_dir = meta
                .as_ref()
                .map(|m| m.is_dir())
                .unwrap_or_else(|| canon_full.is_dir());
            let size_bytes = meta.map(|m| m.len()).unwrap_or(0);
            let rel_path = canon_full
                .strip_prefix(&canon_root)
                .ok()?
                .to_string_lossy()
                .to_string();
            Some(WorkspaceFileEntry {
                name,
                full_path: canon_full.to_string_lossy().to_string(),
                rel_path,
                is_dir,
                size_bytes,
            })
        })
        .collect();

    items.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
    items
}

/// Reads up to `max_lines` (and at most `MAX_PREVIEW_BYTES`) of a UTF-8 text file for previewing inside the `Files` inspector tab.
pub fn read_workspace_file_preview(path: &Path, max_lines: usize) -> String {
    let Ok((bytes, total_len, truncated_bytes)) = read_bounded_preview_bytes(path, MAX_PREVIEW_BYTES) else {
        return format!("Unable to read {}", path.display());
    };
    let sample = &bytes[..bytes.len().min(4096)];
    if sample.contains(&0) {
        return format!("Binary file ({total_len} bytes)");
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut lines_out = Vec::new();
    let mut total_truncated = false;
    for (idx, line) in text.lines().enumerate() {
        if idx >= max_lines {
            total_truncated = true;
            break;
        }
        // #41: Return clean raw lines; the UI file preview pane renders the single line-number gutter.
        lines_out.push(line);
    }
    let mut out = lines_out.join("\n");
    if total_truncated {
        out.push_str(&format!("\n... (truncated at {max_lines} lines)"));
    } else if truncated_bytes {
        out.push_str(&format!("\n... (preview capped at {MAX_PREVIEW_BYTES} bytes)"));
    }
    if out.is_empty() {
        "(empty file)".into()
    } else {
        out
    }
}

/// Scans an explicit list of local repository directories (plus any active terminal working dirs)
/// and produces `RepoWorkspaceSummary` rows using git worktrees & numstats.
pub fn scan_local_repo_summaries(
    machine_name: &str,
    repo_roots: &[String],
) -> Vec<coda_protocol::RepoWorkspaceSummary> {
    let mut summaries = Vec::new();
    for root in repo_roots {
        let p = PathBuf::from(root);
        if !p.is_dir() {
            continue;
        }
        let repo_name = p
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("workspace")
            .to_string();

        let mut branches = Vec::new();
        if let Some(wt_out) = run_git(&p, &["worktree", "list", "--porcelain"]) {
            let mut cur_path: Option<String> = None;
            let mut cur_branch: Option<String> = None;
            for line in wt_out.lines() {
                if let Some(wt) = line.strip_prefix("worktree ") {
                    if let Some(prev) = cur_path.take() {
                        let (a, d) = compute_numstat(Path::new(&prev));
                        branches.push(coda_protocol::BranchWorktreeItem {
                            branch: cur_branch.take().unwrap_or_else(|| "HEAD".into()),
                            worktree_path: prev,
                            diff_added: a,
                            diff_deleted: d,
                            active_terminals: Vec::new(),
                            status_summary: String::new(),
                        });
                    }
                    cur_path = Some(wt.trim().to_string());
                } else if let Some(b) = line.strip_prefix("branch ") {
                    let short = b.trim().strip_prefix("refs/heads/").unwrap_or(b.trim());
                    cur_branch = Some(short.to_string());
                }
            }
            if let Some(prev) = cur_path.take() {
                let (a, d) = compute_numstat(Path::new(&prev));
                branches.push(coda_protocol::BranchWorktreeItem {
                    branch: cur_branch.take().unwrap_or_else(|| "HEAD".into()),
                    worktree_path: prev,
                    diff_added: a,
                    diff_deleted: d,
                    active_terminals: Vec::new(),
                    status_summary: String::new(),
                });
            }
        }

        if branches.is_empty() {
            let branch = run_git(&p, &["rev-parse", "--abbrev-ref", "HEAD"])
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|| "folder".into());
            let (a, d) = compute_numstat(&p);
            branches.push(coda_protocol::BranchWorktreeItem {
                branch,
                worktree_path: root.clone(),
                diff_added: a,
                diff_deleted: d,
                active_terminals: Vec::new(),
                status_summary: String::new(),
            });
        }

        summaries.push(coda_protocol::RepoWorkspaceSummary {
            repo_name,
            root_path: root.clone(),
            machine_name: machine_name.to_string(),
            branches,
        });
    }
    summaries
}
