use std::{net::SocketAddr, sync::OnceLock};

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use coda_daemon::{
    discover_local_endpoints_with_port, list_account_hosts, mint_peer_relay_config,
    poll_device_login_once, run_daemon_once_with_p2p, spawn_direct_p2p_listener,
    spawn_lan_udp_beacon, start_device_login_flow, DirectP2pClient, SmartPeerClient,
    SmartRelayFallbackConfig,
};
use coda_herdr::{discover_hc_binary, HerdrClient};
use coda_protocol::{
    E2eeSession, NetworkEndpointCandidate, PeerMessage, RelayWireFrame, StoredDeviceIdentity,
    TrafficLane, VoiceModelConfig,
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

#[tokio::test]
async fn test_multi_tenant_postgres_and_raw_tcp_e2ee_herdr_tunnel() {
    let _guard = DB_TEST_LOCK.get_or_init(|| Mutex::new(())).lock().await;
    let db_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://vampire@localhost:5432/coda_test".to_string());

    // Ensure schema migrations exist on a brand-new CI database BEFORE truncating tables
    let _ = coda_server::db::Db::connect_and_migrate(&db_url)
        .await
        .expect("PostgreSQL coda_test should be reachable and migrate cleanly");

    // Clean test tables before run
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
    let iphone_duo_id = StoredDeviceIdentity::generate("Vampire iPhone Duo");
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

    // Derive Zero-Knowledge E2EE session on iPhone Duo
    let mut e2ee = E2eeSession::derive(
        &iphone_duo_id.x25519_secret().unwrap(),
        &daemon_cfg.identity.x25519_public_b64,
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
        sender_x25519_pub_b64: Some(iphone_duo_id.x25519_public_b64.clone()),
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
                }
                .encode_length_delimited()
                .unwrap(),
            )
            .await
            .unwrap();

        let launch_pkt = next_route_packet(&mut raw_stream).await;
        let term_addr = if let RelayWireFrame::RoutePacket { e2ee: enc_res, .. } = launch_pkt {
            match e2ee.open_message(&enc_res).unwrap() {
                PeerMessage::LaunchAgentResult {
                    ok: true,
                    terminal: Some(t),
                    ..
                } => t.address,
                other => panic!("unexpected launch result: {other:?}"),
            }
        } else {
            panic!("expected launch RoutePacket");
        };

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

    let mut p2p_client = DirectP2pClient::connect_direct(
        &p2p_addr.to_string(),
        &iphone_duo_id,
        iphone_dev.id,
        daemon_cfg.device_id,
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
    let lan_ws_url = format!("ws://{p2p_addr}");
    let (mut lan_ws, _) = connect_async(&lan_ws_url).await.unwrap();
    let ts_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let web_id = StoredDeviceIdentity::generate("Browser Web Console");
    let web_client_id = iphone_dev.id;
    let signed_msg = format!("p2p:{web_client_id}:{}:{ts_ms}", daemon_cfg.device_id);
    let sig_b64 = web_id.sign(signed_msg.as_bytes()).unwrap();

    lan_ws
        .send(WsMsg::Text(
            serde_json::to_string(&RelayWireFrame::DirectP2pHello {
                client_device_id: web_client_id,
                target_device_id: daemon_cfg.device_id,
                client_ed25519_pub_b64: web_id.ed25519_public_b64.clone(),
                client_x25519_pub_b64: web_id.x25519_public_b64.clone(),
                timestamp_ms: ts_ms,
                signature_b64: sig_b64,
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
    match ack_frame {
        RelayWireFrame::DirectP2pAck {
            ok: true,
            daemon_device_id,
            path_kind,
            ..
        } => {
            assert_eq!(daemon_device_id, daemon_cfg.device_id);
            assert_eq!(path_kind, "lan_ws");
        }
        other => panic!("unexpected direct WS ack: {other:?}"),
    }

    let mut lan_ws_e2ee = E2eeSession::derive(
        &web_id.x25519_secret().unwrap(),
        &daemon_cfg.identity.x25519_public_b64,
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
                sender_x25519_pub_b64: Some(web_id.x25519_public_b64.clone()),
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
        vec![],
        Some(udp_beacon_addr),
        Some(SmartRelayFallbackConfig {
            raw_tcp_addr: Some(server.raw_tcp_addr.to_string()),
            wss_url: None,
            relay_ticket: failover_ticket.ticket,
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

    let _ = coda_server::db::Db::connect_and_migrate(&db_url)
        .await
        .expect("PostgreSQL coda_test should be reachable");

    let (pg, conn) = tokio_postgres::connect(&db_url, tokio_postgres::NoTls)
        .await
        .unwrap();
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
        b"cf-wss-device-flow-secret-2026".to_vec(),
        None,
        None,
        ServerOptions {
            open_registration: true,
            raw_tcp_public_endpoint: None,
            release_version: "v0.3.4".into(),
            github_repo: "Annihilater/coda".into(),
            ios_beta_url: Some("https://testflight.apple.com/join/CodaBeta".into()),
        },
    )
    .await
    .unwrap();

    let base = format!("http://{}", server.http_addr);
    let http = reqwest::Client::new();

    // 1. Public unauthenticated portal metadata + Cloudflare forwarded header detection
    let site: serde_json::Value = http
        .get(format!("{base}/api/v1/public/site-info"))
        .header("x-forwarded-host", "codahub.dev")
        .header("cf-visitor", r#"{"scheme":"https"}"#)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(site["open_registration"], true);
    assert_eq!(
        site["install_command"],
        "curl -fsSL https://codahub.dev/install.sh | sh"
    );
    assert!(site["downloads"]["desktop_macos_dmg"]
        .as_str()
        .unwrap()
        .ends_with("Coda-Desktop-macos-aarch64-v0.3.4.dmg"));
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
        &linux_cfg.identity.x25519_public_b64,
    )
    .await
    .unwrap();
    let mut smart = SmartPeerClient::connect_auto(
        mac_cfg.identity.clone(),
        mac_cfg.device_id,
        linux_cfg.device_id,
        vec![],
        None,
        Some(SmartRelayFallbackConfig {
            raw_tcp_addr: None,
            wss_url: rcfg.wss_url,
            relay_ticket: rcfg.relay_ticket,
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
