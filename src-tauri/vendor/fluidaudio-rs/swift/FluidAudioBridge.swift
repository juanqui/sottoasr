import AVFoundation
import Darwin
import FluidAudio
import Foundation

// Prepared independently so downloads and CoreML compilation never own the
// resident ASR engine. Rust transfers this retained object only when ready.
private final class VocabularyModels {
    let models: CtcModels
    let tokenizer: CtcTokenizer
    let directory: URL

    init(models: CtcModels, tokenizer: CtcTokenizer, directory: URL) {
        self.models = models
        self.tokenizer = tokenizer
        self.directory = directory
    }

    /// Repairs unreadable tokenizer metadata and, when `fetchModels` is set,
    /// fetches missing files at the pinned revision. Present model files are
    /// never replaced, and nothing loads here: Rust verifies the files first.
    static func download(to directory: URL, fetchModels: Bool) throws {
        try requireSdkFolder(directory)
        try waitForAsync {
            _ = try await VocabularyCache.tokenizer(at: directory, allowDownload: true)
            if fetchModels {
                // Download without ModelHub.loadModels' destructive cache-purge
                // fallback. An invalid model surfaces an error and stays on disk.
                try await ModelHub.download(.parakeetCtc110m, to: directory.deletingLastPathComponent(),
                    config: DownloadConfig(timeout: 120, minStallBytes: 256 * 1024, stallWindow: 30))
            }
        }
    }

    /// Loads verified local files. Never downloads.
    static func prepare(at directory: URL) throws -> VocabularyModels {
        try requireSdkFolder(directory)
        return try waitForAsync {
            let tokenizer = try await VocabularyCache.tokenizer(at: directory, allowDownload: false)
            let models = try await CtcModels.loadDirect(from: directory)
            return VocabularyModels(models: models, tokenizer: tokenizer, directory: directory)
        }
    }

    /// ModelHub writes into the parent folder under the repository's folder
    /// name, so any other leaf would verify one folder and download to another.
    private static func requireSdkFolder(_ directory: URL) throws {
        guard directory.lastPathComponent == Repo.parakeetCtc110m.folderName else {
            throw NSError(domain: "SottoASR", code: 5, userInfo: [NSLocalizedDescriptionKey:
                "The vocabulary model folder must be named \(Repo.parakeetCtc110m.folderName)."])
        }
    }
}

// Rust exclusively owns this handle and waits for each async operation before
// making another call. Swift actors own all model and decoder work.
private final class AsrBridge {
    var manager: AsrManager?
    var vocabulary: VocabularyModels?

    func unloadVocabulary() { vocabulary = nil }

    func initialize(at directory: URL) throws {
        try waitForAsync {
            let models = try await AsrCache.load(at: directory)
            let manager = AsrManager()
            try await manager.loadModels(models)
            self.manager = manager
        }
    }

