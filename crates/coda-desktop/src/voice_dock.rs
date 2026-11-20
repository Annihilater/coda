//! macOS Dock `Needs-You` attention badge & built-in Whisper voice dictation for `coda-desktop`.

use std::ffi::{c_char, c_void, CString};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use uuid::Uuid;

static LAST_DOCK_BADGE_COUNT: AtomicUsize = AtomicUsize::new(usize::MAX);

/// Updates the macOS Dock tile attention badge (`"1"`, `"2"`, ..., or `nil` when `0`).
/// Safe no-op in headless `--self-test` / `cargo test` where `NSApp` is null.
pub fn update_macos_dock_badge(needs_you_count: usize) {
    if LAST_DOCK_BADGE_COUNT.swap(needs_you_count, Ordering::Relaxed) == needs_you_count {
        return;
    }
    #[cfg(target_os = "macos")]
    unsafe {
        extern "C" {
            static NSApp: *mut c_void;
            fn objc_getClass(name: *const c_char) -> *mut c_void;
            fn sel_registerName(name: *const c_char) -> *mut c_void;
            fn objc_msgSend();
        }
        if NSApp.is_null() {
            return;
        }
        type MsgSend0 = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
        type MsgSend1Ptr = unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_char) -> *mut c_void;
        type MsgSendVoid1 = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void);

        let send0: MsgSend0 = std::mem::transmute(objc_msgSend as *const ());
        let send_str: MsgSend1Ptr = std::mem::transmute(objc_msgSend as *const ());
        let send_void1: MsgSendVoid1 = std::mem::transmute(objc_msgSend as *const ());

        let dock_tile = send0(NSApp, sel_registerName(c"dockTile".as_ptr()));
        if dock_tile.is_null() {
            return;
        }
        let set_badge_sel = sel_registerName(c"setBadgeLabel:".as_ptr());
        if needs_you_count == 0 {
            send_void1(dock_tile, set_badge_sel, std::ptr::null_mut());
        } else {
            let label = if needs_you_count > 99 {
                "99+".to_string()
            } else {
                needs_you_count.to_string()
            };
            if let Ok(c_label) = CString::new(label) {
                let ns_string_cls = objc_getClass(c"NSString".as_ptr());
                let utf8_sel = sel_registerName(c"stringWithUTF8String:".as_ptr());
                let ns_str = send_str(ns_string_cls, utf8_sel, c_label.as_ptr());
                send_void1(dock_tile, set_badge_sel, ns_str);
            }
        }
    }
}

/// Validates that a Whisper transcription base URL uses `https://` or loopback `http://`.
pub fn validated_transcription_endpoint(base_url: &str) -> Result<String, String> {
    let trimmed = base_url.trim().trim_end_matches('/');
    let base = if trimmed.is_empty() {
        "https://api.openai.com/v1"
    } else {
        trimmed
    };
    let full = if base.ends_with("/audio/transcriptions") {
        base.to_string()
    } else {
        format!("{base}/audio/transcriptions")
    };
    let parsed = reqwest::Url::parse(&full).map_err(|e| format!("Invalid Voice Base URL: {e}"))?;
    let host = parsed.host_str().unwrap_or("");
    let is_https = parsed.scheme() == "https";
    let is_loopback_http = parsed.scheme() == "http"
        && matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]");
    if !is_https && !is_loopback_http {
        return Err("Voice Base URL must use https:// (or http://127.0.0.1 for local Whisper)".into());
    }
    Ok(parsed.to_string())
}

