//! C-ABI Bridge (`libcoda_apple_ffi.a` / `.dylib`) for Swift 6 Native Clients
//! across iOS 27 (iPhone & iPhone Duo), iPadOS 27, and macOS.
//! Exposes zero-knowledge key generation, Ed25519 signing, and E2EE + `zstd` frame sealing/opening.

use std::{
    ffi::{CStr, CString},
    os::raw::c_char,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::Mutex,
};

use coda_protocol::{E2eeSession, EncryptedPayload, PeerMessage, StoredDeviceIdentity};

pub struct CodaFfiSession {
    inner: Mutex<E2eeSession>,
}

fn to_c_string(s: String) -> *mut c_char {
    // Reject strings with interior NUL bytes instead of truncating or silently turning into "".
    match CString::new(s) {
        Ok(c) => c.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Frees any string returned by `coda_ffi_*` functions.
#[no_mangle]
pub unsafe extern "C" fn coda_ffi_string_free(ptr: *mut c_char) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if !ptr.is_null() {
            let _ = CString::from_raw(ptr);
        }
    }));
}

/// Generates a new `StoredDeviceIdentity` JSON string for Apple Keychain / Secure Enclave storage.
#[no_mangle]
pub unsafe extern "C" fn coda_ffi_generate_identity_json(device_name: *const c_char) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        let name = if device_name.is_null() {
            "Apple Device".to_string()
        } else {
            CStr::from_ptr(device_name).to_string_lossy().into_owned()
        };
        let id = StoredDeviceIdentity::generate(name);
        let Ok(json) = serde_json::to_string(&id) else {
            return std::ptr::null_mut();
        };
        to_c_string(json)
    }))
    .unwrap_or(std::ptr::null_mut())
}

/// Creates a stateful `CodaFfiSession` (`X25519 + HKDF-SHA256 + ChaCha20Poly1305 + zstd`).
#[no_mangle]
pub unsafe extern "C" fn coda_ffi_session_new(
    identity_json: *const c_char,
    peer_x25519_pub_b64: *const c_char,
    is_initiator: bool,
) -> *mut CodaFfiSession {
    catch_unwind(AssertUnwindSafe(|| {
        if identity_json.is_null() || peer_x25519_pub_b64.is_null() {
            return std::ptr::null_mut();
        }
        let id_str = CStr::from_ptr(identity_json).to_string_lossy();
        let peer_str = CStr::from_ptr(peer_x25519_pub_b64).to_string_lossy();

        let Ok(id) = serde_json::from_str::<StoredDeviceIdentity>(&id_str) else {
            return std::ptr::null_mut();
        };
        let Ok(secret) = id.x25519_secret() else {
            return std::ptr::null_mut();
        };
        let Ok(inner) = E2eeSession::derive(&secret, &peer_str, is_initiator) else {
            return std::ptr::null_mut();
        };
        Box::into_raw(Box::new(CodaFfiSession {
            inner: Mutex::new(inner),
        }))
    }))
    .unwrap_or(std::ptr::null_mut())
}

/// Creates a stateful Relay v2 `CodaFfiSession` bound to `session_nonce_b64` and device IDs.
#[no_mangle]
pub unsafe extern "C" fn coda_ffi_session_new_relay_v2(
    identity_json: *const c_char,
    peer_x25519_pub_b64: *const c_char,
    session_nonce_b64: *const c_char,
    client_device_id_utf8: *const c_char,
    daemon_device_id_utf8: *const c_char,
    is_initiator: bool,
    peer_supports_zstd: bool,
) -> *mut CodaFfiSession {
    catch_unwind(AssertUnwindSafe(|| {
        if identity_json.is_null()
            || peer_x25519_pub_b64.is_null()
            || session_nonce_b64.is_null()
            || client_device_id_utf8.is_null()
            || daemon_device_id_utf8.is_null()
        {
            return std::ptr::null_mut();
        }
        let id_str = CStr::from_ptr(identity_json).to_string_lossy();
        let peer_str = CStr::from_ptr(peer_x25519_pub_b64).to_string_lossy();
        let nonce_str = CStr::from_ptr(session_nonce_b64).to_string_lossy();
        let cid_str = CStr::from_ptr(client_device_id_utf8).to_string_lossy();
        let did_str = CStr::from_ptr(daemon_device_id_utf8).to_string_lossy();

        let Ok(id) = serde_json::from_str::<StoredDeviceIdentity>(&id_str) else {
            return std::ptr::null_mut();
        };
        let Ok(secret) = id.x25519_secret() else {
            return std::ptr::null_mut();
        };
        let Ok(cid) = cid_str.parse() else {
            return std::ptr::null_mut();
        };
        let Ok(did) = did_str.parse() else {
            return std::ptr::null_mut();
        };
        let Ok(inner) = E2eeSession::derive_relay_session(
            &secret,
            &peer_str,
            &nonce_str,
            cid,
            did,
            is_initiator,
            peer_supports_zstd,
        ) else {
            return std::ptr::null_mut();
        };
        Box::into_raw(Box::new(CodaFfiSession {
            inner: Mutex::new(inner),
        }))
    }))
    .unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub unsafe extern "C" fn coda_ffi_session_free(sess: *mut CodaFfiSession) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if !sess.is_null() {
            let _ = Box::from_raw(sess);
        }
    }));
}

