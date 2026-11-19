import FluidAudio
import Foundation

@main struct AsrCacheProbe {
    static func require(_ value: @autoclosure () throws -> Bool, _ message: String) throws {
        guard try value() else {
            throw NSError(domain: "AsrCacheProbe", code: 1,
                userInfo: [NSLocalizedDescriptionKey: message])
        }
    }

    static func expectFailure(_ directory: URL) async throws {
        do { _ = try await AsrCache.load(at: directory) }
        catch { return }
        throw NSError(domain: "AsrCacheProbe", code: 2,
            userInfo: [NSLocalizedDescriptionKey: "Expected cache load to fail"])
    }

    static func expectThrows(_ message: String, _ operation: () async throws -> Void) async throws {
        do { try await operation() }
        catch { return }
        throw NSError(domain: "AsrCacheProbe", code: 3,
            userInfo: [NSLocalizedDescriptionKey: message])
    }

    static func main() async throws {
        // This is an isolated executable, so the global offline flag cannot
        // race the application's independent vocabulary download.
        ModelHub.offlineMode = true
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("sotto-asr-cache-test-\(UUID().uuidString)")
        let directory = root.appendingPathComponent("parakeet-tdt-0.6b-v3")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let sentinelPath = directory.appendingPathComponent("existing-model-sentinel")
        let sentinel = Data("existing-cache-must-survive".utf8)
        try sentinel.write(to: sentinelPath)

        // Loading never downloads: missing artifacts fail and nothing is deleted.
        try await expectFailure(directory)
        try require(Data(contentsOf: sentinelPath) == sentinel, "Incomplete cache was deleted")
        // Downloading is a separate step. Offline it fails and keeps the cache.
        try await expectThrows("Offline download succeeded") { try await AsrCache.download(to: directory) }
        try require(Data(contentsOf: sentinelPath) == sentinel, "Failed download deleted cache")
        try await expectThrows("Loaded a folder the SDK does not download to") {
            _ = try await AsrCache.load(at: root.appendingPathComponent("other-folder"))
        }
        // Downloads read the pinned commits, never the mutable main branch.
        for repo in [Repo.parakeetV3, Repo.parakeetCtc110m] {
            let url = try ModelRegistry.resolveModel(repo.remotePath, "config.json").absoluteString
            try require(!url.contains("/resolve/main/"), "\(repo.remotePath) downloads from main")
        }
        try require(ModelRegistry.resolveModel(Repo.vad.remotePath, "config.json").absoluteString
            .contains("/resolve/main/"), "Unpinned repositories no longer resolve main")

        let names = ["Preprocessor.mlmodelc", "Encoder.mlmodelc", "Decoder.mlmodelc", "JointDecisionv3.mlmodelc"]
        for name in names {
            try FileManager.default.createDirectory(at: directory.appendingPathComponent(name), withIntermediateDirectories: false)
        }
        let vocabularyPath = directory.appendingPathComponent("parakeet_vocab.json")
        let validVocabulary = Data(#"{"0":"synthetic"}"#.utf8)
        try validVocabulary.write(to: vocabularyPath)
        // All names exist, but CoreML must reject the empty compiled bundle.
        // This previously entered ModelHub's whole-repository purge fallback.
        // Leave online permission enabled here, as in production: preservation
        // must come from direct loading, not the SDK's global offline escape.
        ModelHub.offlineMode = false
        try await expectFailure(directory)
        try require(Data(contentsOf: sentinelPath) == sentinel, "CoreML failure deleted cache")
        for name in names {
            try require(FileManager.default.fileExists(atPath: directory.appendingPathComponent(name).path), "CoreML failure deleted a model")
        }
        try require(Data(contentsOf: vocabularyPath) == validVocabulary, "CoreML failure modified vocabulary")

        let invalidVocabulary = Data(#"{"bad-id":"synthetic"}"#.utf8)
        try invalidVocabulary.write(to: vocabularyPath)
        try await expectFailure(directory)
        try require(Data(contentsOf: vocabularyPath) == invalidVocabulary, "Malformed vocabulary was overwritten")
        try require(Data(contentsOf: sentinelPath) == sentinel, "Malformed vocabulary deleted cache")
        print("ASR cache: 5 cache failure cases and 3 revision checks passed; existing artifacts preserved")

        if CommandLine.arguments.count == 3 {
            ModelHub.offlineMode = true
            let models = try await AsrCache.load(at: URL(fileURLWithPath: CommandLine.arguments[1]))
            let manager = AsrManager()
            try await manager.loadModels(models)
            var decoder = try TdtDecoderState()
            let result = try await manager.transcribe(URL(fileURLWithPath: CommandLine.arguments[2]), decoderState: &decoder)
            print("ASR cache synthetic smoke: \(result.text)")
        }
    }
}
