# Coda v0.6.0 Deep Audit Report (V3)

> Audited at `v0.6.0` (`d30a9d6`) across `crates/coda-server`, `crates/coda-daemon`, `crates/coda-desktop`, `web/`, and `apple/`.
> All 35 verified real bugs (9 P0, 16 P1, 10 P2) have been fixed and regression-tested across all 4 tracks.

## Summary Matrix

| Track | P0 | P1 | P2 | Total | Status |
| --- | --- | --- | --- | --- | --- |
| Rust Server & Daemon (`coda-server`, `coda-daemon`) | 3 | 1 | 2 | 6 | All fixed & verified |
| macOS GPUI Desktop (`coda-desktop`) | 0* | 7 | 6 | 13 | All fixed & verified |
| Web Console (`web/`) | 4 | 5 | 3 | 12 | All fixed & verified |
| Apple Swift 6 (`apple/`) | 4 | 7 | 3 | 14 | All fixed & verified |

*\*Daemon/Desktop cross-transport hot-switch double-stream (V3-01) spans all 4 layers.*

---

## P0 — Critical Bugs

- [x] **V3-01 (P0 · Daemon + Web + Desktop + Apple)**: Relay → Direct P2P hot-switch causes dual PTY streaming (doubled/corrupted characters) & leaked daemon `hc attach` subscriptions because Relay is never detached and `active_ptys` is isolated per transport.
  - *Fix*: Shared `SharedHostRuntime` (`active_ptys` + cross-transport single-owner eviction per `(device_id, terminal)`) in `coda-daemon`; explicit Relay `pty_detach` + duplicate Relay chunk suppression in `coda-desktop`, `web/`, and `apple/`.
- [x] **V3-02 (P0 · Daemon)**: Split-brain `NativeFallbackShells` across Relay vs Direct P2P, and Relay reconnects drop & kill all running fallback shells.
  - *Fix*: Hoisted `herdr`, `fallback_shells`, and `active_ptys` into `SharedHostRuntime` shared across Direct P2P and Relay reconnect loops; Relay loop exit preserves `"p2p-"` PTYs.
- [x] **V3-03 (P0 · Daemon Security)**: Disabling "Allow Remote Control" or signing out aborts `p2p_abort` (listener only) while leaving already-connected LAN Direct P2P sockets alive with full shell/file access.
  - *Fix*: Added `shutdown_tx: watch::Sender<bool>` on `SharedHostRuntime`, triggered in `EmbeddedHostHandle::stop()`; every Direct P2P connection selects on `shutdown_rx.changed()` + `writer_dead_rx.changed()` + 75s idle timeout and tears down only its unique `p2p-<uuid>` nonce.
- [x] **V3-04 (P0 · Apple)**: Fatal Swift `ClosedRange` crash (`EXC_BREAKPOINT`) in `GhosttyReflowEngine.scrollUp` (`top...(bottom - n)` traps when `n == bottom - top + 1`, e.g., `CSI M` on bottom line).
  - *Fix*: Changed `top...(bottom - n)` to half-open `top..<(bottom - n + 1)` and clamped CSI repeat counts in `GhosttySurfaceEngine.swift`, with regression tests in `CodaSelfTest` & `CodaSecurityAndVTTests`.
- [x] **V3-05 (P0 · Apple)**: Dual `@ObservedObject` + `@StateObject` eager instantiation in `CodaAdaptiveRootView.init()` wipes iOS `CodaWorkspaceStore` on SwiftUI body re-evaluation and breaks notification tap delegate.
  - *Fix*: Hoisted `@StateObject private var store = CodaWorkspaceStore()` into `CodaMobileApp.swift` and removed duplicate `@StateObject ownedStore` from `CodaAdaptiveRootView`.
- [x] **V3-06 (P0 · Apple)**: Direct P2P disconnect when Relay was never opened (or Relay reconnect nonce rotation) leaves `attachedSessionIDs` stale and permanently freezes active terminals.
  - *Fix*: Emitted `direct_disconnected_need_relay` when Direct drops without an open Relay leg and `peer_session_reset` on Relay drop/nonce rotation; `CodaWorkspaceStore` clears `attachedSessionIDs`, connects Relay, and re-attaches active sessions before showing the failover banner.
- [x] **V3-07 (P0 · Web)**: Console `⌘K` Command Palette renders with undefined `.cmd-mask` / `.cmd-card` classes off-screen below `100dvh`.
  - *Fix*: Switched command palette markup in `web/src/app.ts` to defined `.modal-mask` / `.modal-card` styles.
