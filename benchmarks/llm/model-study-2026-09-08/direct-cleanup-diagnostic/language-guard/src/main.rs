use std::io::{self, BufRead};
use std::time::Instant;

fn main() {
    for line in io::stdin().lock().lines() {
        let value: serde_json::Value = serde_json::from_str(&line.unwrap()).unwrap();
        let text = value["source"].as_str().unwrap();
        let started = Instant::now();
        let info = whatlang::detect(text);
        let elapsed_ns = started.elapsed().as_nanos();
        let detection = info.map(|info| serde_json::json!({
            "language": info.lang().code(),
            "confidence": info.confidence(),
            "reliable": info.is_reliable(),
            "skip": info.is_reliable() && info.lang() != whatlang::Lang::Eng,
        }));
        println!("{}", serde_json::json!({
            "id": value["id"], "source": text,
            "expected": value["expected"], "group": value["group"],
            "detection": detection, "elapsed_ns": elapsed_ns,
        }));
    }
}
