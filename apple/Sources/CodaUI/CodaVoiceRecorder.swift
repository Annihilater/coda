import AVFoundation
import CodaKit
import Foundation
#if canImport(Speech)
import Speech
#endif

public enum CodaVoiceRecorderError: LocalizedError, Sendable {
    case permissionDenied
    case speechPermissionDenied
    case invalidEndpoint(String)
    case recorderInitFailed
    case noActiveRecording
    case emptyAudio
    case transcriptionFailed(String)

    public var errorDescription: String? {
        switch self {
        case .permissionDenied:
            return "Microphone permission denied. Enable microphone access in System/App Settings."
        case .speechPermissionDenied:
            return "Apple Speech Recognition permission denied. Enable Speech Recognition in System/App Settings, or configure a Whisper API Key."
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

#if canImport(Speech)
private final class SpeechAuthOnceBox: @unchecked Sendable {
    private let lock = NSLock()
    private var continuation: CheckedContinuation<Bool, Never>?

    init(_ continuation: CheckedContinuation<Bool, Never>) {
        self.continuation = continuation
    }

    func finish(_ granted: Bool) {
        lock.lock()
        let cont = continuation
        continuation = nil
        lock.unlock()
        cont?.resume(returning: granted)
    }
}

private final class SpeechRecognitionOnceBox: @unchecked Sendable {
    private let lock = NSLock()
    private var continuation: CheckedContinuation<String, Error>?
    private var latestTranscript: String = ""
    private var task: SFSpeechRecognitionTask?

    init(_ continuation: CheckedContinuation<String, Error>) {
        self.continuation = continuation
    }

    func attachTask(_ recognitionTask: SFSpeechRecognitionTask) {
        lock.lock()
        let alreadyDone = (continuation == nil)
        if !alreadyDone {
            self.task = recognitionTask
        }
        lock.unlock()
        if alreadyDone {
            recognitionTask.cancel()
        }
    }

    func updatePartial(_ text: String) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        lock.lock()
        latestTranscript = trimmed
        lock.unlock()
    }

    func finishFinal(_ text: String?) {
        lock.lock()
        if let text {
            let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
            if !trimmed.isEmpty {
                latestTranscript = trimmed
            }
        }
        let finalTranscript = latestTranscript
        let cont = continuation
        continuation = nil
        let activeTask = task
        task = nil
        lock.unlock()

        activeTask?.cancel()
        guard let cont else { return }
        if finalTranscript.isEmpty {
            cont.resume(throwing: CodaVoiceRecorderError.transcriptionFailed("No speech detected in recording."))
        } else {
            cont.resume(returning: finalTranscript)
        }
    }

    func finishWithError(_ error: Error) {
        lock.lock()
        let fallback = latestTranscript
        let cont = continuation
        continuation = nil
        let activeTask = task
        task = nil
        lock.unlock()

        activeTask?.cancel()
        guard let cont else { return }
        if !fallback.isEmpty {
            cont.resume(returning: fallback)
        } else {
            cont.resume(throwing: CodaVoiceRecorderError.transcriptionFailed(error.localizedDescription))
        }
    }

    func cancelAndFail(_ error: Error) {
        lock.lock()
        let cont = continuation
        continuation = nil
        let activeTask = task
        task = nil
        lock.unlock()

        activeTask?.cancel()
        cont?.resume(throwing: error)
    }
}

private enum AppleNativeSpeechTranscriber {
    static func requestAuthorizationIfNeeded() async -> Bool {
        switch SFSpeechRecognizer.authorizationStatus() {
        case .authorized:
            return true
        case .notDetermined:
            return await withCheckedContinuation { cont in
                let box = SpeechAuthOnceBox(cont)
                SFSpeechRecognizer.requestAuthorization { status in
                    box.finish(status == .authorized)
                }
            }
        case .denied, .restricted:
            return false
        @unknown default:
            return false
        }
    }

    static func transcribeFile(at fileURL: URL, locale: CodaUILocale) async throws -> String {
        let authorized = await requestAuthorizationIfNeeded()
        try Task.checkCancellation()
        guard authorized else {
            throw CodaVoiceRecorderError.speechPermissionDenied
        }

        let preferredLocaleID = (locale == .zh) ? "zh-CN" : Locale.current.identifier
        let recognizer = SFSpeechRecognizer(locale: Locale(identifier: preferredLocaleID))
            ?? SFSpeechRecognizer(locale: Locale.current)
            ?? SFSpeechRecognizer(locale: Locale(identifier: "en-US"))
            ?? SFSpeechRecognizer()

        guard let recognizer, recognizer.isAvailable else {
            throw CodaVoiceRecorderError.transcriptionFailed(
                "Apple Speech recognizer is currently unavailable on this device."
            )
        }

        let request = SFSpeechURLRecognitionRequest(url: fileURL)
        request.shouldReportPartialResults = false
        request.addsPunctuation = true

        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (cont: CheckedContinuation<String, Error>) in
                let box = SpeechRecognitionOnceBox(cont)
                let timeoutTask = Task.detached {
                    try? await Task.sleep(nanoseconds: 30_000_000_000)
                    guard !Task.isCancelled else { return }
                    box.cancelAndFail(
                        CodaVoiceRecorderError.transcriptionFailed("Apple Speech recognition timed out.")
                    )
                }
                let recTask = recognizer.recognitionTask(with: request) { result, error in
                    if let result {
                        let formatted = result.bestTranscription.formattedString
                        box.updatePartial(formatted)
                        if result.isFinal {
                            timeoutTask.cancel()
                            box.finishFinal(formatted)
                            return
                        }
                    }
                    if let error {
                        timeoutTask.cancel()
                        box.finishWithError(error)
                    }
                }
                box.attachTask(recTask)
            }
        } onCancel: {
            // CheckedContinuation inside SpeechRecognitionOnceBox is cleaned up via timeout/task completion.
        }
    }
}
#endif

/// Real cross-platform (iOS 17+ / macOS 14+) voice recorder with zero-config Apple `Speech`
/// (`SFSpeechRecognizer`) fallback when `apiKey` is empty, plus Whisper-compatible
/// `POST <base_url>/audio/transcriptions` client when `apiKey` is configured.
@MainActor
public final class CodaVoiceRecorder {
    private var recorder: AVAudioRecorder?
    private var recordingURL: URL?

    public static var hasNativeSpeechFallback: Bool {
        #if canImport(Speech)
        return true
        #else
        return false
        #endif
    }

    public static func usesNativeSpeechFallback(for config: CodaVoiceModelConfig) -> Bool {
        config.apiKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

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

    public func stopAndTranscribe(
        config: CodaVoiceModelConfig,
        locale: CodaUILocale = .en
    ) async throws -> String {
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

        let apiKey = config.apiKey.trimmingCharacters(in: .whitespacesAndNewlines)
        if apiKey.isEmpty {
            #if canImport(Speech)
            return try await AppleNativeSpeechTranscriber.transcribeFile(at: fileURL, locale: locale)
            #else
            throw CodaVoiceRecorderError.transcriptionFailed(
                "Whisper API Key is empty and Apple Speech framework is unavailable."
            )
            #endif
        }

        let endpoint = try Self.validatedTranscriptionURL(from: config.baseURL)
        let model = config.model.trimmingCharacters(in: .whitespacesAndNewlines)
        let effectiveModel = model.isEmpty ? "whisper-1" : model

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
        req.setValue("Bearer \(apiKey)", forHTTPHeaderField: "Authorization")
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
