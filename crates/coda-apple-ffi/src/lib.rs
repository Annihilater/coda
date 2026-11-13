//! C-ABI Bridge (`libcoda_apple_ffi.a` / `.dylib`) for Swift 6 Native Clients
//! across iOS 27 (iPhone & iPhone Duo), iPadOS 27, and macOS.
//! Exposes zero-knowledge key generation, Ed25519 signing, and E2EE + `zstd` frame sealing/opening.

use std::{
    ffi::{CStr, CString},
    os::raw::c_char,
};

use coda_protocol::{E2eeSession, EncryptedPayload, PeerMessage, StoredDeviceIdentity};

pub struct CodaFfiSession {
    inner: E2eeSession,
}

fn to_c_string(s: String) -> *mut c_char {
    CString::new(s).unwrap_or_default().into_raw()
}

/// Frees any string returned by `coda_ffi_*` functions.
#[no_mangle]
pub unsafe extern "C" fn coda_ffi_string_free(ptr: *mut c_char) {
    if !ptr.is_null() {
        let _ = CString::from_raw(ptr);
    }
}

/// Generates a new `StoredDeviceIdentity` JSON string for Apple Keychain / Secure Enclave storage.
#[no_mangle]
pub unsafe extern "C" fn coda_ffi_generate_identity_json(device_name: *const c_char) -> *mut c_char {
    let name = if device_name.is_null() {
        "Apple Device".to_string()
    } else {
        CStr::from_ptr(device_name).to_string_lossy().into_owned()
    };
    let id = StoredDeviceIdentity::generate(name);
    to_c_string(serde_json::to_string(&id).unwrap_or_default())
}

/// Creates a stateful `CodaFfiSession` (`X25519 + HKDF-SHA256 + ChaCha20Poly1305 + zstd`).
#[no_mangle]
pub unsafe extern "C" fn coda_ffi_session_new(
    identity_json: *const c_char,
    peer_x25519_pub_b64: *const c_char,
    is_initiator: bool,
) -> *mut CodaFfiSession {
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
    Box::into_raw(Box::new(CodaFfiSession { inner }))
}

#[no_mangle]
pub unsafe extern "C" fn coda_ffi_session_free(sess: *mut CodaFfiSession) {
    if !sess.is_null() {
        let _ = Box::from_raw(sess);
    }
}

/// Encrypts & optionally `zstd`-compresses a `PeerMessage` JSON string into an `EncryptedPayload` JSON string.
#[no_mangle]
pub unsafe extern "C" fn coda_ffi_session_seal(
    sess: *mut CodaFfiSession,
    peer_message_json: *const c_char,
) -> *mut c_char {
    if sess.is_null() || peer_message_json.is_null() {
        return std::ptr::null_mut();
    }
    let session = &mut *sess;
    let raw = CStr::from_ptr(peer_message_json).to_string_lossy();
    let Ok(msg) = serde_json::from_str::<PeerMessage>(&raw) else {
        return std::ptr::null_mut();
    };
    let Ok(sealed) = session.inner.seal_message(&msg) else {
        return std::ptr::null_mut();
    };
    to_c_string(serde_json::to_string(&sealed).unwrap_or_default())
}

/// Decrypts & decompresses an `EncryptedPayload` JSON string back into a `PeerMessage` JSON string.
#[no_mangle]
pub unsafe extern "C" fn coda_ffi_session_open(
    sess: *mut CodaFfiSession,
    encrypted_payload_json: *const c_char,
) -> *mut c_char {
    if sess.is_null() || encrypted_payload_json.is_null() {
        return std::ptr::null_mut();
    }
    let session = &mut *sess;
    let raw = CStr::from_ptr(encrypted_payload_json).to_string_lossy();
    let Ok(payload) = serde_json::from_str::<EncryptedPayload>(&raw) else {
        return std::ptr::null_mut();
    };
    let Ok(msg) = session.inner.open_message(&payload) else {
        return std::ptr::null_mut();
    };
    to_c_string(serde_json::to_string(&msg).unwrap_or_default())
}

/// Decodes base64-encoded `zstd` compressed payload bytes into a UTF-8 JSON C string.
#[no_mangle]
pub unsafe extern "C" fn coda_ffi_zstd_decompress_b64(compressed_b64: *const c_char) -> *mut c_char {
    if compressed_b64.is_null() {
        return std::ptr::null_mut();
    }
    let raw = CStr::from_ptr(compressed_b64).to_string_lossy();
    let Ok(utf8) = coda_protocol::zstd_decompress_b64_to_utf8(&raw) else {
        return std::ptr::null_mut();
    };
    to_c_string(utf8)
}
