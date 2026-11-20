import AVFoundation
import CodaKit
import Foundation

public enum CodaVoiceRecorderError: LocalizedError, Sendable {
    case permissionDenied
    case invalidEndpoint(String)
    case recorderInitFailed
    case noActiveRecording
    case emptyAudio
    case transcriptionFailed(String)

    public var errorDescription: String? {
        switch self {
        case .permissionDenied:
            return "Microphone permission denied. Enable microphone access in System/App Settings."
        case .invalidEndpoint(let msg):
            return msg
        case .recorderInitFailed:
            return "Could not initialize audio recorder."
        case .noActiveRecording:
            return "No active voice recording."
        case .emptyAudio:
            return "Recorded audio is empty."
        case .transcriptionFailed(let msg):
            return msg
        }
    }
}

/// Real cross-platform (iOS 17+ / macOS 14+) voice recorder + Whisper-compatible
/// `POST <base_url>/audio/transcriptions` client.
@MainActor
public final class CodaVoiceRecorder {
    private var recorder: AVAudioRecorder?
    private var recordingURL: URL?

    public var isRecording: Bool {
        recorder?.isRecording == true
    }

    public init() {}

    /// Validates that a voice API endpoint uses HTTPS (or localhost HTTP for local Whisper servers).
    public static func validatedTranscriptionURL(from rawBaseURL: String) throws -> URL {
        let trimmed = rawBaseURL.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, let parsed = URL(string: trimmed), let scheme = parsed.scheme?.lowercased() else {
            throw CodaVoiceRecorderError.invalidEndpoint("Invalid voice API base URL.")
        }
        let host = (parsed.host ?? "").lowercased()
        let isLoopback = host == "localhost" || host == "127.0.0.1" || host == "::1" || host == "[::1]"
        if scheme != "https" && !(scheme == "http" && isLoopback) {
            throw CodaVoiceRecorderError.invalidEndpoint(
                "Voice API URL must use https:// (or http://localhost for local Whisper)."
            )
        }
        let cleanStr = trimmed.hasSuffix("/") ? String(trimmed.dropLast()) : trimmed
        let finalStr = cleanStr.hasSuffix("/audio/transcriptions")
            ? cleanStr
            : "\(cleanStr)/audio/transcriptions"
        guard let endpoint = URL(string: finalStr) else {
            throw CodaVoiceRecorderError.invalidEndpoint("Could not construct /audio/transcriptions URL.")
        }
        return endpoint
    }

    public func startRecording() async throws {
        cancelRecording()

        let granted = await requestMicrophonePermission()
        try Task.checkCancellation()
        guard granted else {
            throw CodaVoiceRecorderError.permissionDenied
        }

        #if os(iOS)
        let session = AVAudioSession.sharedInstance()
        try session.setCategory(
            .playAndRecord,
            mode: .default,
            options: [.defaultToSpeaker, .allowBluetooth]
        )
        try session.setActive(true, options: .notifyOthersOnDeactivation)
        #endif

        let tempURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("coda-voice-\(UUID().uuidString.lowercased()).m4a")

        let recordSettings: [String: Any] = [
            AVFormatIDKey: Int(kAudioFormatMPEG4AAC),
            AVSampleRateKey: 16_000.0,
            AVNumberOfChannelsKey: 1,
            AVEncoderAudioQualityKey: AVAudioQuality.high.rawValue,
        ]

        let rec = try AVAudioRecorder(url: tempURL, settings: recordSettings)
        rec.prepareToRecord()
        guard rec.record() else {
            try? FileManager.default.removeItem(at: tempURL)
            throw CodaVoiceRecorderError.recorderInitFailed
        }
        self.recorder = rec
        self.recordingURL = tempURL
    }

    public func cancelRecording() {
        recorder?.stop()
        recorder = nil
        if let url = recordingURL {
            try? FileManager.default.removeItem(at: url)
            recordingURL = nil
        }
        #if os(iOS)
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
        #endif
    }

    public func stopAndTranscribe(config: CodaVoiceModelConfig) async throws -> String {
        guard let rec = recorder, let fileURL = recordingURL else {
            throw CodaVoiceRecorderError.noActiveRecording
        }
        rec.stop()
        self.recorder = nil
        self.recordingURL = nil

        #if os(iOS)
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
        #endif

        defer {
            try? FileManager.default.removeItem(at: fileURL)
        }

        let audioData = try Data(contentsOf: fileURL)
        guard audioData.count > 64 else {
            throw CodaVoiceRecorderError.emptyAudio
        }

        let endpoint = try Self.validatedTranscriptionURL(from: config.baseURL)
        let model = config.model.trimmingCharacters(in: .whitespacesAndNewlines)
        let effectiveModel = model.isEmpty ? "whisper-1" : model
        let apiKey = config.apiKey.trimmingCharacters(in: .whitespacesAndNewlines)

        let boundary = "CodaVoiceBoundary-\(UUID().uuidString.replacingOccurrences(of: "-", with: ""))"
        var body = Data()

        func appendField(name: String, value: String) {
            body.append(Data("--\(boundary)\r\n".utf8))
            body.append(Data("Content-Disposition: form-data; name=\"\(name)\"\r\n\r\n".utf8))
            body.append(Data("\(value)\r\n".utf8))
        }

        appendField(name: "model", value: effectiveModel)
        appendField(name: "response_format", value: "json")

        body.append(Data("--\(boundary)\r\n".utf8))
        body.append(Data("Content-Disposition: form-data; name=\"file\"; filename=\"dictation.m4a\"\r\n".utf8))
        body.append(Data("Content-Type: audio/m4a\r\n\r\n".utf8))
        body.append(audioData)
        body.append(Data("\r\n--\(boundary)--\r\n".utf8))

        var req = URLRequest(url: endpoint)
        req.httpMethod = "POST"
        req.timeoutInterval = 45
        req.setValue("multipart/form-data; boundary=\(boundary)", forHTTPHeaderField: "Content-Type")
        if !apiKey.isEmpty {
            req.setValue("Bearer \(apiKey)", forHTTPHeaderField: "Authorization")
        }
        req.httpBody = body

        let (data, resp) = try await URLSession.shared.data(for: req)
        guard let http = resp as? HTTPURLResponse else {
            throw CodaVoiceRecorderError.transcriptionFailed("Invalid response from voice endpoint.")
        }
        guard (200..<300).contains(http.statusCode) else {
            let apiErr = CodaAPIError.fromResponse(
                data: data,
                response: resp,
                fallback: "Transcription failed (HTTP \(http.statusCode))"
            )
            throw CodaVoiceRecorderError.transcriptionFailed(apiErr.localizedDescription)
        }

        if let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
           let text = obj["text"] as? String {
            return text.trimmingCharacters(in: .whitespacesAndNewlines)
        }
        let plain = String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
        if !plain.isEmpty {
            return plain
        }
        throw CodaVoiceRecorderError.transcriptionFailed("Voice API returned empty transcript.")
    }

    private func requestMicrophonePermission() async -> Bool {
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized:
            return true
        case .notDetermined:
            return await AVCaptureDevice.requestAccess(for: .audio)
        case .denied, .restricted:
            return false
        @unknown default:
            return false
        }
    }
}