/// Builds an RFC 7578 `multipart/form-data` payload for `/audio/transcriptions`.
pub fn build_whisper_multipart_body(
    boundary: &str,
    filename: &str,
    mime_type: &str,
    audio_bytes: &[u8],
    model: &str,
) -> Vec<u8> {
    let clean_model = if model.trim().is_empty() {
        "whisper-1"
    } else {
        model.trim()
    };
    let mut body = Vec::with_capacity(audio_bytes.len() + 512);
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: {mime_type}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(audio_bytes);
    body.extend_from_slice(b"\r\n");

    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"model\"\r\n\r\n");
    body.extend_from_slice(clean_model.as_bytes());
    body.extend_from_slice(b"\r\n");

    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"response_format\"\r\n\r\njson\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    body
}

pub async fn transcribe_audio_file(
    audio_path: &Path,
    base_url: &str,
    api_key: &str,
    model: &str,
) -> Result<String, String> {
    let endpoint = validated_transcription_endpoint(base_url)?;
    let audio_bytes = std::fs::read(audio_path)
        .map_err(|e| format!("Failed reading recorded audio: {e}"))?;
    if audio_bytes.len() < 128 {
        return Err("Recording too short or microphone muted.".into());
    }
    let ext = audio_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("m4a");
    let (fname, mime) = if ext == "wav" {
        ("voice.wav", "audio/wav")
    } else {
        ("voice.m4a", "audio/mp4")
    };
    let boundary = format!("----CodaVoiceBoundary{}", Uuid::new_v4().simple());
    let body = build_whisper_multipart_body(&boundary, fname, mime, &audio_bytes, model);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(35))
        .build()
        .map_err(|e| e.to_string())?;
    let mut req = client.post(&endpoint).header(
        "Content-Type",
        format!("multipart/form-data; boundary={boundary}"),
    );
    if !api_key.trim().is_empty() {
        req = req.bearer_auth(api_key.trim());
    }
    let resp = req
        .body(body)
        .send()
        .await
        .map_err(|e| format!("Transcription request failed: {e}"))?;
    let status = resp.status();
    let txt = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        let snippet: String = txt.chars().take(160).collect();
        return Err(format!("Whisper HTTP {}: {}", status.as_u16(), snippet));
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) {
        if let Some(t) = v.get("text").and_then(|x| x.as_str()) {
            return Ok(t.trim().to_string());
        }
    }
    Ok(txt.trim().to_string())
}

#[cfg(target_os = "macos")]
#[link(name = "AVFoundation", kind = "framework")]
#[link(name = "Foundation", kind = "framework")]
extern "C" {}

struct ActiveRecording {
    file_path: PathBuf,
    #[cfg(target_os = "macos")]
    objc_recorder: usize,
    #[cfg(not(target_os = "macos"))]
    child: Option<std::process::Child>,
}

/// Thread-safe desktop microphone recorder (`AVAudioRecorder` 16kHz Mono AAC on macOS).
#[derive(Default)]
pub struct DesktopVoiceController {
    inner: Mutex<Option<ActiveRecording>>,
    pub is_recording: AtomicBool,
    pub is_transcribing: AtomicBool,
}

impl DesktopVoiceController {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn start_recording(&self) -> Result<(), String> {
        let mut guard = self.inner.lock().map_err(|_| "voice lock poisoned")?;
        if guard.is_some() {
            return Ok(());
        }
        #[cfg(target_os = "macos")]
        {
            let file_path = std::env::temp_dir()
                .join(format!("coda-desktop-voice-{}.m4a", Uuid::new_v4().simple()));
            let recorder_ptr = unsafe { start_macos_avaudiorecorder(&file_path)? };
            self.is_recording.store(true, Ordering::Release);
            *guard = Some(ActiveRecording {
                file_path,
                objc_recorder: recorder_ptr,
            });
            Ok(())
        }
        #[cfg(not(target_os = "macos"))]
        {
            let file_path = std::env::temp_dir()
                .join(format!("coda-desktop-voice-{}.wav", Uuid::new_v4().simple()));
            let child = std::process::Command::new("ffmpeg")
                .args([
                    "-loglevel",
                    "quiet",
                    "-y",
                    "-f",
                    "pulse",
                    "-i",
                    "default",
                    "-ac",
                    "1",
                    "-ar",
                    "16000",
                    &file_path.to_string_lossy(),
                ])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map_err(|e| format!("Microphone capture requires ffmpeg on Linux: {e}"))?;
            self.is_recording.store(true, Ordering::Release);
            *guard = Some(ActiveRecording {
                file_path,
                child: Some(child),
            });
            Ok(())
        }
    }

