import CoreML
import FluidAudio
import Foundation

/// Pinned speech model cache without ModelHub's purge-and-redownload load
/// recovery. Downloading and loading are separate steps: the application
/// verifies the files between them, outside the resident engine. CoreML
/// failures preserve every existing artifact and surface to Settings.
enum AsrCache {
    /// The model the application downloads.
    static let current = Repo.parakeetUltra
    /// The model earlier releases downloaded. It is never downloaded again: an
    /// installation that has it keeps dictating with it until `current` is
    /// downloaded and verified.
    static let previous = Repo.parakeetV3

    /// Fetches missing artifacts of `current` at the pinned revision. Present
    /// files are never replaced, and nothing is loaded or deleted.
    static func download(to directory: URL) async throws {
        // ModelHub writes into the parent folder under the repository's folder
        // name, so any other leaf would verify one folder and download to another.
        guard directory.lastPathComponent == current.folderName else {
            throw NSError(domain: "SottoASR", code: 5, userInfo: [NSLocalizedDescriptionKey:
                "The speech model folder must be named \(current.folderName)."])
        }
        // Supported downloader and SDK cache folder. Unlike AsrModels.download
        // and load, this does not enter model-load recovery.
        try await ModelHub.download(current, to: directory.deletingLastPathComponent())
    }

    /// Loads verified local files. Never downloads. The folder name selects
    /// the model version.
    static func load(at directory: URL) throws -> AsrModels {
        let version = try version(of: directory)
        guard AsrModels.modelsExist(at: directory, version: version, encoderPrecision: .int8) else {
            throw NSError(domain: "SottoASR", code: 4, userInfo: [NSLocalizedDescriptionKey:
                "The speech model files are missing. Existing model files were preserved."])
        }
        // The SDK's local loader: CPU preprocessor; CPU/ANE encoder, decoder and
        // joint. It reads the bundles directly through CoreML and has no
        // network or cache recovery.
        return try AsrModels.loadLocal(from: directory, version: version)
    }

    private static func version(of directory: URL) throws -> AsrModelVersion {
        switch directory.lastPathComponent {
        case current.folderName: return .ultra
        case previous.folderName: return .v3
        default:
            throw NSError(domain: "SottoASR", code: 5, userInfo: [NSLocalizedDescriptionKey:
                "The speech model folder must be named \(current.folderName) or \(previous.folderName)."])
        }
    }
}
