import CryptoKit
import Darwin
import Foundation

/// Streams a model file through SHA-256 so verification never holds a whole
/// bundle in memory and never maps a file that could shrink underneath it.
enum FileDigest {
    static func sha256(at url: URL) throws -> String {
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        var hasher = SHA256()
        var finished = false
        while !finished {
            // Each chunk is released before the next read on this worker thread.
            try autoreleasepool {
                guard let chunk = try handle.read(upToCount: 1 << 20), !chunk.isEmpty else {
                    finished = true
                    return
                }
                hasher.update(data: chunk)
            }
        }
        return hasher.finalize().map { String(format: "%02x", $0) }.joined()
    }
}

@_cdecl("sotto_sha256_file")
public func sha256File(
    _ path: UnsafePointer<CChar>,
    _ digest: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>,
    _ error: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>
) -> Int32 {
    do {
        let value = try FileDigest.sha256(at: URL(fileURLWithPath: String(cString: path)))
        guard let copied = strdup(value) else {
            throw NSError(domain: NSPOSIXErrorDomain, code: Int(ENOMEM))
        }
        digest.pointee = copied
        return 0
    } catch let failure {
        error.pointee = strdup(failure.localizedDescription)
        return -1
    }
}
