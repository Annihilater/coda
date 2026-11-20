//! Edge-triggered native desktop notification tracker & non-blocking macOS dispatcher
//! for `OSC 7501` agent lifecycle transitions (`needs_you`, `error`, `working -> done`).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::config::{tr, UiLocale};
use crate::pty::{AgentLifecycleState, Osc7501Status};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopNotificationPayload {
    pub session_id: String,
    pub kind: NotificationKind,
    pub title: String,
    pub subtitle: String,
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationKind {
    NeedsYou,
    Done,
}

/// Edge-triggered tracker that prevents notification storms across rapid PTY polling ticks.
#[derive(Debug, Clone)]
pub struct DesktopNotificationTracker {
    last_state: HashMap<String, AgentLifecycleState>,
    last_notified_at: HashMap<String, Instant>,
    cooldown: Duration,
}

impl Default for DesktopNotificationTracker {
    fn default() -> Self {
        Self::new(Duration::from_secs(20))
    }
}

impl DesktopNotificationTracker {
    pub fn new(cooldown: Duration) -> Self {
        Self {
            last_state: HashMap::new(),
            last_notified_at: HashMap::new(),
            cooldown,
        }
    }

    pub fn observe(
        &mut self,
        session_id: &str,
        session_title: &str,
        osc: &Osc7501Status,
        active_session_id: Option<&str>,
        locale: UiLocale,
        now: Instant,
    ) -> Option<DesktopNotificationPayload> {
        let next = osc.state;
        let prev = self.last_state.insert(session_id.to_string(), next);

        // Initial snapshot establishes baseline without spamming on startup/attach.
        let prev = prev?;
        if prev == next {
            return None;
        }

        let kind = match next {
            AgentLifecycleState::NeedsYou => NotificationKind::NeedsYou,
            AgentLifecycleState::Done if prev == AgentLifecycleState::Working => {
                NotificationKind::Done
            }
            _ => return None,
        };

        // Suppress alerts for the currently focused session unless it explicitly requires user approval.
        if active_session_id == Some(session_id) && kind == NotificationKind::Done {
            return None;
        }

        if let Some(&last_ts) = self.last_notified_at.get(session_id) {
            if now.saturating_duration_since(last_ts) < self.cooldown {
                return None;
            }
        }
        self.last_notified_at.insert(session_id.to_string(), now);

        let detail = osc.task.as_deref().unwrap_or("").trim();
        let (title, fallback_body) = match kind {
            NotificationKind::NeedsYou => (
                tr(locale, "Coda · Action Needed", "Coda · 需要交互审批").to_string(),
                tr(
                    locale,
                    "Agent is waiting for your confirmation",
                    "AI Agent 正在等待你的确认或授权",
                ),
            ),
            NotificationKind::Done => (
                tr(locale, "Coda · Task Completed", "Coda · 任务完成").to_string(),
                tr(
                    locale,
                    "Background agent finished its run",
                    "后台 Agent 已完成当前任务",
                ),
            ),
        };

        Some(DesktopNotificationPayload {
            session_id: session_id.to_string(),
            kind,
            title,
            subtitle: session_title.to_string(),
            body: if detail.is_empty() {
                fallback_body.to_string()
            } else {
                detail.to_string()
            },
        })
    }
}

/// Safely escapes arbitrary terminal text before embedding in an AppleScript double-quoted string.
pub fn escape_applescript_string(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 8);
    for ch in input.chars().take(240) {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\r' | '\n' | '\t' => out.push(' '),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Dispatches a native macOS Notification Center banner asynchronously off the UI thread.
/// No-op under unit tests (`cfg(test)`) and non-macOS targets.
pub fn dispatch_native_notification(payload: DesktopNotificationPayload) {
    #[cfg(all(target_os = "macos", not(test)))]
    {
        std::thread::spawn(move || {
            let title = escape_applescript_string(&payload.title);
            let subtitle = escape_applescript_string(&payload.subtitle);
            let body = escape_applescript_string(&payload.body);
            let sound = match payload.kind {
                NotificationKind::NeedsYou => " sound name \"Glass\"",
                NotificationKind::Done => "",
            };
            let script = format!(
                "display notification \"{body}\" with title \"{title}\" subtitle \"{subtitle}\"{sound}"
            );
            let _ = std::process::Command::new("/usr/bin/osascript")
                .args(["-e", &script])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        });
    }
    #[cfg(not(all(target_os = "macos", not(test))))]
    {
        let _ = payload;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_osc(state: AgentLifecycleState, task: Option<&str>) -> Osc7501Status {
        Osc7501Status {
            state,
            task: task.map(str::to_string),
            updated_at: Instant::now(),
        }
    }

    #[test]
    fn tracker_is_edge_triggered_and_enforces_cooldown() {
        let mut tracker = DesktopNotificationTracker::new(Duration::from_secs(20));
        let t0 = Instant::now();

        // Baseline snapshot never fires
        assert!(tracker
            .observe(
                "s1",
                "Mac · claude",
                &mk_osc(AgentLifecycleState::Working, None),
                Some("s2"),
                UiLocale::Zh,
                t0,
            )
            .is_none());

        // Transition Working -> NeedsYou fires immediately
        let alert = tracker
            .observe(
                "s1",
                "Mac · claude",
                &mk_osc(AgentLifecycleState::NeedsYou, Some("Approve git push?")),
                Some("s2"),
                UiLocale::Zh,
                t0 + Duration::from_secs(2),
            )
            .expect("expected needs_you alert");
        assert_eq!(alert.kind, NotificationKind::NeedsYou);
        assert_eq!(alert.body, "Approve git push?");
        assert!(alert.title.contains("需要交互审批"));

        // Repeated same-state poll does not fire
        assert!(tracker
            .observe(
                "s1",
                "Mac · claude",
                &mk_osc(AgentLifecycleState::NeedsYou, Some("Approve git push?")),
                Some("s2"),
                UiLocale::Zh,
                t0 + Duration::from_secs(3),
            )
            .is_none());

        // Within cooldown, Working -> Done is suppressed
        tracker.observe(
            "s1",
            "Mac · claude",
            &mk_osc(AgentLifecycleState::Working, None),
            Some("s2"),
            UiLocale::En,
            t0 + Duration::from_secs(4),
        );
        assert!(tracker
            .observe(
                "s1",
                "Mac · claude",
                &mk_osc(AgentLifecycleState::Done, Some("All green")),
                Some("s2"),
                UiLocale::En,
                t0 + Duration::from_secs(10),
            )
            .is_none());

        // Active session suppresses Done, but background session after cooldown fires Done
        tracker.observe(
            "s1",
            "Mac · claude",
            &mk_osc(AgentLifecycleState::Working, None),
            Some("s1"),
            UiLocale::En,
            t0 + Duration::from_secs(25),
        );
        assert!(tracker
            .observe(
                "s1",
                "Mac · claude",
                &mk_osc(AgentLifecycleState::Done, Some("Build ok")),
                Some("s1"),
                UiLocale::En,
                t0 + Duration::from_secs(26),
            )
            .is_none());

        tracker.observe(
            "s1",
            "Mac · claude",
            &mk_osc(AgentLifecycleState::Working, None),
            Some("s2"),
            UiLocale::En,
            t0 + Duration::from_secs(27),
        );
        let done = tracker
            .observe(
                "s1",
                "Mac · claude",
                &mk_osc(AgentLifecycleState::Done, Some("Build ok")),
                Some("s2"),
                UiLocale::En,
                t0 + Duration::from_secs(28),
            )
            .expect("expected background done notification");
        assert_eq!(done.kind, NotificationKind::Done);
        assert_eq!(done.body, "Build ok");
    }

    #[test]
    fn applescript_escaping_neutralizes_quotes_backslashes_and_newlines() {
        let escaped = escape_applescript_string("hi \"world\" \\ \n\x07ok");
        assert_eq!(escaped, "hi \\\"world\\\" \\\\  ok");
        assert!(!escaped.contains('\n'));
        assert!(!escaped.contains('\x07'));
    }
}