    pub fn stop_recording_to_path(&self) -> Option<PathBuf> {
        let rec = self.inner.lock().ok()?.take()?;
        self.is_recording.store(false, Ordering::Release);
        #[cfg(target_os = "macos")]
        unsafe {
            stop_and_release_macos_avaudiorecorder(rec.objc_recorder);
        }
        #[cfg(not(target_os = "macos"))]
        {
            if let Some(mut c) = rec.child {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
        Some(rec.file_path)
    }
}

impl Drop for DesktopVoiceController {
    fn drop(&mut self) {
        if let Some(p) = self.stop_recording_to_path() {
            let _ = std::fs::remove_file(p);
        }
    }
}

#[cfg(target_os = "macos")]
unsafe fn start_macos_avaudiorecorder(out_path: &Path) -> Result<usize, String> {
    extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
    }
    type Msg0 = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
    type MsgStr = unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_char) -> *mut c_void;
    type MsgObj = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> *mut c_void;
    type MsgInt = unsafe extern "C" fn(*mut c_void, *mut c_void, i64) -> *mut c_void;
    type MsgF64 = unsafe extern "C" fn(*mut c_void, *mut c_void, f64) -> *mut c_void;
    type MsgDict = unsafe extern "C" fn(
        *mut c_void,
        *mut c_void,
        *const *mut c_void,
        *const *mut c_void,
        usize,
    ) -> *mut c_void;
    type MsgInitRec = unsafe extern "C" fn(
        *mut c_void,
        *mut c_void,
        *mut c_void,
        *mut c_void,
        *mut *mut c_void,
    ) -> *mut c_void;
    type MsgBool = unsafe extern "C" fn(*mut c_void, *mut c_void) -> bool;
    type MsgVoid = unsafe extern "C" fn(*mut c_void, *mut c_void);

    let send0: Msg0 = std::mem::transmute(objc_msgSend as *const ());
    let send_str: MsgStr = std::mem::transmute(objc_msgSend as *const ());
    let send_obj: MsgObj = std::mem::transmute(objc_msgSend as *const ());
    let send_int: MsgInt = std::mem::transmute(objc_msgSend as *const ());
    let send_f64: MsgF64 = std::mem::transmute(objc_msgSend as *const ());
    let send_dict: MsgDict = std::mem::transmute(objc_msgSend as *const ());
    let send_init: MsgInitRec = std::mem::transmute(objc_msgSend as *const ());
    let send_bool: MsgBool = std::mem::transmute(objc_msgSend as *const ());
    let send_void: MsgVoid = std::mem::transmute(objc_msgSend as *const ());

