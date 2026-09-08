import CoreML
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
    return (Double(info.ru_utime.tv_sec + info.ru_stime.tv_sec)
        + Double(info.ru_utime.tv_usec + info.ru_stime.tv_usec) / 1_000_000, info.ru_maxrss)
}

@main struct Probe {
    static func main() async throws {
        let root = URL(fileURLWithPath: CommandLine.arguments[1])
        let mode = CommandLine.arguments[2]
        let manifest = try JSONSerialization.jsonObject(with: Data(contentsOf: root.appendingPathComponent("manifest.json"))) as! [[String: Any]]
        let directory = root.appendingPathComponent("Models/parakeet-tdt-0.6b-v3")
        ModelHub.offlineMode = true
        let config = MLModelConfiguration()
        var encoderOverride: MLComputeUnits? = nil
        switch mode {
        case "ane": config.computeUnits = .cpuAndNeuralEngine
        case "all": config.computeUnits = .all
        case "gpu_encoder":
            config.computeUnits = .cpuAndNeuralEngine
            encoderOverride = .cpuAndGPU
        case "cpu": config.computeUnits = .cpuOnly
        default: fatalError("Expected ane, all, gpu_encoder, or cpu")
        }
        let loadStart = Date()
        let manager = AsrManager()
        let models = try await AsrModels.load(from: directory, configuration: config,
            version: .v3, encoderPrecision: .int8, encoderComputeUnits: encoderOverride)
        try await manager.loadModels(models)
        emit(["event": "loaded", "mode": mode, "seconds": Date().timeIntervalSince(loadStart), "peak_rss_bytes": usage().1])
        let converter = AudioConverter()
        let warmup = try converter.resampleAudioFile(root.appendingPathComponent(manifest[0]["file"] as! String))
        var warmupState = try TdtDecoderState()
        _ = try await manager.transcribe(warmup, decoderState: &warmupState)
        for item in manifest {
            let samples = try converter.resampleAudioFile(root.appendingPathComponent(item["file"] as! String))
            var decoder = try TdtDecoderState()
            let cpuStart = usage().0
            let started = Date()
            let result = try await manager.transcribe(samples, decoderState: &decoder)
            emit(["event": "transcribed", "mode": mode, "id": item["id"]!, "text": result.text,
                "seconds": Date().timeIntervalSince(started), "cpu_seconds": usage().0 - cpuStart,
                "peak_rss_bytes": usage().1])
        }
    }
}
