//! Non-interactive fixture runner. Uses the same pinned batch bridge as Sotto.

fn main() -> Result<(), String> {
    let paths: Vec<_> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        return Err("Provide one or more generated WAV paths".into());
    }
    let mut audio = fluidaudio_rs::FluidAudio::new()?;
    audio.init_asr()?;
    for path in paths {
        let result = audio.transcribe_file(&path)?;
        println!(
            "{}",
            serde_json::json!({
                "path": path,
                "text": result.text,
                "duration_secs": result.duration,
                "processing_secs": result.processing_time
            })
        );
    }
    Ok(())
}