    let ns_string_cls = objc_getClass(c"NSString".as_ptr());
    let utf8_sel = sel_registerName(c"stringWithUTF8String:".as_ptr());
    let path_c = CString::new(out_path.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
    let ns_path = send_str(ns_string_cls, utf8_sel, path_c.as_ptr());

    let ns_url_cls = objc_getClass(c"NSURL".as_ptr());
    let file_url_sel = sel_registerName(c"fileURLWithPath:".as_ptr());
    let ns_url = send_obj(ns_url_cls, file_url_sel, ns_path);
    if ns_url.is_null() {
        return Err("Failed creating NSURL for voice recording".into());
    }

    let ns_num_cls = objc_getClass(c"NSNumber".as_ptr());
    let num_int_sel = sel_registerName(c"numberWithLongLong:".as_ptr());
    let num_f64_sel = sel_registerName(c"numberWithDouble:".as_ptr());

    // kAudioFormatMPEG4AAC = 'aac ' = 0x61616320 = 1633772320
    let k_fmt = send_str(ns_string_cls, utf8_sel, c"AVFormatIDKey".as_ptr());
    let v_fmt = send_int(ns_num_cls, num_int_sel, 1_633_772_320);
    let k_rate = send_str(ns_string_cls, utf8_sel, c"AVSampleRateKey".as_ptr());
    let v_rate = send_f64(ns_num_cls, num_f64_sel, 16_000.0);
    let k_ch = send_str(ns_string_cls, utf8_sel, c"AVNumberOfChannelsKey".as_ptr());
    let v_ch = send_int(ns_num_cls, num_int_sel, 1);

    let keys = [k_fmt, k_rate, k_ch];
    let vals = [v_fmt, v_rate, v_ch];
    let ns_dict_cls = objc_getClass(c"NSDictionary".as_ptr());
    let dict_sel = sel_registerName(c"dictionaryWithObjects:forKeys:count:".as_ptr());
    let settings = send_dict(ns_dict_cls, dict_sel, vals.as_ptr(), keys.as_ptr(), 3);

    let rec_cls = objc_getClass(c"AVAudioRecorder".as_ptr());
    if rec_cls.is_null() {
        return Err("AVAudioRecorder unavailable on this system".into());
    }
    let alloc = send0(rec_cls, sel_registerName(c"alloc".as_ptr()));
    let mut err_obj: *mut c_void = std::ptr::null_mut();
    let init_sel = sel_registerName(c"initWithURL:settings:error:".as_ptr());
    let recorder = send_init(alloc, init_sel, ns_url, settings, &raw mut err_obj);
    if recorder.is_null() || !err_obj.is_null() {
        if !recorder.is_null() {
            send_void(recorder, sel_registerName(c"release".as_ptr()));
        }
        return Err("Microphone initialization failed (check macOS Microphone permission)".into());
    }
    let prepared = send_bool(recorder, sel_registerName(c"prepareToRecord".as_ptr()));
    let started = prepared && send_bool(recorder, sel_registerName(c"record".as_ptr()));
    if !started {
        send_void(recorder, sel_registerName(c"release".as_ptr()));
        return Err("Microphone recording could not start (grant Microphone access in System Settings)".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(out_path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(recorder as usize)
}

#[cfg(target_os = "macos")]
unsafe fn stop_and_release_macos_avaudiorecorder(ptr: usize) {
    if ptr == 0 {
        return;
    }
    extern "C" {
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
    }
    type MsgVoid = unsafe extern "C" fn(*mut c_void, *mut c_void);
    let send_void: MsgVoid = std::mem::transmute(objc_msgSend as *const ());
    let recorder = ptr as *mut c_void;
    send_void(recorder, sel_registerName(c"stop".as_ptr()));
    send_void(recorder, sel_registerName(c"release".as_ptr()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dock_badge_noop_when_headless_and_url_validation() {
        // Must never crash when NSApp is null in cargo test
        update_macos_dock_badge(3);
        update_macos_dock_badge(0);

        assert_eq!(
            validated_transcription_endpoint("https://api.openai.com/v1").unwrap(),
            "https://api.openai.com/v1/audio/transcriptions"
        );
        assert_eq!(
            validated_transcription_endpoint("http://127.0.0.1:8080/v1/").unwrap(),
            "http://127.0.0.1:8080/v1/audio/transcriptions"
        );
        assert!(validated_transcription_endpoint("http://evil.example.com/v1").is_err());

        let mp = build_whisper_multipart_body("BOUND", "voice.m4a", "audio/mp4", b"ABC", "whisper-1");
        let mp_str = String::from_utf8_lossy(&mp);
        assert!(mp_str.contains("filename=\"voice.m4a\""));
        assert!(mp_str.contains("whisper-1"));
    }
}
