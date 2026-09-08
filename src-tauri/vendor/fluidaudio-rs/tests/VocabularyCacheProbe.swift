import FluidAudio
import Foundation

@main struct VocabularyCacheProbe {
    static func require(_ value: @autoclosure () throws -> Bool, _ message: String) throws {
        guard try value() else { throw NSError(domain: "VocabularyCacheProbe", code: 1,
            userInfo: [NSLocalizedDescriptionKey: message]) }
    }

    static func expectFailure(_ operation: () async throws -> Void) async throws {
        do { try await operation() } catch { return }
        throw NSError(domain: "VocabularyCacheProbe", code: 10,
            userInfo: [NSLocalizedDescriptionKey: "Expected operation to fail"])
    }

    static func main() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("sotto-cache-test-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let encoder = directory.appendingPathComponent("AudioEncoder.mlmodelc")
        try FileManager.default.createDirectory(at: encoder, withIntermediateDirectories: false)
        let weights = encoder.appendingPathComponent("weight.bin")
        let sentinel = Data("preserve-model-weights".utf8)
        try sentinel.write(to: weights)
        let tokenFile = directory.appendingPathComponent("tokenizer.json")
        let valid = Data(#"{"model":{"type":"BPE","vocab":{"▁":1,"q":2,"w":3,"e":4,"n":5},"merges":[]}}"#.utf8)
        let invalid = Data(#"{"model":{"type":"BPE","vocab":{"q":"invalid-id"},"merges":[]}}"#.utf8)
        var fetches = 0
        let fetch = { () async throws -> Data in fetches += 1; return valid }

        try await expectFailure {
            _ = try await VocabularyCache.tokenizer(at: directory, allowDownload: false, fetch: fetch)
        }
        try require(fetches == 0, "cached-only load attempted a download")
        _ = try await VocabularyCache.tokenizer(at: directory, allowDownload: true, fetch: fetch)
        try require(fetches == 1, "missing tokenizer was not repaired")
        try require(Data(contentsOf: tokenFile) == valid, "repair bytes differ")
        _ = try await VocabularyCache.tokenizer(at: directory, allowDownload: true, fetch: fetch)
        try require(fetches == 1, "valid cache fetched again")

        try invalid.write(to: tokenFile)
        _ = try await VocabularyCache.tokenizer(at: directory, allowDownload: true, fetch: fetch)
        try require(fetches == 2, "invalid BPE tokenizer was not repaired")
        try invalid.write(to: tokenFile)
        try await expectFailure {
            _ = try await VocabularyCache.tokenizer(at: directory, allowDownload: true, fetch: { invalid })
        }
        try require(Data(contentsOf: tokenFile) == invalid, "invalid replacement destroyed existing cache")
        try await expectFailure {
            _ = try await VocabularyCache.tokenizer(at: directory, allowDownload: true, fetch: { throw URLError(.timedOut) })
        }
        try require(Data(contentsOf: tokenFile) == invalid, "failed fetch destroyed existing cache")
        try FileManager.default.removeItem(at: tokenFile)
        try FileManager.default.createDirectory(at: tokenFile, withIntermediateDirectories: false)
        try await expectFailure {
            _ = try await VocabularyCache.tokenizer(at: directory, allowDownload: true, fetch: fetch)
        }
        var isDirectory: ObjCBool = false
        try require(FileManager.default.fileExists(atPath: tokenFile.path, isDirectory: &isDirectory) && isDirectory.boolValue,
            "failed atomic write replaced a directory")
        try require(Data(contentsOf: weights) == sentinel, "model weights changed")
        try require(!FileManager.default.contentsOfDirectory(atPath: directory.path).contains(where: { $0.hasPrefix(".sotto-tokenizer-") }),
            "staging directory leaked")
        print("Vocabulary cache: 7 offline repair/failure checks passed; model weights preserved")
    }
}
