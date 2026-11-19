use serde::Serialize;

/// One selectable microphone. cpal identifies macOS input devices by name.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct InputDevice {
    pub name: String,
    pub is_default: bool,
}

fn enumerate_input_devices() -> Result<Vec<InputDevice>, String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let default_name = host.default_input_device().and_then(|device| device.name().ok());
    let devices = host
        .input_devices()
        .map_err(|error| format!("Could not list microphones: {error}"))?;
    let mut listed: Vec<InputDevice> = Vec::new();
    for device in devices {
        // A device that cannot report its name cannot be selected by name.
        let Ok(name) = device.name() else { continue };
        if listed.iter().any(|known| known.name == name) {
            continue;
        }
        let is_default = default_name.as_deref() == Some(name.as_str());
        listed.push(InputDevice { name, is_default });
    }
    Ok(listed)
}

/// List microphones for Settings. Enumeration talks to CoreAudio, so it runs
/// off the async executor.
#[tauri::command]
pub async fn list_input_devices() -> Result<Vec<InputDevice>, String> {
    tokio::task::spawn_blocking(enumerate_input_devices)
        .await
        .map_err(|error| format!("Microphone listing task failed: {error}"))?
}
