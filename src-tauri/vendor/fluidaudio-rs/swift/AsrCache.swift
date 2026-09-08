import CoreML
import FluidAudio
import Foundation

/// Pinned TDT v3 loader without ModelHub's purge-and-redownload load recovery.
/// CoreML failures preserve every existing artifact and surface to Settings.
enum AsrCache {
    static func load(
        at directory: URL = AsrModels.defaultCacheDirectory(for: .v3)
    ) async throws -> AsrModels {
        if !AsrModels.modelsExist(at: directory, version: .v3, encoderPrecision: .int8) {
            // Supported downloader, same SDK cache and checkpoint. Unlike
            // AsrModels.download/load, this does not enter model-load recovery.
            try await ModelHub.download(.parakeetV3,
                to: directory.deletingLastPathComponent(),
                variant: ParakeetEncoderPrecision.int8.rawValue)
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
}
