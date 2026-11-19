//! Non-interactive fixture runner. Uses the same pinned batch bridge as Sotto.
//! It never downloads: open SottoASR once so it downloads and verifies Parakeet
//! Ultra. An installation that still uses the previous model has not finished
//! that download.

fn main() -> Result<(), String> {
    let paths: Vec<_> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        return Err("Provide one or more generated WAV paths".into());
    }
    let models = dirs::data_dir()
        .ok_or("Could not find the Application Support folder")?
        .join("FluidAudio/Models/parakeet-ultra");
    let mut audio = fluidaudio_rs::FluidAudio::new()?;
    audio.init_asr(&models)
        .map_err(|error| format!("{error} Open SottoASR once to download the speech model, then try again."))?;
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
