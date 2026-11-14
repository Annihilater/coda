use std::{net::SocketAddr, path::PathBuf, sync::OnceLock, time::Duration};

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use coda_daemon::{
    list_account_hosts, login_with_password_and_enroll, mint_peer_relay_config,
    register_client_identity, run_daemon_once_with_p2p, spawn_direct_p2p_listener,
    subscribe_account_events_ws, LivePeerSession, PeerAsyncEvent,
};
use coda_protocol::{AccountLiveEvent, NetworkEndpointCandidate, PeerMessage, TrafficLane};
use coda_server::{start_full_server_with_options, AuthResponse, ServerOptions};
use serde_json::json;
use tokio::sync::{broadcast, mpsc, Mutex};
use uuid::Uuid;

static DB_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

async fn next_matching_account_event<F>(
    rx: &mut mpsc::Receiver<AccountLiveEvent>,
    timeout: Duration,
    label: &str,
    mut predicate: F,
) -> AccountLiveEvent
where
    F: FnMut(&AccountLiveEvent) -> bool,
{
    let deadline = tokio::time::Instant::now() + timeout;
    let mut seen = Vec::new();
    loop {
        let rem = deadline.saturating_duration_since(tokio::time::Instant::now());
        if rem.is_zero() {
            panic!("timed out waiting for account WS event `{label}`; seen={seen:?}");
        }
        match tokio::time::timeout(rem, rx.recv()).await {
            Ok(Some(ev)) => {
                if predicate(&ev) {
                    return ev;
                }
                seen.push(ev);
            }
            Ok(None) => {
                panic!("account events WS closed while waiting for `{label}`; seen={seen:?}");
            }
            Err(_) => {
                panic!("timed out waiting for account WS event `{label}`; seen={seen:?}");
            }
        }
    }
}

async fn collect_pty_marker_and_herdr_event(
    rx: &mut broadcast::Receiver<PeerAsyncEvent>,
    term_addr: &str,
    marker: &str,
    timeout: Duration,
    client_label: &str,
) -> (String, bool) {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut captured = String::new();
    let mut saw_herdr_event = false;

    while !captured.contains(marker) {
        let rem = deadline.saturating_duration_since(tokio::time::Instant::now());
        if rem.is_zero() {
            panic!(
                "{client_label} timed out waiting for `{marker}` in PtyOutputChunk; captured={captured:?}"
            );
        }
        match tokio::time::timeout(rem, rx.recv()).await {
            Ok(Ok(PeerAsyncEvent::PtyOutputChunk {
                terminal, data_b64, ..
            })) if terminal == term_addr => {
                if let Ok(bytes) = B64.decode(&data_b64) {
                    captured.push_str(&String::from_utf8_lossy(&bytes));
                }
            }
            Ok(Ok(PeerAsyncEvent::HerdrLiveEvent { .. })) => {
                saw_herdr_event = true;
            }
            Ok(Ok(_)) => {}
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => {}
            Ok(Err(e)) => {
                panic!("{client_label} event stream ended early ({e}); captured={captured:?}");
            }
            Err(_) => {
                panic!(
                    "{client_label} timed out waiting for `{marker}` in PtyOutputChunk; captured={captured:?}"
                );
            }
        }
    }

    while let Ok(ev) = rx.try_recv() {
        if matches!(ev, PeerAsyncEvent::HerdrLiveEvent { .. }) {
            saw_herdr_event = true;
        }
    }

    (captured, saw_herdr_event)
}

fn create_dirty_git_repo(path: &PathBuf) {
    std::fs::create_dir_all(path).expect("create temp git repo dir");
    let run_git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .output()
            .expect("spawn git");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run_git(&["init"]);
    run_git(&["config", "user.email", "test@coda.dev"]);
    run_git(&["config", "user.name", "CodaTest"]);
    std::fs::write(path.join("README.md"), "# Coda E2E Mesh\n").expect("write README.md");
    run_git(&["add", "README.md"]);
    run_git(&["commit", "-m", "init"]);
    std::fs::write(
        path.join("README.md"),
        "# Coda E2E Mesh\ndirty_mesh_line_2026\n",
    )
    .expect("append dirty line to README.md");
}

