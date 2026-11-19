// Speech model comparison probe (FluidAudio 0.18.3). Usage: probe <experiment root> <fixture dir> <model>
// <model>: v3, ultra, nemotron-unified-fp16, nemotron-unified-int8, nemotron-unified-auto-int8,
// nemotron35-ml-2240, nemotron35-ml-1120, nemotron-en-2240, nemotron-en-1120.
// Models are in <experiment root>/Models: v3 must already be there (no download); the others download there.
// Reads <fixture dir>/manifest.json, transcribes the first clip once to warm up, then emits one JSON line per clip.
// Emits the same JSON lines as benchmarks/asr/*_probe.swift so analyze.py and score_models.py score them.
import AVFoundation
import CoreML
import Darwin
import FluidAudio
import Foundation

func emit(_ value: [String: Any]) {
    let data = try! JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    print(String(data: data, encoding: .utf8)!)
    fflush(stdout)
}

func usage() -> (cpu: Double, rss: Int) {
    var info = rusage()
    getrusage(RUSAGE_SELF, &info)
    return (
        Double(info.ru_utime.tv_sec + info.ru_stime.tv_sec)
            + Double(info.ru_utime.tv_usec + info.ru_stime.tv_usec) / 1_000_000, info.ru_maxrss
    )
}

func pcmBuffer(_ url: URL) throws -> AVAudioPCMBuffer {
    let file = try AVAudioFile(forReading: url)
    let buffer = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: AVAudioFrameCount(file.length))!
    try file.read(into: buffer)
    return buffer
}

let root = URL(fileURLWithPath: CommandLine.arguments[1])
let fixtures = URL(fileURLWithPath: CommandLine.arguments[2])
let model = CommandLine.arguments[3]
let models = root.appendingPathComponent("Models")
let manifest =
    try JSONSerialization.jsonObject(with: Data(contentsOf: fixtures.appendingPathComponent("manifest.json")))
    as! [[String: Any]]
let converter = AudioConverter()

let transcribe: (URL) async throws -> String
let loadStart = Date()

switch model {
case "v3", "ultra":
    let version: AsrModelVersion = model == "v3" ? .v3 : .ultra
    let directory = models.appendingPathComponent(AsrModels.defaultCacheDirectory(for: version).lastPathComponent)
    let loaded: AsrModels
    if model == "v3" {
        // APFS clone of the app's installed int8 cache; no network, no recovery path.
        loaded = try AsrModels.loadLocal(from: directory, version: .v3)
    } else {
        loaded = try await AsrModels.downloadAndLoad(to: directory, version: .ultra)
    }
    let manager = AsrManager()
    try await manager.loadModels(loaded)
    transcribe = { url in
        let samples = try converter.resampleAudioFile(url)
        var state = try TdtDecoderState()
        return try await manager.transcribe(samples, decoderState: &state).text
    }
case "nemotron-unified-fp16", "nemotron-unified-int8", "nemotron-unified-auto-int8":
    let manager = UnifiedAsrManager(
        config: UnifiedConfig(family: .nemotronMultilingual(model.contains("auto") ? .auto : .en)),
        encoderPrecision: model.hasSuffix("fp16") ? .fp16 : .int8)
    try await manager.loadModels(to: models)
    transcribe = { url in try await manager.transcribe(try converter.resampleAudioFile(url)) }
case "nemotron35-ml-2240", "nemotron35-ml-1120":
    let chunk = model.hasSuffix("2240") ? 2240 : 1120
    let directory = try await StreamingNemotronMultilingualAsrManager.downloadVariant(
        languageCode: "en-US", chunkMs: chunk, to: models)
    let manager = StreamingNemotronMultilingualAsrManager()
    try await manager.loadModels(from: directory)
    await manager.setLanguage("en-US")
    transcribe = { url in
        _ = try await manager.process(samples: try converter.resampleAudioFile(url))
        let text = try await manager.finish()
        await manager.reset()
        return text
    }
case "nemotron-en-2240", "nemotron-en-1120":
    let manager = StreamingNemotronAsrManager(
        requestedChunkSize: model.hasSuffix("2240") ? .ms2240 : .ms1120)
    try await manager.loadModels(to: models)
    transcribe = { url in
        _ = try await manager.process(audioBuffer: try pcmBuffer(url))
        let text = try await manager.finish()
        await manager.reset()
        return text
    }
default:
    fatalError("unknown model \(model)")
}

emit(["event": "loaded", "model": model, "seconds": Date().timeIntervalSince(loadStart), "peak_rss_bytes": usage().rss])
_ = try await transcribe(fixtures.appendingPathComponent(manifest[0]["file"] as! String))
for item in manifest {
    let url = fixtures.appendingPathComponent(item["file"] as! String)
    let cpuStart = usage().cpu
    let started = Date()
    let text = try await transcribe(url)
    emit([
        "event": "transcribed", "model": model, "id": item["id"]!, "text": text,
        "seconds": Date().timeIntervalSince(started), "cpu_seconds": usage().cpu - cpuStart,
        "peak_rss_bytes": usage().rss,
    ])
}