/// Encrypts & optionally `zstd`-compresses a `PeerMessage` JSON string into an `EncryptedPayload` JSON string.
/// Thread-safe: acquires an internal mutex so full-duplex Swift tasks never alias `&mut E2eeSession`.
#[no_mangle]
pub unsafe extern "C" fn coda_ffi_session_seal(
    sess: *mut CodaFfiSession,
    peer_message_json: *const c_char,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if sess.is_null() || peer_message_json.is_null() {
            return std::ptr::null_mut();
        }
        let session = &*sess;
        let raw = CStr::from_ptr(peer_message_json).to_string_lossy();
        let Ok(msg) = serde_json::from_str::<PeerMessage>(&raw) else {
            return std::ptr::null_mut();
        };
        let Ok(mut guard) = session.inner.lock() else {
            return std::ptr::null_mut();
        };
        let Ok(sealed) = guard.seal_message(&msg) else {
            return std::ptr::null_mut();
        };
        let Ok(json) = serde_json::to_string(&sealed) else {
            return std::ptr::null_mut();
        };
        to_c_string(json)
    }))
    .unwrap_or(std::ptr::null_mut())
}

/// Decrypts & decompresses an `EncryptedPayload` JSON string back into a `PeerMessage` JSON string.
/// Thread-safe: acquires an internal mutex so full-duplex Swift tasks never alias `&mut E2eeSession`.
#[no_mangle]
pub unsafe extern "C" fn coda_ffi_session_open(
    sess: *mut CodaFfiSession,
    encrypted_payload_json: *const c_char,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if sess.is_null() || encrypted_payload_json.is_null() {
            return std::ptr::null_mut();
        }
        let session = &*sess;
        let raw = CStr::from_ptr(encrypted_payload_json).to_string_lossy();
        let Ok(payload) = serde_json::from_str::<EncryptedPayload>(&raw) else {
            return std::ptr::null_mut();
        };
        let Ok(mut guard) = session.inner.lock() else {
            return std::ptr::null_mut();
        };
        let Ok(msg) = guard.open_message(&payload) else {
            return std::ptr::null_mut();
        };
        let Ok(json) = serde_json::to_string(&msg) else {
            return std::ptr::null_mut();
        };
        to_c_string(json)
    }))
    .unwrap_or(std::ptr::null_mut())
}

/// Decodes base64-encoded `zstd` compressed payload bytes into a UTF-8 JSON C string (bounded against zip-bombs).
#[no_mangle]
pub unsafe extern "C" fn coda_ffi_zstd_decompress_b64(compressed_b64: *const c_char) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if compressed_b64.is_null() {
            return std::ptr::null_mut();
        }
        let raw = CStr::from_ptr(compressed_b64).to_string_lossy();
        let Ok(utf8) = coda_protocol::zstd_decompress_b64_to_utf8(&raw) else {
            return std::ptr::null_mut();
        };
        to_c_string(utf8)
    }))
    .unwrap_or(std::ptr::null_mut())
}

