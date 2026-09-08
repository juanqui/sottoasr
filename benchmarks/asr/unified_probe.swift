import Darwin
import FluidAudio
import Foundation

func emit(_ value: [String: Any]) {
    let data=try! JSONSerialization.data(withJSONObject:value,options:[.sortedKeys])
    print(String(data:data,encoding:.utf8)!)
    fflush(stdout)
}
func cpu() -> Double {
    var info=rusage(); getrusage(RUSAGE_SELF,&info)
    return Double(info.ru_utime.tv_sec+info.ru_stime.tv_sec)+Double(info.ru_utime.tv_usec+info.ru_stime.tv_usec)/1_000_000
}
@main struct Probe {
    static func main() async throws {
        let root=URL(fileURLWithPath:CommandLine.arguments[1])
        let manifest=try JSONSerialization.jsonObject(with:Data(contentsOf:root.appendingPathComponent("manifest.json"))) as! [[String:Any]]
        let manager=UnifiedAsrManager(encoderPrecision:.int8)
        let start=Date()
        try await manager.loadModels(to:root.appendingPathComponent("Models"))
        emit(["event":"loaded","seconds":Date().timeIntervalSince(start)])
        let converter=AudioConverter()
        for item in manifest {
            let samples=try converter.resampleAudioFile(root.appendingPathComponent(item["file"] as! String))
            let start=Date(); let cpuStart=cpu()
            let result=try await manager.transcribe(samples)
            var info=rusage();getrusage(RUSAGE_SELF,&info)
            emit(["event":"transcribed","id":item["id"]!,"text":result,"seconds":Date().timeIntervalSince(start),"cpu_seconds":cpu()-cpuStart,"peak_rss_bytes":info.ru_maxrss])
        }
    }
}