#[tokio::test]
async fn test_realtime_ws_push_multiclient_pty_and_direct_p2p_mesh() {
    let _guard = DB_TEST_LOCK.get_or_init(|| Mutex::new(())).lock().await;
    let db_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://vampire@localhost:5432/coda_test".to_string());

    let _ = coda_server::db::Db::connect_and_migrate(&db_url)
        .await
        .expect("PostgreSQL coda_test should be reachable and migrate cleanly");

    let (pg, conn) = tokio_postgres::connect(&db_url, tokio_postgres::NoTls)
        .await
        .expect("PostgreSQL coda_test should be running");
    tokio::spawn(async move {
        let _ = conn.await;
    });
    pg.batch_execute(
        "TRUNCATE device_auth_requests, pairing_tokens, devices, auth_sessions, invite_codes, users RESTART IDENTITY CASCADE;",
    )
    .await
    .unwrap();

    let server = start_full_server_with_options(
        &db_url,
        "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        b"realtime-mesh-hmac-secret-2026".to_vec(),
        None,
        None,
        ServerOptions {
            open_registration: true,
            ..ServerOptions::default()
        },
    )
    .await
    .unwrap();

    let base = format!("http://{}", server.http_addr);
    let http = reqwest::Client::new();

    // 1. Register user on open coordinator
    let auth: AuthResponse = http
        .post(format!("{base}/api/v1/auth/register"))
        .json(&json!({
            "email": "mesh-e2e@coda.dev",
            "handle": "meshe2e",
            "display_name": "Mesh E2E",
            "password": "MeshE2ePass2026!",
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();

    // 2. Subscribe to real-time account WebSocket push & verify initial `connected`
    let mut ws_rx = subscribe_account_events_ws(&base, &auth.token)
        .await
        .expect("connect /api/v1/events/ws");
    let first_ev = tokio::time::timeout(Duration::from_secs(5), ws_rx.recv())
        .await
        .expect("timed out waiting for connected WS event")
        .expect("events WS closed prematurely");
    assert_eq!(first_ev.kind, "connected");

    // 3. Enroll hybrid host via password login -> assert real-time `devices_changed` push
    let daemon_cfg = login_with_password_and_enroll(
        &base,
        "mesh-e2e@coda.dev",
        "MeshE2ePass2026!",
        "hybrid",
        Some("Mac-Hybrid-E2E".into()),
    )
    .await
    .expect("enroll hybrid host");

    let enrolled_ev = next_matching_account_event(
        &mut ws_rx,
        Duration::from_secs(5),
        "devices_changed on host enroll",
        |ev| ev.kind == "devices_changed" && ev.device_id == Some(daemon_cfg.device_id),
    )
    .await;
    assert_eq!(enrolled_ev.role.as_deref(), Some("hybrid"));

    // 4. Start Direct P2P listener & relay daemon uplink -> assert `peer_presence` (online=true)
    let p2p_addr = spawn_direct_p2p_listener(
        daemon_cfg.clone(),
        "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
    )
    .await
    .expect("bind direct P2P listener");

    let cfg_bg = daemon_cfg.clone();
    let p2p_port = p2p_addr.port();
    let daemon_task = tokio::spawn(async move {
        let _ = run_daemon_once_with_p2p(&cfg_bg, Some("raw_tcp_no_sni"), Some(p2p_port)).await;
    });

    let online_ev = next_matching_account_event(
        &mut ws_rx,
        Duration::from_secs(8),
        "peer_presence online=true",
        |ev| {
            ev.kind == "peer_presence"
                && ev.device_id == Some(daemon_cfg.device_id)
                && ev.online == Some(true)
        },
    )
    .await;
    assert_eq!(online_ev.role.as_deref(), Some("hybrid"));

    let hosts = list_account_hosts(&base, &auth.token)
        .await
        .expect("list account hosts");
    assert!(
        hosts
            .iter()
            .any(|h| h.id == daemon_cfg.device_id && h.is_online && h.role == "hybrid"),
        "enrolled hybrid host should appear online in list_account_hosts"
    );

    // 5. Create isolated temporary dirty Git repo
    let tmp_git_dir = std::env::temp_dir().join(format!("coda-e2e-git-{}", Uuid::new_v4().simple()));
    create_dirty_git_repo(&tmp_git_dir);
    let tmp_git_str = tmp_git_dir.to_string_lossy().to_string();

    // 6. Connect 3 concurrent client sessions (2 Relay Raw TCP + 1 Direct P2P LAN IPv4)
    let (dev1, id1) = register_client_identity(&base, &auth.token, "Desktop-Client-1", "macos")
        .await
        .expect("register client 1");
    let relay_cfg1 = mint_peer_relay_config(
        &base,
        &auth.token,
        dev1,
        daemon_cfg.device_id,
        &daemon_cfg.identity.x25519_public_b64,
    )
    .await
    .expect("mint relay cfg 1");
    let client1 =
        LivePeerSession::connect_auto(id1, dev1, daemon_cfg.device_id, vec![], None, Some(relay_cfg1))
            .await
            .expect("connect client1 over relay");
    assert_eq!(client1.route_label(), "relay_raw_tcp");
    assert!(!client1.is_direct_p2p());

    let (dev2, id2) = register_client_identity(&base, &auth.token, "Mobile-Client-2", "ios")
        .await
        .expect("register client 2");
    let relay_cfg2 = mint_peer_relay_config(
        &base,
        &auth.token,
        dev2,
        daemon_cfg.device_id,
        &daemon_cfg.identity.x25519_public_b64,
    )
    .await
    .expect("mint relay cfg 2");
    let client2 =
        LivePeerSession::connect_auto(id2, dev2, daemon_cfg.device_id, vec![], None, Some(relay_cfg2))
            .await
            .expect("connect client2 over relay");
    assert_eq!(client2.route_label(), "relay_raw_tcp");
    assert!(!client2.is_direct_p2p());

    let (dev3, id3) =
        register_client_identity(&base, &auth.token, "iPad-Direct-Client-3", "ipados")
            .await
            .expect("register client 3");
    let p2p_endpoints = vec![NetworkEndpointCandidate {
        kind: "lan_ipv4".into(),
        addr: p2p_addr.to_string(),
    }];
    let client3 =
        LivePeerSession::connect_auto(id3, dev3, daemon_cfg.device_id, p2p_endpoints, None, None)
            .await
            .expect("connect client3 over Direct P2P");
    assert!(client3.is_direct_p2p());
    assert_eq!(client3.route_label(), "lan_ipv4");

    // 7–9. Launch `sh` inside dirty repo, attach 3 concurrent clients, verify multicast PTY + kill
    if coda_herdr::discover_hc_binary().is_some() {
        let mut rx1 = client1.subscribe_events();
        let mut rx2 = client2.subscribe_events();
        let mut rx3 = client3.subscribe_events();

        let launch_reply = client1
            .request(
                PeerMessage::LaunchAgent {
                    req_id: format!("launch-{}", Uuid::new_v4().simple()),
                    agent: "sh".into(),
                    name: Some("e2e-shared-shell".into()),
                    cwd: Some(tmp_git_str.clone()),
                    initial_prompt: None,
                    cols: 100,
                    rows: 30,
                },
                TrafficLane::ControlP0,
                Duration::from_secs(10),
            )
            .await
            .expect("LaunchAgent RPC");
        let term_addr = match launch_reply {
            PeerMessage::LaunchAgentResult {
                ok: true,
                terminal: Some(t),
                ..
            } => t.address,
            other => panic!("unexpected LaunchAgent response: {other:?}"),
        };

        for (idx, client) in [&client1, &client2, &client3].into_iter().enumerate() {
            let att = client
                .request(
                    PeerMessage::PtyAttach {
                        req_id: format!("att-{idx}-{}", Uuid::new_v4().simple()),
                        terminal: term_addr.clone(),
                        cols: 100,
                        rows: 30,
                    },
                    TrafficLane::ControlP0,
                    Duration::from_secs(10),
                )
                .await
                .expect("PtyAttach RPC");
            assert!(
                matches!(att, PeerMessage::PtyAttached { ok: true, .. }),
                "client {} PtyAttach failed: {att:?}",
                idx + 1
            );
        }

        // Brief settle so `sh` inside all 3 `hc attach` bridges finishes mounting
        tokio::time::sleep(Duration::from_millis(200)).await;

        let marker = "E2E_MULTICAST_PTY_OK";
        client1
            .send_oneway(
                PeerMessage::PtyInput {
                    terminal: term_addr.clone(),
                    data_b64: B64.encode(format!("echo {marker}\n")),
                },
                TrafficLane::ControlP0,
            )
            .await
            .expect("send PtyInput marker");

        let ((_cap1, h1), (_cap2, h2), (_cap3, h3)) = tokio::join!(
            collect_pty_marker_and_herdr_event(
                &mut rx1,
                &term_addr,
                marker,
                Duration::from_secs(8),
                "client1(relay)",
            ),
            collect_pty_marker_and_herdr_event(
                &mut rx2,
                &term_addr,
                marker,
                Duration::from_secs(8),
                "client2(relay)",
            ),
            collect_pty_marker_and_herdr_event(
                &mut rx3,
                &term_addr,
                marker,
                Duration::from_secs(8),
                "client3(p2p)",
            ),
        );
        assert!(
            h1 || h2 || h3,
            "expected at least one HerdrLiveEvent across the 3 attached event streams"
        );

        let kill_reply = client1
            .request(
                PeerMessage::KillTerminal {
                    req_id: format!("kill-{}", Uuid::new_v4().simple()),
                    terminal: term_addr.clone(),
                },
                TrafficLane::ControlP0,
                Duration::from_secs(10),
            )
            .await
            .expect("KillTerminal RPC");
        assert!(
            matches!(kill_reply, PeerMessage::ActionAck { ok: true, .. }),
            "expected KillTerminal ActionAck ok=true, got: {kill_reply:?}"
        );
    }

    // 10. Client 2 requests remote Git diff for the temporary repo
    let diff_reply = client2
        .request(
            PeerMessage::GitDiffRequest {
                req_id: format!("diff-{}", Uuid::new_v4().simple()),
                cwd: tmp_git_str.clone(),
            },
            TrafficLane::BulkP1,
            Duration::from_secs(10),
        )
        .await
        .expect("GitDiffRequest RPC");
    match diff_reply {
        PeerMessage::GitDiffResponse {
            branch,
            diff_unified,
            ..
        } => {
            assert!(!branch.is_empty(), "expected non-empty git branch");
            assert!(
                diff_unified.contains("dirty_mesh_line_2026"),
                "expected dirty line in diff_unified, got: {diff_unified:?}"
            );
        }
        other => panic!("unexpected GitDiffRequest response: {other:?}"),
    }

    // 11. Disconnect daemon, verify offline presence WS push & cleanup
    client1.close();
    client2.close();
    client3.close();
    daemon_task.abort();

    let offline_ev = next_matching_account_event(
        &mut ws_rx,
        Duration::from_secs(8),
        "peer_presence online=false",
        |ev| {
            ev.kind == "peer_presence"
                && ev.device_id == Some(daemon_cfg.device_id)
                && ev.online == Some(false)
        },
    )
    .await;
    assert_eq!(offline_ev.role.as_deref(), Some("hybrid"));

    let _ = std::fs::remove_dir_all(&tmp_git_dir);
}
