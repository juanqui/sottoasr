import AVFoundation
import Darwin
import FluidAudio
import Foundation

// Prepared independently so downloads and CoreML compilation never own the
// resident ASR engine. Rust transfers this retained object only when ready.
private final class VocabularyModels {
    let models: CtcModels
    let tokenizer: CtcTokenizer

    init(models: CtcModels, tokenizer: CtcTokenizer) {
        self.models = models
        self.tokenizer = tokenizer
    }

    static func prepare(allowDownload: Bool) throws -> VocabularyModels {
        try waitForAsync {
            let directory = CtcModels.defaultCacheDirectory()
            let tokenizer = try await VocabularyCache.tokenizer(at: directory, allowDownload: allowDownload)
            if allowDownload && !CtcModels.modelsExist(at: directory) {
                // Download without ModelHub.loadModels' destructive cache-purge
                // fallback. An invalid model surfaces an error and stays on disk.
                try await ModelHub.download(.parakeetCtc110m, to: directory.deletingLastPathComponent(),
                    config: DownloadConfig(timeout: 120, minStallBytes: 256 * 1024, stallWindow: 30))
            }
            let models = try await CtcModels.loadDirect(from: directory)
            return VocabularyModels(models: models, tokenizer: tokenizer)
        }
    }
}

// Rust exclusively owns this handle and waits for each async operation before
// making another call. Swift actors own all model and decoder work.
private final class AsrBridge {
    var manager: AsrManager?
    var vocabulary: VocabularyModels?

    func unloadVocabulary() { vocabulary = nil }

    func initialize() throws {
        try waitForAsync {
            let models = try await AsrCache.load()
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
            let result = try await manager.transcribe(url, decoderState: &state)
            var candidates = "[]"
            if !terms.isEmpty, let vocabularyModels = self.vocabulary,
               let timings = result.tokenTimings, !timings.isEmpty {
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
                            ctcModelDirectory: CtcModels.defaultCacheDirectory())
                        let samples = try AudioConverter().resampleAudioFile(url)
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

@_cdecl("sotto_asr_initialize")
public func initializeBridge(
    _ handle: UnsafeMutableRawPointer,
    _ error: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>
) -> Int32 {
    do {
        try Unmanaged<AsrBridge>.fromOpaque(handle).takeUnretainedValue().initialize()
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

@_cdecl("sotto_vocabulary_prepare")
public func prepareVocabulary(_ allowDownload: Bool,
    _ error: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>) -> UnsafeMutableRawPointer? {
    do {
        return Unmanaged.passRetained(try VocabularyModels.prepare(allowDownload: allowDownload)).toOpaque()
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