- [x] **V3-08 (P0 · Web)**: Switching from Direct LAN Daemon A to Daemon B never closes old `directWs` or resets `activeTransportMode`, deadlocking LAN probing for Daemon B; `handlePeerMessage` also lacks `peerId === selectedDaemon?.id` guard, polluting Daemon B with stale Daemon A snapshots.
  - *Fix*: Added centralized `teardownDirectWs()` + `selectDaemon()` used across all 5 switch/delete paths with `directProbeGeneration` guards and `isSelected` gating in `handlePeerMessage`.
- [x] **V3-09 (P0 · Web + Apple)**: Delete Account (`DELETE /api/v1/auth/me`) sends `current_password` instead of `password`, always failing with HTTP 422.
  - *Fix*: Updated JSON payload key to `password` in both `web/src/app.ts` and `apple/Sources/CodaKit/CodaNetworkClient.swift` (plus safe non-JSON error parsing in `web/src/app.ts`).

## P1 — High Severity Bugs & Races

- [x] **V3-10 (P1 · Web + Desktop + Apple)**: Sticky OSC 7501 `NeedsYou` state reverts after `1 · Yes` / `2 · Always` / `Esc · Deny` or `herdr_live_event` on the very next `pty_output_chunk`, popping the approval bar back up and firing phantom duplicate notifications.
  - *Fix*: Added edge-only OSC 7501 consumption (`consumeLatestStatus()` in Web, `osc_seq` in GPUI, `oscStatusGeneration` in Apple), optimistic `NeedsYou → Working` clearing on approval/interrupt, and unconditional notification baseline tracking with reset on sign-out/URL change.
- [x] **V3-11 (P1 · Desktop)**: Tuple destructuring swap in `on_scroll_wheel` reads `bracketed_paste` as `app_cursor_keys`.
  - *Fix*: Corrected destructuring to `(in_alt, _, app_cursor)` in `crates/coda-desktop/src/lib.rs`.
- [x] **V3-12 (P1 · Desktop)**: Concurrent handshake race in `try_hot_upgrade_peer_to_direct` (missing `peer_connect_locks`) & Direct → Relay failover ignores 8s flap-guard by immediately re-probing dead LAN.
  - *Fix*: Guarded hot-upgrade with per-host `peer_connect_locks`, atomic peer swap before closing superseded Relay, and forced `allow_direct = false` on Direct failover in `crates/coda-desktop/src/remote_mesh.rs`.
- [x] **V3-13 (P1 · Desktop)**: Out-of-order remote keystrokes from per-keystroke `spawn_bg` on 2-worker Tokio runtime.
  - *Fix*: Added `peers_sync: std::sync::RwLock` + non-blocking FIFO `LivePeerSession::try_send_oneway` in `coda-daemon` & `coda-desktop/src/remote_mesh.rs`.
- [x] **V3-14 (P1 · Daemon)**: Multi-relay silent leg death (`cdn_wss` drop never detected while `tcp_reader` holds `in_tx`) & `LivePeerSession` write-half error never aborts reader or triggers failover (plus missing read idle timeout).
  - *Fix*: Wired `leg_dead_tx` per relay leg, `writer_dead_rx` signaling in `LivePeerSession::connect_auto`, and 75s read idle timeouts across Relay and Direct P2P.
- [x] **V3-15 (P1 · Desktop)**: `VtScreenBuffer` unclamped `CSI S/T` scroll DoS & `CSI B/C/E/X` overflow; `EMBEDDED_HOST_STARTED` stuck across Dock reopen / rapid toggle / account switch; `Cmd+,` hardcoded to initial window entity.
  - *Fix*: Clamped scroll counts & used `saturating_add` in `pty.rs`; serialized embedded host start/stop/account-switch with generation counter & `Drop` cleanup in `remote_mesh.rs`; routed `ToggleSettings` directly in `CodaGpuWorkspaceView::render`.
- [x] **V3-16 (P1 · Desktop)**: Native macOS notification foreground/background suppression inverted, newly spawned sessions swallow their first `NeedsYou` alert, unbounded `osascript` thread spawning & tracker memory leak on closed sessions.
  - *Fix*: Wired `window.is_window_active()` into `app_is_active`, added `seed_baseline`/`prune_session`/`reset`, and replaced per-alert `std::thread::spawn` with a bounded `sync_channel(16)` worker in `crates/coda-desktop/src/notify.rs`.