    func transcribe(_ path: String, terms: [String]) throws -> (String, Double, Double, String) {
        guard let manager else {
            throw NSError(domain: "SottoASR", code: 1,
                          userInfo: [NSLocalizedDescriptionKey: "ASR has not been initialized"])
        }
        return try waitForAsync {
            let url = URL(fileURLWithPath: path)
            let file = try AVAudioFile(forReading: url)
            let duration = Double(file.length) / file.processingFormat.sampleRate
            // Never carry predictor state from a previous independent recording.
            var state = try TdtDecoderState()
            let vocabularyModels = terms.isEmpty ? nil : self.vocabulary
            // At or below its streaming threshold, AsrManager.transcribe(url)
            // decodes the whole file with this same stateless converter and
            // passes the samples on, so one decode here feeds both consumers
            // identical input. Longer files stream from disk through another
            // converter; they keep the file path and decode again for the spotter.
            let config = ASRConfig.default
            let estimatedSamples = Int((Double(file.length)
                * (Double(config.sampleRate) / file.processingFormat.sampleRate)).rounded(.up))
            let decoded: [Float]?
            let result: ASRResult
            if vocabularyModels != nil,
               !(config.streamingEnabled && estimatedSamples > config.streamingThreshold) {
                let samples = try AudioConverter().resampleAudioFile(url)
                decoded = samples
                result = try await manager.transcribe(samples, decoderState: &state)
            } else {
                decoded = nil
                result = try await manager.transcribe(url, decoderState: &state)
            }
            var candidates = "[]"
            if let vocabularyModels, let timings = result.tokenTimings, !timings.isEmpty {
                // Auxiliary failures preserve the complete original ASR result.
                // Only candidate evidence crosses FFI; Rust owns the acceptance policy.
                do {
                    let models = vocabularyModels.models
                    let tokenizer = vocabularyModels.tokenizer
                    let vocabularyTerms = terms.compactMap { term -> CustomVocabularyTerm? in
                        let ids = tokenizer.encode(term)
                        guard !ids.isEmpty, !ids.contains(0),
                              term.allSatisfy({ $0.isLetter || $0 == " " }) else { return nil }
                        return CustomVocabularyTerm(text: term, ctcTokenIds: ids)
                    }
                    if !vocabularyTerms.isEmpty {
                        let vocabulary = CustomVocabularyContext(terms: vocabularyTerms)
                        let spotter = CtcKeywordSpotter(models: models, blankId: models.vocabulary.count)
                        let rescorer = try await VocabularyRescorer.create(spotter: spotter,
                            vocabulary: vocabulary, config: .init(spotterRescueEnabled: false),
                            ctcModelDirectory: vocabularyModels.directory)
                        let samples = try decoded ?? AudioConverter().resampleAudioFile(url)
                        let spotted = try await spotter.spotKeywordsWithLogProbs(
                            audioSamples: samples, customVocabulary: vocabulary)
                        let evaluation = rescorer.ctcTokenEvaluateCandidates(transcript: result.text,
                            tokenTimings: timings, logProbs: spotted.logProbs,
                            frameDuration: spotted.frameDuration, cbw: 4.5,
                            marginSeconds: 0.1, minSimilarity: 0.55)
                        let rows: [[String: Any]] = evaluation.candidates.compactMap { candidate in
                            guard let range = candidate.baseTextUTF8Range,
                                  let original = candidate.rawOriginalCTCScore,
                                  let proposed = candidate.rawVocabularyCTCScore,
                                  original.isFinite, proposed.isFinite, candidate.similarity.isFinite
                            else { return nil }
                            return ["term": candidate.canonicalTerm, "start": range.lowerBound,
                                "end": range.upperBound, "similarity": candidate.similarity,
                                "vocabulary_score": proposed, "original_score": original]
                        }
                        candidates = String(data: try JSONSerialization.data(withJSONObject: rows), encoding: .utf8) ?? "[]"
                    }
                } catch {
                    // Never replace or truncate a successful transcript on auxiliary failure.
                    candidates = "[]"
                }
            }
            return (result.text, duration, result.processingTime, candidates)
        }
    }
}

// The C entry points must finish before returning to Rust. The semaphore is
// also the synchronization boundary for the operation's Result storage.
private func waitForAsync<T>(_ operation: @escaping () async throws -> T) throws -> T {
    let semaphore = DispatchSemaphore(value: 0)
    var result: Result<T, Error>?
    Task {
        do { result = .success(try await operation()) }
        catch { result = .failure(error) }
        semaphore.signal()
    }
    semaphore.wait()
    return try result!.get()
}

@_cdecl("sotto_asr_create")
public func createBridge() -> UnsafeMutableRawPointer {
    Unmanaged.passRetained(AsrBridge()).toOpaque()
}

@_cdecl("sotto_asr_destroy")
public func destroyBridge(_ handle: UnsafeMutableRawPointer) {
    Unmanaged<AsrBridge>.fromOpaque(handle).release()
}

private func directoryURL(_ path: UnsafePointer<CChar>) -> URL {
    URL(fileURLWithPath: String(cString: path), isDirectory: true)
}

