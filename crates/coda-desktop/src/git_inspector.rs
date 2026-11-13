//! Real local Git repository inspector (`status`, `diff`, `log`) and workspace file browser.
//! All commands execute on background worker threads and never block the GPUI render loop.

use std::path::{Path, PathBuf};
use std::process::Command;

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
}

#[derive(Debug, Clone)]
pub struct WorkspaceFileEntry {
    pub name: String,
    pub full_path: String,
    pub rel_path: String,
    pub is_dir: bool,
    pub size_bytes: u64,
}

fn run_git(repo_path: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(args)
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
        return RepoInspectorSnapshot {
            repo_path: repo_path.to_string_lossy().to_string(),
            repo_name,
            diff_title: "Directory not found".into(),
            diff_text: format!("Path does not exist: {}", repo_path.display()),
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
        if line.len() < 4 {
            continue;
        }
        let code = line[..2].trim().to_string();
        let raw_path = line[3..].trim();
        let path = raw_path
            .rsplit(" -> ")
            .next()
            .unwrap_or(raw_path)
            .trim_matches('"')
            .to_string();
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
        if let Some(first_file) = changed_files.first() {
            Some(format!("file:{}", first_file.path))
        } else {
            recent_commits
                .first()
                .map(|c| format!("commit:{}", c.short_sha))
        }
    });

    let (diff_title, diff_text) = match selected_item.as_deref() {
        Some(sel) if sel.starts_with("file:") => {
            let rel = &sel["file:".len()..];
            let mut patch = run_git(repo_path, &["diff", "HEAD", "--", rel]).unwrap_or_default();
            if patch.trim().is_empty() {
                patch = run_git(repo_path, &["diff", "--cached", "--", rel]).unwrap_or_default();
            }
            if patch.trim().is_empty() {
                // Untracked or binary fallback: read plain file content prefixed as additions.
                let abs = repo_path.join(rel);
                patch = read_untracked_diff_preview(&abs, rel);
            }
            (format!("Working Tree: {rel}"), truncate_lines(&patch, 400))
        }
        Some(sel) if sel.starts_with("commit:") => {
            let sha = &sel["commit:".len()..];
            let show = run_git(repo_path, &["show", "--stat", "--patch", "--max-count=1", sha])
                .unwrap_or_else(|| "Unable to load commit diff.".into());
            (format!("Commit {sha}"), truncate_lines(&show, 400))
        }
        _ => (
            format!("{repo_name} ({branch})"),
            "Working tree is clean and no commits were found yet.".into(),
        ),
    };

    RepoInspectorSnapshot {
        repo_path: repo_path.to_string_lossy().to_string(),
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
    let Ok(bytes) = std::fs::read(path) else {
        return format!("(Cannot read file: {rel})");
    };
    let sample = &bytes[..bytes.len().min(4096)];
    if sample.contains(&0) {
        return format!("Binary file ({} bytes): {rel}", bytes.len());
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
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut items: Vec<WorkspaceFileEntry> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name == ".git" || name == ".DS_Store" || name == "target" || name == "node_modules" {
                return None;
            }
            let full = e.path();
            let meta = e.metadata().ok();
            let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or_else(|| full.is_dir());
            let size_bytes = meta.map(|m| m.len()).unwrap_or(0);
            let rel_path = full
                .strip_prefix(repo_root)
                .unwrap_or(&full)
                .to_string_lossy()
                .to_string();
            Some(WorkspaceFileEntry {
                name,
                full_path: full.to_string_lossy().to_string(),
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

/// Reads up to `max_lines` of a UTF-8 text file for previewing inside the `Files` inspector tab.
pub fn read_workspace_file_preview(path: &Path, max_lines: usize) -> String {
    let Ok(bytes) = std::fs::read(path) else {
        return format!("Unable to read {}", path.display());
    };
    let sample = &bytes[..bytes.len().min(4096)];
    if sample.contains(&0) {
        return format!("Binary file ({} bytes)", bytes.len());
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut numbered = Vec::new();
    let mut total_truncated = false;
    for (idx, line) in text.lines().enumerate() {
        if idx >= max_lines {
            total_truncated = true;
            break;
        }
        numbered.push(format!("{:>4} | {}", idx + 1, line));
    }
    let mut out = numbered.join("\n");
    if total_truncated {
        out.push_str(&format!("\n... (truncated at {max_lines} lines)"));
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
