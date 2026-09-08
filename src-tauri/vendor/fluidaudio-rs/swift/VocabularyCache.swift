import FluidAudio
import Foundation

/// Repairs only tokenizer metadata after explicit preparation. Model bundles
/// and an existing tokenizer survive failed downloads or invalid replacements.
enum VocabularyCache {
    static func tokenizer(
        at directory: URL,
        allowDownload: Bool,
        fetch: () async throws -> Data = fetchTokenizer
    ) async throws -> CtcTokenizer {
        do {
            return try await CtcTokenizer.load(from: directory)
        } catch {
            guard allowDownload else { throw error }
        }

        let data = try await fetch()
        let fileManager = FileManager.default
        try fileManager.createDirectory(at: directory, withIntermediateDirectories: true)
        let staging = directory.appendingPathComponent(".sotto-tokenizer-\(UUID().uuidString)", isDirectory: true)
        try fileManager.createDirectory(at: staging, withIntermediateDirectories: false)
        // This removes only our own staging directory, never cached models.
        defer { try? fileManager.removeItem(at: staging) }
        try data.write(to: staging.appendingPathComponent("tokenizer.json"), options: .atomic)
        let tokenizer = try await CtcTokenizer.load(from: staging)
        try data.write(to: directory.appendingPathComponent("tokenizer.json"), options: .atomic)
        return tokenizer
    }

    private static func fetchTokenizer() async throws -> Data {
        guard !ModelHub.offlineMode else { throw DownloadError.networkDisabled(operation: "vocabulary tokenizer repair") }
        let url = try ModelRegistry.resolveModel(CtcModelVariant.ctc110m.repo.remotePath, "tokenizer.json")
        let configuration = ModelHub.session.configuration
        configuration.timeoutIntervalForRequest = 30
        configuration.timeoutIntervalForResource = 120
        let session = URLSession(configuration: configuration)
        defer { session.finishTasksAndInvalidate() }
        let (data, response) = try await session.data(from: url)
        guard let response = response as? HTTPURLResponse, response.statusCode == 200 else {
            throw NSError(domain: "SottoASR", code: 2,
                userInfo: [NSLocalizedDescriptionKey: "Unable to download the vocabulary tokenizer. Please retry."])
        }
        return data
    }
}
