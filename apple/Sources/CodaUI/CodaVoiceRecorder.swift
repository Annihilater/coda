import AVFoundation
import CodaKit
import Foundation
#if canImport(Speech)
import Speech
#endif

public enum CodaVoiceRecorderError: LocalizedError, Sendable {
    case unbundledMicrophoneRequiresAppBundle
    case unbundledSpeechRequiresAppBundleOrWhisper
    case permissionDenied
    case speechPermissionDenied
    case invalidEndpoint(String)
    case recorderInitFailed
    case noActiveRecording
    case emptyAudio
    case transcriptionFailed(String)

    public var errorDescription: String? {
        switch self {
        case .unbundledMicrophoneRequiresAppBundle:
            return "Voice recording requires running the bundled Coda.app (with Info.plist microphone permission) rather than a bare CLI binary."
        case .unbundledSpeechRequiresAppBundleOrWhisper:
            return "Local Apple Speech requires running bundled Coda.app. Launch Coda.app or configure a Whisper API Key (locally or on your paired host)."
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

private final class SpeechCancellationCoordinator: @unchecked Sendable {
    private let lock = NSLock()
    private var isCancelled = false
    private var box: SpeechRecognitionOnceBox?
    private var recognitionTask: SFSpeechRecognitionTask?
    private var timeoutTask: Task<Void, Never>?

    func register(
        box: SpeechRecognitionOnceBox,
        recognitionTask: SFSpeechRecognitionTask,
        timeoutTask: Task<Void, Never>
    ) {
        lock.lock()
        let alreadyCancelled = isCancelled
        if !alreadyCancelled {
            self.box = box
            self.recognitionTask = recognitionTask
            self.timeoutTask = timeoutTask
        }
        lock.unlock()
        if alreadyCancelled {
            timeoutTask.cancel()
            recognitionTask.cancel()
            box.cancelAndFail(CancellationError())
        }
    }

    func cancelAll() {
        lock.lock()
        isCancelled = true
        let activeBox = box
        let activeRec = recognitionTask
        let activeTimeout = timeoutTask
        box = nil
        recognitionTask = nil
        timeoutTask = nil
        lock.unlock()

        activeTimeout?.cancel()
        activeRec?.cancel()
        activeBox?.cancelAndFail(CancellationError())
    }
}

private enum AppleNativeSpeechTranscriber {
    static func requestAuthorizationIfNeeded() async throws -> Bool {
        guard CodaVoiceRecorder.hasBundledSpeechUsageDescription else {
            throw CodaVoiceRecorderError.unbundledSpeechRequiresAppBundleOrWhisper
        }
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

    static func transcribeFile(
        at fileURL: URL,
        locale: CodaUILocale,
        onPartialTranscript: (@Sendable (String) -> Void)? = nil
    ) async throws -> String {
        let authorized = try await requestAuthorizationIfNeeded()
        try Task.checkCancellation()
        guard authorized else {
            throw CodaVoiceRecorderError.speechPermissionDenied
        }

        // Explicitly honor Coda's UI locale (`zh-CN` vs `en-US`) before falling back to OS locale.
        let preferredLocaleID = (locale == .zh) ? "zh-CN" : "en-US"
        let recognizer = SFSpeechRecognizer(locale: Locale(identifier: preferredLocaleID))
            ?? SFSpeechRecognizer(locale: Locale.current)
            ?? SFSpeechRecognizer()

        guard let recognizer, recognizer.isAvailable else {
            throw CodaVoiceRecorderError.transcriptionFailed(
                "Apple Speech recognizer is currently unavailable on this device."
            )
        }

        let request = SFSpeechURLRecognitionRequest(url: fileURL)
        request.shouldReportPartialResults = true
        request.addsPunctuation = true

        let coordinator = SpeechCancellationCoordinator()

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
                        let trimmed = formatted.trimmingCharacters(in: .whitespacesAndNewlines)
                        if !trimmed.isEmpty {
                            onPartialTranscript?(trimmed)
                        }
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
                coordinator.register(box: box, recognitionTask: recTask, timeoutTask: timeoutTask)
            }
        } onCancel: {
            coordinator.cancelAll()
        }
    }
}
#endif

/// Cross-platform (iOS 17+ / macOS 14+) voice recorder with TCC bundle guards, zero-config Apple `Speech`
/// (`SFSpeechRecognizer`) fallback, E2EE remote daemon Whisper delegation (`transcribe_audio`), and direct
/// Whisper-compatible `POST <base_url>/audio/transcriptions` client when a local API key is configured.
@MainActor
public final class CodaVoiceRecorder {
    private var recorder: AVAudioRecorder?
    private var recordingURL: URL?

    public nonisolated static var hasBundledMicrophoneUsageDescription: Bool {
        CodaVoiceEndpointValidator.isBundledWithUsageDescription(
            bundlePathExtension: Bundle.main.bundleURL.pathExtension,
            usageDescription: Bundle.main.object(forInfoDictionaryKey: "NSMicrophoneUsageDescription") as? String
        )
    }

    public nonisolated static var hasBundledSpeechUsageDescription: Bool {
        CodaVoiceEndpointValidator.isBundledWithUsageDescription(
            bundlePathExtension: Bundle.main.bundleURL.pathExtension,
            usageDescription: Bundle.main.object(forInfoDictionaryKey: "NSSpeechRecognitionUsageDescription") as? String
        )
    }

    public nonisolated static var hasNativeSpeechFallback: Bool {
        #if canImport(Speech)
        return true
        #else
        return false
        #endif
    }

    public nonisolated static func usesNativeSpeechFallback(for config: CodaVoiceModelConfig) -> Bool {
        !config.hasLocalPlaintextApiKey && !config.hasMaskedDaemonKey
    }

    public var isRecording: Bool {
        recorder?.isRecording == true
    }

    public init() {}

    /// Normalized live audio power (0.0...1.0) for live recording waveform indicators.
    public func currentNormalizedAudioLevel() -> Float {
        guard let rec = recorder, rec.isRecording else { return 0 }
        rec.updateMeters()
        let avgDb = rec.averagePower(forChannel: 0) // -160 .. 0 dBFS
        let clamped = max(-55.0, min(0.0, avgDb))
        return (clamped + 55.0) / 55.0
    }

    /// Validates that a voice API endpoint uses HTTPS (or localhost HTTP for local Whisper servers),
    /// rejects empty hosts (`https:///foo`), and places `/audio/transcriptions` before query parameters.
    public static func validatedTranscriptionURL(from rawBaseURL: String) throws -> URL {
        do {
            return try CodaVoiceEndpointValidator.validatedTranscriptionURL(from: rawBaseURL)
        } catch {
            throw CodaVoiceRecorderError.invalidEndpoint(error.localizedDescription)
        }
    }

    public func startRecording() async throws {
        cancelRecording()

        guard Self.hasBundledMicrophoneUsageDescription else {
            throw CodaVoiceRecorderError.unbundledMicrophoneRequiresAppBundle
        }

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
        rec.isMeteringEnabled = true
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
        locale: CodaUILocale = .en,
        onPartialTranscript: (@Sendable (String) -> Void)? = nil,
        daemonWhisperFallback: (@Sendable (Data, String) async throws -> String)? = nil
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

        let langHint = (locale == .zh) ? "zh" : "en"

        // Path 1: Remote host holds a saved Whisper API key (`••••••••`), while local plaintext key is absent
        if config.hasMaskedDaemonKey, !config.hasLocalPlaintextApiKey {
            if let daemonWhisperFallback {
                return try await daemonWhisperFallback(audioData, langHint)
            }
            #if canImport(Speech)
            if Self.hasBundledSpeechUsageDescription {
                return try await AppleNativeSpeechTranscriber.transcribeFile(
                    at: fileURL,
                    locale: locale,
                    onPartialTranscript: onPartialTranscript
                )
            }
            #endif
            throw CodaVoiceRecorderError.unbundledSpeechRequiresAppBundleOrWhisper
        }

        // Path 2: Zero-config empty key -> try Apple Native Speech first, with automatic E2EE daemon Whisper fallback
        if !config.hasLocalPlaintextApiKey {
            #if canImport(Speech)
            do {
                return try await AppleNativeSpeechTranscriber.transcribeFile(
                    at: fileURL,
                    locale: locale,
                    onPartialTranscript: onPartialTranscript
                )
            } catch is CancellationError {
                throw CancellationError()
            } catch {
                if let daemonWhisperFallback {
                    return try await daemonWhisperFallback(audioData, langHint)
                }
                throw error
            }
            #else
            if let daemonWhisperFallback {
                return try await daemonWhisperFallback(audioData, langHint)
            }
            throw CodaVoiceRecorderError.transcriptionFailed(
                "Whisper API Key is empty and Apple Speech framework is unavailable."
            )
            #endif
        }

        // Path 3: Local plaintext Whisper API key configured on device
        let apiKey = config.apiKey.trimmingCharacters(in: .whitespacesAndNewlines)
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
        appendField(name: "language", value: langHint)
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
        guard Self.hasBundledMicrophoneUsageDescription else {
            return false
        }
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