- [x] **V3-17 (P1 · Apple)**: Single `pathMonitor` / `lanReprobeTask` overwrites earlier hosts, `handleNetworkLeftLan` only drops one peer, `NWPathMonitor` defeats backoff, `connectBestAvailable` short-circuits on open Relay stranding local LAN machines, and `reprobeDirectIfNeeded` lacks per-peer concurrency lock.
  - *Fix*: Refactored `CodaNetworkClient` to a shared `NWPathMonitor` with transition-gated `lastOnLanInterface` (`false -> true` only), per-peer `trackedSmartPathPeers` / `lanReprobeTasks` / `inFlightDirectProbes`, drop-all LAN peers on cellular switch, and LAN-first probing for unprobed hosts in `connectBestAvailable`.
- [x] **V3-18 (P1 · Apple)**: Fresh users never get prompted for notification permission (`notificationsEnabled` defaults `true`, only calls `requestAuthorization` on toggle); cold-launch / mid-refresh notification tap silently dropped by `isRefreshing` early return.
  - *Fix*: Installed `UNUserNotificationCenter.delegate` in `CodaLocalNotifier.init()`, buffered pre-registration taps in `pendingTappedSessionId`, added `requestAuthorizationIfNotDetermined()` in `startLiveSync()`, and persisted `pendingSelectSessionID` resolved at the end of `refreshMachinesAndSessions()`.
- [x] **V3-19 (P1 · Apple)**: `refreshTerminalScreen` leaves `treatLFAsCRLF = true` permanently on live PTY engines & clobbers in-flight PTY chunks; transient host failure wipes multi-host sessions & ejects user from active terminal.
  - *Fix*: Made `treatLFAsCRLF` mutable and reset it to `false` immediately after feeding snapshot text; ordered snapshot-before-attach with revision guard; made `listTerminals` throw on decode failure and preserved cached sessions for transiently failing still-online hosts.
- [x] **V3-20 (P1 · Apple)**: Duo secondary pane (`L2`) misroutes Compose, Upload, Git Diff, Minimap, Swipes & Resizes to primary pane (`L1`); terminal scrollback & input buffer leak across session switches & broken `atBottom` auto-scroll on macOS/iOS.
  - *Fix*: Added `activeSheetSessionID`, `siblingSessions(for:)`, `switchSibling(delta:for:isSecondary:)`, `loadGitDiff(for:)`, dual-pane `resizeTerminalPty`, `terminalRenderRevisionBySession`, bottom-anchor `.onAppear`/`.onDisappear` `atBottom` tracking, and `.onChange(of: session.id)` state reset.
- [x] **V3-21 (P1 · Web)**: File uploads `4.5–8 MiB` exceed `MAX_WIRE_FRAME_BYTES = 8 MiB` after double-Base64 expansion and drop the WebSocket.
  - *Fix*: Lowered `MAX_UPLOAD_BYTES` to `4 MiB` in `web/src/app.ts` (and matched 4 MiB guard in Apple `CodaWorkspaceStore.uploadAttachment`).
- [x] **V3-22 (P1 · Web)**: `renderConsoleView` never restores `#term-box.scrollTop`, yanking terminal to line 1 on every prompt submit / re-render.
  - *Fix*: Saved and restored `#term-box.scrollTop` / `wasAtBottom` across `renderConsoleView` and switched composer/upload/voice updates to in-place DOM updates.
- [x] **V3-23 (P1 · Web)**: `vt.ts` `patchDom` degrades to O(N) full rewrite after 400-line scrollback fills, misses partial-outside selections, and never paints deferred rows after selection clears.
  - *Fix*: Added `scrollbackEvictions` for O(1) front-row DOM eviction, `containsNode` partial-outside selection protection, `skippedProtected` flag, and `selectionchange` catch-up listener in `web/src/vt.ts` & `web/src/app.ts`.
- [x] **V3-24 (P1 · Web)**: `Ctrl+K` double-fires in `#term-box`, `Cmd+K` wipes `/login` & `/activate` forms, `Cmd+Backspace/Arrows` hijacked by `terminal_keys.ts`, and IME `Enter` executes `#console-cmd-input` prematurely.
  - *Fix*: Passed through all `e.metaKey` shortcuts in `terminal_keys.ts`, stopped propagation in `#term-box` `keydown`, restricted `⌘K` to `home|console` with `!e.defaultPrevented`, and guarded `#console-cmd-input` `Enter` against `e.isComposing || e.keyCode === 229`.