/// Binary-safe variant: decodes base64-encoded `zstd` compressed bytes and returns base64-encoded raw output
/// so embedded NUL bytes (`\0`) are never truncated across the C-ABI boundary.
#[no_mangle]
pub unsafe extern "C" fn coda_ffi_zstd_decompress_b64_to_b64(
    compressed_b64: *const c_char,
) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if compressed_b64.is_null() {
            return std::ptr::null_mut();
        }
        let raw = CStr::from_ptr(compressed_b64).to_string_lossy();
        let Ok(b64_out) = coda_protocol::zstd_decompress_b64_to_b64(&raw) else {
            return std::ptr::null_mut();
        };
        to_c_string(b64_out)
    }))
    .unwrap_or(std::ptr::null_mut())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ffi_roundtrip_and_null_safety() {
        unsafe {
            assert!(coda_ffi_session_new(std::ptr::null(), std::ptr::null(), true).is_null());
            assert!(coda_ffi_session_seal(std::ptr::null_mut(), std::ptr::null()).is_null());
            assert!(coda_ffi_session_open(std::ptr::null_mut(), std::ptr::null()).is_null());
            assert!(coda_ffi_zstd_decompress_b64(std::ptr::null()).is_null());
            assert!(coda_ffi_zstd_decompress_b64_to_b64(std::ptr::null()).is_null());
            coda_ffi_string_free(std::ptr::null_mut());
            coda_ffi_session_free(std::ptr::null_mut());

            let name = CString::new("iPhone Test").unwrap();
            let id1_ptr = coda_ffi_generate_identity_json(name.as_ptr());
            let id2_ptr = coda_ffi_generate_identity_json(std::ptr::null());
            assert!(!id1_ptr.is_null() && !id2_ptr.is_null());

            let id1_json = CStr::from_ptr(id1_ptr).to_str().unwrap().to_owned();
            let id2_json = CStr::from_ptr(id2_ptr).to_str().unwrap().to_owned();
            let id1: StoredDeviceIdentity = serde_json::from_str(&id1_json).unwrap();
            let id2: StoredDeviceIdentity = serde_json::from_str(&id2_json).unwrap();

            let p1 = CString::new(id1.x25519_public_b64.as_str()).unwrap();
            let p2 = CString::new(id2.x25519_public_b64.as_str()).unwrap();
            let c1 = CString::new(id1_json).unwrap();
            let c2 = CString::new(id2_json).unwrap();

            let sess_c = coda_ffi_session_new(c1.as_ptr(), p2.as_ptr(), true);
            let sess_d = coda_ffi_session_new(c2.as_ptr(), p1.as_ptr(), false);
            assert!(!sess_c.is_null() && !sess_d.is_null());

            let ping = CString::new(
                serde_json::to_string(&PeerMessage::Ping {
                    req_id: "ffi-1".into(),
                    sent_ms: 99,
                })
                .unwrap(),
            )
            .unwrap();
            let sealed_ptr = coda_ffi_session_seal(sess_c, ping.as_ptr());
            assert!(!sealed_ptr.is_null());
            let opened_ptr = coda_ffi_session_open(sess_d, sealed_ptr);
            assert!(!opened_ptr.is_null());

            let opened_json = CStr::from_ptr(opened_ptr).to_str().unwrap();
            let opened: PeerMessage = serde_json::from_str(opened_json).unwrap();
            assert_eq!(opened.request_id(), Some("ffi-1"));

            // Binary-safe zstd roundtrip containing embedded NUL bytes (`KLUv/...` frame for b"ab\0cd"):
            // Compressed with standard zstd for raw 5 bytes `ab\0cd`: "KLUv/SAFQAAAAGFiAGNk" -> test error & valid utf8 reject
            assert!(to_c_string("hello\0world".to_string()).is_null());

            coda_ffi_string_free(id1_ptr);
            coda_ffi_string_free(id2_ptr);
            coda_ffi_string_free(sealed_ptr);
            coda_ffi_string_free(opened_ptr);
            coda_ffi_session_free(sess_c);
            coda_ffi_session_free(sess_d);
        }
    }

    #[test]
    fn test_ffi_zstd_binary_safe_nul_bytes() {
        use base64::Engine as _;
        let payload = b"alpha\0beta\0gamma";
        let compressed = zstd::stream::encode_all(std::io::Cursor::new(payload), 3).unwrap();
        let compressed_b64 = base64::engine::general_purpose::STANDARD.encode(compressed);

        unsafe {
            let compressed_c = CString::new(compressed_b64).unwrap();
            let b64_ptr = coda_ffi_zstd_decompress_b64_to_b64(compressed_c.as_ptr());
            assert!(!b64_ptr.is_null());
            let decoded_b64 = CStr::from_ptr(b64_ptr).to_str().unwrap();
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(decoded_b64)
                .unwrap();
            assert_eq!(decoded.as_slice(), payload);
            coda_ffi_string_free(b64_ptr);

            // Legacy UTF-8 CStr helper must reject embedded NUL rather than truncating!
            assert!(coda_ffi_zstd_decompress_b64(compressed_c.as_ptr()).is_null());

            let bad = CString::new("not-valid-zstd!").unwrap();
            assert!(coda_ffi_zstd_decompress_b64_to_b64(bad.as_ptr()).is_null());
        }
    }

    #[test]
    fn test_ffi_concurrent_seal_and_open_is_thread_safe() {
        let id1 = StoredDeviceIdentity::generate("Client");
        let id2 = StoredDeviceIdentity::generate("Daemon");
        let cid = "11111111-1111-4111-8111-111111111111".to_string();
        let did = "22222222-2222-4222-8222-222222222222".to_string();
        let nonce = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [42u8; 16]);
        let c1 = CString::new(serde_json::to_string(&id1).unwrap()).unwrap();
        let c2 = CString::new(serde_json::to_string(&id2).unwrap()).unwrap();
        let p1 = CString::new(id1.x25519_public_b64.as_str()).unwrap();
        let p2 = CString::new(id2.x25519_public_b64.as_str()).unwrap();
        let n = CString::new(nonce).unwrap();
        let cid_c = CString::new(cid).unwrap();
        let did_c = CString::new(did).unwrap();

        unsafe {
            let sess_c = coda_ffi_session_new_relay_v2(
                c1.as_ptr(),
                p2.as_ptr(),
                n.as_ptr(),
                cid_c.as_ptr(),
                did_c.as_ptr(),
                true,
                true,
            ) as usize;
            let sess_d = coda_ffi_session_new_relay_v2(
                c2.as_ptr(),
                p1.as_ptr(),
                n.as_ptr(),
                cid_c.as_ptr(),
                did_c.as_ptr(),
                false,
                true,
            ) as usize;
            assert!(sess_c != 0 && sess_d != 0);

            let t1 = std::thread::spawn(move || {
                for i in 0..20u64 {
                    let msg = CString::new(
                        serde_json::to_string(&PeerMessage::Ping {
                            req_id: format!("c-{i}"),
                            sent_ms: i,
                        })
                        .unwrap(),
                    )
                    .unwrap();
                    let sealed = coda_ffi_session_seal(sess_c as *mut CodaFfiSession, msg.as_ptr());
                    assert!(!sealed.is_null());
                    let opened = coda_ffi_session_open(sess_d as *mut CodaFfiSession, sealed);
                    assert!(!opened.is_null());
                    coda_ffi_string_free(sealed);
                    coda_ffi_string_free(opened);
                }
            });
            let t2 = std::thread::spawn(move || {
                for i in 0..20u64 {
                    let msg = CString::new(
                        serde_json::to_string(&PeerMessage::Pong {
                            req_id: format!("d-{i}"),
                            sent_ms: i,
                            daemon_hostname: "mac".into(),
                            herdr_version: "0.1.20".into(),
                        })
                        .unwrap(),
                    )
                    .unwrap();
                    let sealed = coda_ffi_session_seal(sess_d as *mut CodaFfiSession, msg.as_ptr());
                    assert!(!sealed.is_null());
                    let opened = coda_ffi_session_open(sess_c as *mut CodaFfiSession, sealed);
                    assert!(!opened.is_null());
                    coda_ffi_string_free(sealed);
                    coda_ffi_string_free(opened);
                }
            });
            t1.join().unwrap();
            t2.join().unwrap();
            coda_ffi_session_free(sess_c as *mut CodaFfiSession);
            coda_ffi_session_free(sess_d as *mut CodaFfiSession);
        }
    }
}
