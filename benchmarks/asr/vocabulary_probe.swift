import AVFoundation
import Darwin
import FluidAudio
import Foundation

func emit(_ value: [String: Any]) {
    let data = try! JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    print(String(data: data, encoding: .utf8)!)
    fflush(stdout)
}
func usage() -> (Double, Int) {
    var info = rusage()
    getrusage(RUSAGE_SELF, &info)
    let seconds = Double(info.ru_utime.tv_sec + info.ru_stime.tv_sec) + Double(info.ru_utime.tv_usec + info.ru_stime.tv_usec) / 1_000_000
    return (seconds, info.ru_maxrss)
}
func nullable<T>(_ value: T?) -> Any { value as Any? ?? NSNull() }
@main struct Probe {
    static func main() async throws {
        let root = URL(fileURLWithPath: CommandLine.arguments[1])
        let manifest = try JSONSerialization.jsonObject(with: Data(contentsOf: root.appendingPathComponent("manifest.json"))) as! [[String: Any]]
        let asrDir = root.appendingPathComponent("Models/parakeet-tdt-0.6b-v3")
        let loadStart = Date()
        let manager = AsrManager()
        let models = try await AsrModels.load(from: asrDir, version: .v3, encoderPrecision: .int8)
        try await manager.loadModels(models)
        emit(["event": "asr_loaded", "seconds": Date().timeIntervalSince(loadStart), "peak_rss_bytes": usage().1])
        let converter = AudioConverter()
        var baseline: [(ASRResult, [Float])] = []
        for item in manifest {
            let samples = try converter.resampleAudioFile(root.appendingPathComponent(item["file"] as! String))
            var decoder = try TdtDecoderState()
            let cpuStart = usage().0
            let started = Date()
            let result = try await manager.transcribe(samples, decoderState: &decoder)
            emit(["event": "baseline", "id": item["id"]!, "text": result.text, "seconds": Date().timeIntervalSince(started), "cpu_seconds": usage().0-cpuStart, "duration": Double(samples.count)/16000, "peak_rss_bytes": usage().1, "timings": result.tokenTimings?.count ?? 0])
            baseline.append((result, samples))
        }
        let ctcDir = root.appendingPathComponent("Models/parakeet-ctc-110m-coreml")
        let ctcStart = Date()
        let ctc = try await CtcModels.downloadAndLoad(to: ctcDir, variant: .ctc110m)
        let tokenizer = try await CtcTokenizer.load(from: ctcDir)
        let namesFile = root.appendingPathComponent("vocabulary.json")
        let names = FileManager.default.fileExists(atPath: namesFile.path)
            ? try JSONDecoder().decode([String].self, from: Data(contentsOf: namesFile))
            : ["Qwen", "Qwen3.8-Flash-Next", "NVIDIA", "Parakeet", "SottoASR", "FluidAudio", "CoreML", "MLX", "Kubernetes", "PostgreSQL", "Tauri", "Claude", "CRAN", "Snyk"]
        let terms = names.map { CustomVocabularyTerm(text: $0, ctcTokenIds: tokenizer.encode($0)) }
        let vocabulary = CustomVocabularyContext(terms: terms)
        emit(["event": "ctc_loaded", "seconds": Date().timeIntervalSince(ctcStart), "peak_rss_bytes": usage().1, "terms": terms.map { ["text": $0.text, "token_ids": $0.ctcTokenIds!] }])
        let spotter = CtcKeywordSpotter(models: ctc, blankId: ctc.vocabulary.count)
        let policies: [(String, VocabularyRescorer.Config, Double, Float, Float)] = [
            ("sdk_session_default", .default, 0.5, 0.55, 4.5),
            ("sdk_default_margin", .default, 0.1, 0.55, 4.5),
            ("guarded", .init(shortTermCbwTaperPivot: 5, shortTermCbwTaperExponent: 2, spotterRescueMinSimilarity: 0.3, spotterRescueMultiWordMinSimilarity: 0.5), 0.1, 0.55, 4.5),
            ("no_rescue", .init(spotterRescueEnabled: false), 0.1, 0.55, 4.5)
        ]
        var rescorers: [VocabularyRescorer] = []
        for policy in policies {
            rescorers.append(try await VocabularyRescorer.create(spotter: spotter, vocabulary: vocabulary, config: policy.1, ctcModelDirectory: ctcDir))
        }
        for (index, item) in manifest.enumerated() {
            let (result, samples) = baseline[index]
            let cpuStart = usage().0
            let started = Date()
            let spotted = try await spotter.spotKeywordsWithLogProbs(audioSamples: samples, customVocabulary: vocabulary)
            emit(["event": "ctc", "id": item["id"]!, "seconds": Date().timeIntervalSince(started), "cpu_seconds": usage().0-cpuStart, "peak_rss_bytes": usage().1, "frames": spotted.totalFrames])
            let evidence = rescorers[3].ctcTokenEvaluateCandidates(transcript: result.text, tokenTimings: result.tokenTimings ?? [], logProbs: spotted.logProbs, frameDuration: spotted.frameDuration, cbw: 4.5, marginSeconds: 0.1, minSimilarity: 0.55)
            let evidenceRows: [[String: Any]] = evidence.candidates.map { candidate in
                ["base": candidate.basePhrase, "term": candidate.canonicalTerm, "similarity": candidate.similarity,
                 "vocab_score": nullable(candidate.rawVocabularyCTCScore), "original_score": nullable(candidate.rawOriginalCTCScore),
                 "boost": nullable(candidate.effectiveBoost), "origin": String(describing: candidate.origin),
                 "comparison_passed": candidate.comparisonPassed, "outcome": String(describing: candidate.legacyOutcome),
                 "range": nullable(candidate.baseTextUTF8Range.map { [$0.lowerBound, $0.upperBound] }),
                 "reason": candidate.reason]
            }
            emit(["event": "evidence", "id": item["id"]!, "text": evidence.baseText, "candidates": evidenceRows])
            for (policyIndex, policy) in policies.enumerated() {
                let rescoreStart = Date()
                let rescored = rescorers[policyIndex].ctcTokenRescore(transcript: result.text, tokenTimings: result.tokenTimings ?? [], logProbs: spotted.logProbs, frameDuration: spotted.frameDuration, cbw: policy.4, marginSeconds: policy.2, minSimilarity: policy.3)
                emit(["event": "rescored", "id": item["id"]!, "policy": policy.0, "seconds": Date().timeIntervalSince(rescoreStart), "text": rescored.text, "modified": rescored.wasModified, "applied": rescored.replacements.filter { $0.shouldReplace }.map { ["original": $0.originalWord, "replacement": $0.replacementWord ?? ""] }])
            }
        }
    }
}
