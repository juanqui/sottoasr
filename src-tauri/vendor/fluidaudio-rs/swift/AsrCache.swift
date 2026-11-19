import CoreML
import FluidAudio
import Foundation

/// Pinned TDT v3 cache without ModelHub's purge-and-redownload load recovery.
/// Downloading and loading are separate steps: the application verifies the
/// files between them, outside the resident engine. CoreML failures preserve
/// every existing artifact and surface to Settings.
enum AsrCache {
    /// Fetches missing artifacts at the pinned revision. Present files are
    /// never replaced, and nothing is loaded or deleted.
    static func download(to directory: URL) async throws {
        try requireSdkFolder(directory)
        // Supported downloader, same SDK cache and checkpoint. Unlike
        // AsrModels.download/load, this does not enter model-load recovery.
        try await ModelHub.download(.parakeetV3,
            to: directory.deletingLastPathComponent(),
            variant: ParakeetEncoderPrecision.int8.rawValue)
    }

    /// Loads verified local files. Never downloads.
    static func load(at directory: URL) async throws -> AsrModels {
        try requireSdkFolder(directory)
        guard AsrModels.modelsExist(at: directory, version: .v3, encoderPrecision: .int8) else {
            throw NSError(domain: "SottoASR", code: 4, userInfo: [NSLocalizedDescriptionKey:
                "The speech model files are missing. Existing model files were preserved."])
        }

        let vocabulary = try readVocabulary(at: directory.appendingPathComponent("parakeet_vocab.json"))
        let configuration = AsrModels.defaultConfiguration()
        configuration.allowLowPrecisionAccumulationOnGPU = true
        let preprocessorConfiguration = MLModelConfiguration()
        preprocessorConfiguration.computeUnits = .cpuOnly
        preprocessorConfiguration.allowLowPrecisionAccumulationOnGPU = true

        // Preserve the SDK's device policy: CPU preprocessor, CPU/ANE decoder,
        // encoder and joint. Direct CoreML loading has no network/cache recovery.
        let preprocessor = try MLModel(contentsOf: directory.appendingPathComponent("Preprocessor.mlmodelc"),
            configuration: preprocessorConfiguration)
        let encoder = try MLModel(contentsOf: directory.appendingPathComponent("Encoder.mlmodelc"),
            configuration: configuration)
        let decoder = try MLModel(contentsOf: directory.appendingPathComponent("Decoder.mlmodelc"),
            configuration: configuration)
        let joint = try MLModel(contentsOf: directory.appendingPathComponent("JointDecisionv3.mlmodelc"),
            configuration: configuration)
        return AsrModels(encoder: encoder, preprocessor: preprocessor,
            decoder: decoder, joint: joint, configuration: configuration,
            vocabulary: vocabulary, version: .v3)
    }

    private static func readVocabulary(at path: URL) throws -> [Int: String] {
        let json = try JSONSerialization.jsonObject(with: Data(contentsOf: path))
        var vocabulary: [Int: String] = [:]
        if let values = json as? [String: String] {
            for (key, value) in values {
                guard let index = Int(key), index >= 0 else {
                    throw invalidVocabulary()
                }
                vocabulary[index] = value
            }
        } else if let values = json as? [String] {
            vocabulary = Dictionary(uniqueKeysWithValues: values.enumerated().map { ($0.offset, $0.element) })
        }
        guard !vocabulary.isEmpty else { throw invalidVocabulary() }
        return vocabulary
    }

    private static func invalidVocabulary() -> NSError {
        NSError(domain: "SottoASR", code: 3, userInfo: [NSLocalizedDescriptionKey:
            "The ASR vocabulary cache is invalid. Existing model files were preserved."])
    }

    /// The SDK resolves the cache from the parent folder and the repository's
    /// folder name, so any other leaf would verify one folder and load another.
    private static func requireSdkFolder(_ directory: URL) throws {
        guard directory.lastPathComponent == Repo.parakeetV3.folderName else {
            throw NSError(domain: "SottoASR", code: 5, userInfo: [NSLocalizedDescriptionKey:
                "The speech model folder must be named \(Repo.parakeetV3.folderName)."])
        }
    }
}
