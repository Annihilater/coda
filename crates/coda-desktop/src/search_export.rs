//! Terminal scrollback search (`Cmd+F`), clickable URL extraction (`Cmd+Click`),
//! session transcript Markdown export, and host health badge formatting for `coda-desktop`.

use std::path::PathBuf;

use coda_protocol::HostHealthSnapshot;

use crate::config::write_file_atomic_mode;
use crate::pty::{char_display_width, StyledTerminalLine};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSearchMatch {
    pub line_idx: usize,
    pub byte_start: usize,
    pub byte_end: usize,
}

/// Finds all case-insensitive occurrences of `query` across `lines`, returning valid UTF-8 byte ranges.
pub fn find_terminal_matches(
    lines: &[StyledTerminalLine],
    query: &str,
) -> Vec<TerminalSearchMatch> {
    let needle = query.trim();
    if needle.is_empty() {
        return Vec::new();
    }
    let needle_chars: Vec<char> = needle.chars().flat_map(|c| c.to_lowercase()).collect();
    if needle_chars.is_empty() {
        return Vec::new();
    }

    let mut out = Vec::new();
    for (line_idx, line) in lines.iter().enumerate() {
        if line.text.is_empty() {
            continue;
        }
        // Map each lowered char back to its original UTF-8 `(byte_start, byte_end)` in `line.text`.
        let mut lowered: Vec<(char, usize, usize)> = Vec::with_capacity(line.text.len());
        for (b_idx, ch) in line.text.char_indices() {
            let b_end = b_idx + ch.len_utf8();
            for lc in ch.to_lowercase() {
                lowered.push((lc, b_idx, b_end));
            }
        }
        if lowered.len() < needle_chars.len() {
            continue;
        }
        let mut i = 0usize;
        while i + needle_chars.len() <= lowered.len() {
            let matched = lowered[i..i + needle_chars.len()]
                .iter()
                .zip(needle_chars.iter())
                .all(|((a, _, _), b)| a == b);
            if matched {
                let byte_start = lowered[i].1;
                let byte_end = lowered[i + needle_chars.len() - 1].2;
                if byte_start < byte_end && byte_end <= line.text.len() {
                    out.push(TerminalSearchMatch {
                        line_idx,
                        byte_start,
                        byte_end,
                    });
                }
                i += needle_chars.len();
            } else {
                i += 1;
            }
        }
        if out.len() >= 2000 {
            break;
        }
    }
    out
}

/// Extracts safe `http://` and `https://` URLs with balanced trailing punctuation trimmed,
/// returning `(col_start, col_end, url)` in terminal display columns.
pub fn extract_clickable_urls(line: &str) -> Vec<(usize, usize, String)> {
    if !line.contains("http://") && !line.contains("https://") {
        return Vec::new();
    }
    let mut results = Vec::new();
    let mut search_from = 0usize;
    while search_from < line.len() {
        let rest = &line[search_from..];
        let next_rel = match (rest.find("https://"), rest.find("http://")) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        let Some(rel) = next_rel else {
            break;
        };
        let start_b = search_from + rel;
        let mut end_b = start_b;
        for (off, ch) in line[start_b..].char_indices() {
            if !ch.is_ascii()
                || ch.is_ascii_whitespace()
                || ch.is_ascii_control()
                || matches!(ch, '"' | '\'' | '<' | '>' | '`')
            {
                break;
            }
            end_b = start_b + off + ch.len_utf8();
        }
        let mut raw_url = &line[start_b..end_b];
        // Strip trailing punctuation while keeping balanced parentheses/brackets intact (e.g. Wikipedia/Rust docs).
        while let Some(last) = raw_url.chars().last() {
            let strip = match last {
                '.' | ',' | ';' | ':' | '!' | '?' => true,
                ')' => {
                    let opens = raw_url.chars().filter(|&c| c == '(').count();
                    let closes = raw_url.chars().filter(|&c| c == ')').count();
                    closes > opens
                }
                ']' => {
                    let opens = raw_url.chars().filter(|&c| c == '[').count();
                    let closes = raw_url.chars().filter(|&c| c == ']').count();
                    closes > opens
                }
                _ => false,
            };
            if strip {
                raw_url = &raw_url[..raw_url.len() - last.len_utf8()];
            } else {
                break;
            }
        }
        if raw_url.len() > "https://a".len() {
            let col_start: usize = line[..start_b]
                .chars()
                .map(|c| char_display_width(c).max(1))
                .sum();
            let col_width: usize = raw_url
                .chars()
                .map(|c| char_display_width(c).max(1))
                .sum();
            results.push((col_start, col_start + col_width, raw_url.to_string()));
        }
        search_from = end_b.max(start_b + 1);
    }
    results
}

/// Returns the clickable `http(s)://` URL covering display column `col` on `line`, if any.
pub fn url_at_display_col(line: &str, col: usize) -> Option<String> {
    for (c0, c1, url) in extract_clickable_urls(line) {
        if col >= c0 && col < c1 {
            return Some(url);
        }
    }
    None
}