- [x] **V3-25 (P1 · Web)**: Async races in `probeSingleDirectWs`, `tryNegotiateDirectLanWs`, `onDrop` (stale `peer.is_online`), and `connectRelayWebSocket` (missing generation/token guards).
  - *Fix*: Added `directProbeGeneration` and `relayConnectGeneration` checks across all async Direct/Relay callbacks in `web/src/app.ts`.

## P2 — Medium Severity & State Hygiene

- [x] **V3-26 (P2 · Server)**: `change_password_handler` broadcasts wildcard `notify_session_revoked(..., None)`, disconnecting the caller's own valid `/api/v1/events/ws`.
  - *Fix*: Updated `db::change_password` to return revoked `token_sha256` hashes via `RETURNING token_sha256` and notified only revoked sessions in `crates/coda-server/src/lib.rs`.
- [x] **V3-27 (P2 · Desktop)**: Hardcoded `120x36` resize clobber during hot-switch / failover (`reattach_terms_preserving_screen`).
  - *Fix*: Queried real active dimensions via `PtyManager::session_dims` during hot-switch/failover re-attach.
- [x] **V3-28 (P2 · Desktop)**: Double `seed_and_attach_remote_term` race in `spawn_remote_terminal_or_agent` & duplicate fallback initial screen.
  - *Fix*: Guarded `spawn_remote_terminal_or_agent` with `if need_attach` + `clear_buffer` before snapshot, and prepended `\x1b[2J\x1b[H` on daemon fallback `initial_snap`.
- [x] **V3-29 (P2 · Desktop)**: Stale `renaming_host_id` / `pw_change_expanded` / plaintext `pw_*_input` retained across Settings close, tab switch, Escape, `sign_out`, and `delete_remote_device`.
  - *Fix*: Added `clear_settings_ephemeral_inputs()` invoked across close, tab switch, Escape, sign-out, and host deletion.
- [x] **V3-30 (P2 · Desktop)**: CJK wide-char selection highlight misalignment in `render_styled_terminal_line`.
  - *Fix*: Built exact terminal-column-to-UTF-8-byte mapping using `char_display_width` in `crates/coda-desktop/src/lib.rs`.
- [x] **V3-31 (P2 · Desktop)**: Deleted remote hosts not pruned in `sync_all_hosts_and_sessions` & `scroll_offset` not clamped when VT output shrinks.
  - *Fix*: Pruned deleted hosts & cleaned peer maps in `sync_all_hosts_and_sessions`; clamped `scroll_offset` against live scrollback length.
- [x] **V3-32 (P2 · Web)**: Notification false alert on daemon switch-back, missing background daemon enrollment, `needs_you` alias mismatch, and `focusNotifiedTerminal` fallback bug on removed peer.
  - *Fix*: Exported shared `isNeedsYouState` in `web/src/notify.ts`, enrolled background daemons, restored `activeTerminalAddr` before observing, and guarded removed-peer lookup.
- [x] **V3-33 (P2 · Web)**: Multi-file drop / multi-image paste drops files 2..N, text drag triggers file-drop highlight, and `upload_terminal_file_result` disrupts focus/scroll.
  - *Fix*: Restricted drag-over/drop highlighting to `types.includes("Files")`, queued all dropped/pasted/selected files, and rendered upload status in place.
- [x] **V3-34 (P2 · Web)**: `performLocalSignOut` leaks `voiceApiKeyLocal` / `voiceConfig` across accounts in same tab & `insertTranscribedText` blurs input via full `render()`.
  - *Fix*: Cleared voice credentials, smart-path state, and timers in `performLocalSignOut()`, and inserted transcripts directly into `#console-cmd-input` in place.
- [x] **V3-35 (P2 · Apple)**: Voice dictation start/cancel concurrency races & Compose → Settings modal collision; Alt-Screen `resizeReflow` discards `primaryScrollback` & `savedPrimaryCursor`; `CodaNetworkClient.asyncEvents` retains 2,048 stale buffers when `asyncEventHandler` is set.
  - *Fix*: Tracked `voiceTask` + `isStartingVoice` + `Task.checkCancellation()` in voice dictation, coordinated Compose → Settings via `pendingOpenSettingsAfterCompose` in Compose `onDismiss`, preserved & reflowed `primaryScrollback` + `savedPrimaryCursor` in alt-screen `resizeReflow`, and bypassed `asyncEvents` buffering when `asyncEventHandler` is installed.