@_cdecl("sotto_asr_download")
public func downloadSpeechModel(
    _ directory: UnsafePointer<CChar>,
    _ error: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>
) -> Int32 {
    do {
        let url = directoryURL(directory)
        try waitForAsync { try await AsrCache.download(to: url) }
        return 0
    } catch let failure {
        error.pointee = strdup(failure.localizedDescription)
        return -1
    }
}

@_cdecl("sotto_asr_initialize")
public func initializeBridge(
    _ handle: UnsafeMutableRawPointer,
    _ directory: UnsafePointer<CChar>,
    _ error: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>
) -> Int32 {
    do {
        try Unmanaged<AsrBridge>.fromOpaque(handle).takeUnretainedValue()
            .initialize(at: directoryURL(directory))
        return 0
    } catch let failure {
        error.pointee = strdup(failure.localizedDescription)
        return -1
    }
}

@_cdecl("sotto_asr_transcribe")
public func transcribeFile(
    _ handle: UnsafeMutableRawPointer,
    _ path: UnsafePointer<CChar>,
    _ terms: UnsafePointer<CChar>,
    _ text: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>,
    _ candidates: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>,
    _ duration: UnsafeMutablePointer<Double>,
    _ processingTime: UnsafeMutablePointer<Double>,
    _ error: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>
) -> Int32 {
    do {
        let result = try Unmanaged<AsrBridge>.fromOpaque(handle).takeUnretainedValue()
            .transcribe(String(cString: path), terms: JSONDecoder().decode([String].self, from: Data(String(cString: terms).utf8)))
        guard let copiedText = strdup(result.0) else {
            throw NSError(domain: NSPOSIXErrorDomain, code: Int(ENOMEM))
        }
        guard let copiedCandidates = strdup(result.3) else {
            free(copiedText)
            throw NSError(domain: NSPOSIXErrorDomain, code: Int(ENOMEM))
        }
        text.pointee = copiedText
        candidates.pointee = copiedCandidates
        duration.pointee = result.1
        processingTime.pointee = result.2
        return 0
    } catch let failure {
        error.pointee = strdup(failure.localizedDescription)
        return -1
    }
}

@_cdecl("sotto_vocabulary_download")
public func downloadVocabulary(
    _ directory: UnsafePointer<CChar>,
    _ fetchModels: Bool,
    _ error: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>
) -> Int32 {
    do {
        try VocabularyModels.download(to: directoryURL(directory), fetchModels: fetchModels)
        return 0
    } catch let failure {
        error.pointee = strdup(failure.localizedDescription)
        return -1
    }
}

@_cdecl("sotto_vocabulary_prepare")
public func prepareVocabulary(_ directory: UnsafePointer<CChar>,
    _ error: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>) -> UnsafeMutableRawPointer? {
    do {
        return Unmanaged.passRetained(try VocabularyModels.prepare(at: directoryURL(directory))).toOpaque()
    } catch let failure {
        error.pointee = strdup(failure.localizedDescription)
        return nil
    }
}

@_cdecl("sotto_vocabulary_destroy")
public func destroyVocabulary(_ handle: UnsafeMutableRawPointer) {
    Unmanaged<VocabularyModels>.fromOpaque(handle).release()
}

@_cdecl("sotto_asr_attach_vocabulary")
public func attachVocabulary(_ handle: UnsafeMutableRawPointer, _ vocabulary: UnsafeMutableRawPointer) {
    Unmanaged<AsrBridge>.fromOpaque(handle).takeUnretainedValue().vocabulary =
        Unmanaged<VocabularyModels>.fromOpaque(vocabulary).takeRetainedValue()
}

@_cdecl("sotto_asr_unload_vocabulary")
public func unloadVocabulary(_ handle: UnsafeMutableRawPointer) {
    Unmanaged<AsrBridge>.fromOpaque(handle).takeUnretainedValue().unloadVocabulary()
}

@_cdecl("sotto_asr_free_string")
public func freeString(_ value: UnsafeMutablePointer<CChar>?) {
    free(value)
}
