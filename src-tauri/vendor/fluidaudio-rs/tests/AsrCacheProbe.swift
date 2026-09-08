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

        // Missing artifacts cannot turn an offline setup failure into deletion.
        try await expectFailure(directory)
        try require(Data(contentsOf: sentinelPath) == sentinel, "Incomplete cache was deleted")

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
        print("ASR cache: 3 cache failure cases passed; existing artifacts preserved")

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