---

## V3.1 — Production Deploy Hotfix (`green-jp-02`) & Post-V3 Cross-Audit Sign-Off

- [x] **PROD-01 (Critical · Deploy & `coda-server`)**: `coda-deploy-bundle-v0.6.0.tar.gz` startup crash on `green-jp-02` (`error: a value is required for '--trust-proxy-headers' but none was supplied`).
  - *Root Cause*: `deploy/docker-compose.yml` set `CODA_TRUST_PROXY_HEADERS: ${CODA_TRUST_PROXY_HEADERS:-}`, injecting empty string `""` into `clap`'s `bool` `--trust-proxy-headers` (`ArgAction::SetTrue`) whenever `.env` left `CODA_TRUST_PROXY_HEADERS` unset/commented; `auto` from `.env.example` also failed `bool` parsing.
  - *Fix*: Introduced 3-state `TrustProxyMode { Auto, Always, Never }` + custom `parse_trust_proxy_mode` (`""|"auto"` → `Auto`, `"true"|"1"|"always"` → `Always`, `"false"|"0"|"never"` → `Never`) + `num_args = 0..=1, default_missing_value = "true"`, added `sanitize_empty_coda_env_vars()` in both `coda-server` and `coda-daemon` before `Args::parse()`, updated `deploy/docker-compose.yml` to `${CODA_TRUST_PROXY_HEADERS:-auto}`, synced `deploy/install-server.sh`, and added unit test `test_cli_args_empty_docker_env_and_trust_proxy_modes`.
- [x] **V3.1-Rust (Server / Daemon / GPUI Desktop — 13 items)**:
  - Single-frame `SessionRevocation::AllExcept` + fail-closed `RecvError::Lagged` on `/api/v1/events/ws`; `shutdown_tx` notification when Relay write tasks exit + `peer_offline` on closed `mpsc`.
  - Fixed `parse_relay_ticket_exp_unix` `STANDARD` Base64 decoding (added `test_parse_relay_ticket_exp_unix_standard_base64_and_renewal`), returned `SessionResetRequired` on unknown Relay peer session (`seq > 64`), pruned orphaned `p2p-*` PTYs across `SIGHUP`, made `handle_direct_p2p_socket` cancel-safe via dedicated reader task, fell back from `raw_tcp_addr` to `wss_url`, and omitted `session_auth` after peer confirmation.
  - Serialized `RemoteMeshManager` `peers` + `peers_sync` writes, added Relay disconnect/`SessionReset` auto-reattach in `spawn_peer_event_loop`, wired GPUI `cx.observe_window_activation` for `app_is_active`, isolated raw alt-screen scroll from `NeedsYou` dismissal, and aligned `SelectAllAction` with `char_display_width`.
- [x] **V3.1-Web (Web Console — 11 items)**:
  - Fixed `selectDaemon` same-ID refresh, `#term-draft-slot` + `syncVoiceButtonInPlace` in-place DOM updates without full `render()`, `data-vt-alt` sync, `imeComposing` reset on blur/render/sign-out, pre-`await` `relayConnectGeneration` allocation, background daemon enrollment on `auth_ack`/Settings/`peer_presence`, light-mode `.term-scroll-bottom-btn`, `loadSnapshot()` clearing `pendingStatus`, and `patchDom` selection freeze across scrollback head eviction (with regression tests in `web/src/web.test.ts`).
- [x] **V3.1-Apple (Swift 6 Client — 11 items)**:
  - Enforced `validatedServerURL` (`https://` or loopback `http://`) + cross-server bearer token isolation in `updateServerURL` / `ensureClient` and `workspaceEpoch` guards across async callbacks.
  - Removed spurious `peer_session_reset` in `connectAutoSmartPath`, refreshed `trackedSmartPathPeers` endpoints in `connectBestAvailable`, added cancellation/untracked cleanup in `reprobeDirectIfNeeded`, isolated snapshot errors from `attachTerminalPty`, fixed `LazyVStack` `terminal_bottom_anchor` auto-scroll suppression, scoped Duo secondary pane sheets via `activeSheetIsSecondary`, guarded voice & git-diff generations, and matched git worktree paths in `CodaMobileSpacesTabView`.