/// Opens a validated `http://` or `https://` URL in the OS default browser without shell expansion.
pub fn open_external_url_safely(url: &str) -> bool {
    let trimmed = url.trim();
    if (!trimmed.starts_with("https://") && !trimmed.starts_with("http://"))
        || trimmed.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return false;
    }
    #[cfg(target_os = "macos")]
    let opener = "/usr/bin/open";
    #[cfg(not(target_os = "macos"))]
    let opener = "xdg-open";

    std::process::Command::new(opener)
        .arg(trimmed)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok()
}

/// Formats a session's scrollback buffer into a clean, timestamped Markdown transcript.
pub fn format_session_markdown_export(
    title: &str,
    cwd: &str,
    lines: &[StyledTerminalLine],
) -> String {
    let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let mut out = String::with_capacity(lines.len().saturating_mul(48) + 256);
    out.push_str(&format!("# Coda Session Transcript — {title}\n\n"));
    out.push_str(&format!("- **Exported**: `{ts}`\n"));
    out.push_str(&format!("- **Working Directory**: `{cwd}`\n"));
    out.push_str(&format!("- **Lines**: `{}`\n\n", lines.len()));
    out.push_str("```text\n");
    // Trim trailing blank padding rows while keeping internal blank lines intact.
    let last_non_empty = lines
        .iter()
        .rposition(|l| !l.text.trim_end().is_empty())
        .map(|i| i + 1)
        .unwrap_or(0);
    for line in &lines[..last_non_empty] {
        out.push_str(line.text.trim_end());
        out.push('\n');
    }
    out.push_str("```\n");
    out
}

/// Writes a Markdown session log into `~/Downloads` (or `$TMPDIR` if `~/Downloads` is absent).
pub fn export_session_markdown_file(
    title: &str,
    cwd: &str,
    lines: &[StyledTerminalLine],
) -> Result<PathBuf, String> {
    let slug: String = title
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let clean_slug = if slug.is_empty() {
        "session".to_string()
    } else {
        slug.chars().take(36).collect()
    };
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let filename = format!("coda-{clean_slug}-{stamp}.md");

    let base_dir = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|h| {
            let dl = h.join("Downloads");
            if dl.is_dir() { dl } else { h }
        })
        .unwrap_or_else(std::env::temp_dir);

    let out_path = base_dir.join(filename);
    let md = format_session_markdown_export(title, cwd, lines);
    write_file_atomic_mode(&out_path, md.as_bytes(), 0o644).map_err(|e| e.to_string())?;
    Ok(out_path)
}

/// Formats `HostHealthSnapshot` into a compact status line (`v0.7.0 · 8c · load 0.42 · 6.2/16.0G`).
pub fn format_host_health_compact(h: &HostHealthSnapshot) -> String {
    let mut parts = vec![format!("v{}", h.daemon_version.trim_start_matches('v'))];
    parts.push(format!("{}c", h.cpu_cores));
    if let Some(load) = h.load_avg_1m {
        parts.push(format!("load {load:.2}"));
    }
    if let (Some(used), Some(tot)) = (h.mem_used_mb, h.mem_total_mb) {
        parts.push(format!(
            "{:.1}/{:.1}G",
            (used as f64) / 1024.0,
            (tot as f64) / 1024.0
        ));
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_terminal_matches_case_insensitive_and_utf8_safe() {
        let lines = vec![
            StyledTerminalLine {
                text: "Building coda-server v0.7.0...".into(),
                spans: vec![],
            },
            StyledTerminalLine {
                text: "中文 ERROR: connection reset; error retry".into(),
                spans: vec![],
            },
        ];
        let m = find_terminal_matches(&lines, "error");
        assert_eq!(m.len(), 2);
        assert_eq!(&lines[1].text[m[0].byte_start..m[0].byte_end], "ERROR");
        assert_eq!(&lines[1].text[m[1].byte_start..m[1].byte_end], "error");
    }

    #[test]
    fn test_extract_clickable_urls_trims_punctuation_and_maps_cjk_columns() {
        let line = "详见文档 (https://codahub.dev/docs/ota?v=1)，以及 https://example.com/a(b).";
        let urls = extract_clickable_urls(line);
        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0].2, "https://codahub.dev/docs/ota?v=1");
        assert_eq!(urls[1].2, "https://example.com/a(b)");
        // "详见文档 (" = 4 CJK chars (8 cols) + " (" (2 cols) = col 10
        assert_eq!(
            url_at_display_col(line, 10).as_deref(),
            Some("https://codahub.dev/docs/ota?v=1")
        );
        assert_eq!(url_at_display_col(line, 3), None);
    }

    #[test]
    fn test_format_session_markdown_export_strips_trailing_blank_rows() {
        let lines = vec![
            StyledTerminalLine {
                text: "$ cargo test".into(),
                spans: vec![],
            },
            StyledTerminalLine {
                text: "test result: ok.".into(),
                spans: vec![],
            },
            StyledTerminalLine {
                text: "    ".into(),
                spans: vec![],
            },
        ];
        let md = format_session_markdown_export("coda · claude", "/tmp/coda", &lines);
        assert!(md.contains("# Coda Session Transcript — coda · claude"));
        assert!(md.contains("$ cargo test\ntest result: ok.\n```"));
    }
}
