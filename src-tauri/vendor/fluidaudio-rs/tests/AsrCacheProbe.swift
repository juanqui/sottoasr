import FluidAudio
import Foundation

@main struct AsrCacheProbe {
    static func require(_ value: @autoclosure () throws -> Bool, _ message: String) throws {
        guard try value() else {
            throw NSError(domain: "AsrCacheProbe", code: 1,
                userInfo: [NSLocalizedDescriptionKey: message])
        }
    }

    /// The load must fail, and with the expected reason when one is given.
    static func expectFailure(_ directory: URL, containing text: String? = nil) throws {
        do { _ = try AsrCache.load(at: directory) }
        catch {
            if let text {
                try require(error.localizedDescription.contains(text),
                    "Expected a failure containing '\(text)', got: \(error.localizedDescription)")
            }
            return
        }
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
        let directory = root.appendingPathComponent(AsrCache.current.folderName)
        let previous = root.appendingPathComponent(AsrCache.previous.folderName)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let sentinelPath = directory.appendingPathComponent("existing-model-sentinel")
        let sentinel = Data("existing-cache-must-survive".utf8)
        try sentinel.write(to: sentinelPath)

        // Loading never downloads: missing artifacts fail and nothing is deleted.
        try expectFailure(directory, containing: "missing")
        try require(Data(contentsOf: sentinelPath) == sentinel, "Incomplete cache was deleted")
        // Downloading is a separate step. Offline it fails and keeps the cache.
        try await expectThrows("Offline download succeeded") { try await AsrCache.download(to: directory) }
        try require(Data(contentsOf: sentinelPath) == sentinel, "Failed download deleted cache")
        // The folder names the model. The previous model loads from its own
        // folder but is never downloaded; any other folder is refused.
        try expectFailure(previous, containing: "missing")
        try expectFailure(root.appendingPathComponent("other-folder"), containing: "must be named")
        try await expectThrows("Downloaded into the previous model's folder") {
            try await AsrCache.download(to: previous)
        }
        try require(!FileManager.default.fileExists(atPath: previous.path), "A refused download created a folder")

        // Downloads read the pinned commits, never the mutable main branch.
        for repo in [AsrCache.current, Repo.parakeetCtc110m] {
            let url = try ModelRegistry.resolveModel(repo.remotePath, "config.json").absoluteString
            try require(!url.contains("/resolve/main/"), "\(repo.remotePath) downloads from main")
            // The pin is in the URL only. If the SDK pinned these repositories
            // itself, its cache would delete every file its listing lacks:
            // the integrity record and the copies a re-download keeps.
            try require(repo.revision == "main", "The SDK now pins \(repo.remotePath); review the cache before adopting it")
        }
        try require(ModelRegistry.resolveModel(Repo.vad.remotePath, "config.json").absoluteString
            .contains("/resolve/main/"), "Unpinned repositories no longer resolve main")

        let names = ["Preprocessor.mlmodelc", "Encoder.mlmodelc", "Decoder.mlmodelc", "JointDecisionv3.mlmodelc"]
        for name in names {
            try FileManager.default.createDirectory(at: directory.appendingPathComponent(name), withIntermediateDirectories: false)
        }
        let vocabularyPath = directory.appendingPathComponent("parakeet_vocab.json")
        // Every token before the blank, so the load reaches CoreML.
        let tokens = Dictionary(uniqueKeysWithValues: (0..<AsrModelVersion.ultra.blankId).map { (String($0), "t\($0)") })
        let validVocabulary = try JSONSerialization.data(withJSONObject: tokens)
        try validVocabulary.write(to: vocabularyPath)
        // All names exist, but CoreML must reject the empty compiled bundle.
        // The SDK's downloading loaders answer that with a whole-repository
        // purge. Leave online permission enabled here, as in production:
        // preservation must come from local loading, not the SDK's global
        // offline escape.
        ModelHub.offlineMode = false
        try expectFailure(directory)
        try require(Data(contentsOf: sentinelPath) == sentinel, "CoreML failure deleted cache")
        for name in names {
            try require(FileManager.default.fileExists(atPath: directory.appendingPathComponent(name).path), "CoreML failure deleted a model")
        }
        try require(Data(contentsOf: vocabularyPath) == validVocabulary, "CoreML failure modified vocabulary")

        let invalidVocabulary = Data(#"{"bad-id":"synthetic"}"#.utf8)
        try invalidVocabulary.write(to: vocabularyPath)
        try expectFailure(directory)
        try require(Data(contentsOf: vocabularyPath) == invalidVocabulary, "Malformed vocabulary was overwritten")
        try require(Data(contentsOf: sentinelPath) == sentinel, "Malformed vocabulary deleted cache")
        print("ASR cache: 7 cache failure cases and 5 revision checks passed; existing artifacts preserved")

        if CommandLine.arguments.count == 3 {
            ModelHub.offlineMode = true
            let models = try AsrCache.load(at: URL(fileURLWithPath: CommandLine.arguments[1]))
            let manager = AsrManager()
            try await manager.loadModels(models)
            var decoder = try TdtDecoderState()
            let result = try await manager.transcribe(URL(fileURLWithPath: CommandLine.arguments[2]), decoderState: &decoder)
            print("ASR cache synthetic smoke: \(result.text)")
        }
    }
}
