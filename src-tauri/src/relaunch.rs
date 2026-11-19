//! Restart on macOS: start the app bundle again through LaunchServices once
//! this process has exited.
//!
//! Tauri's own restart starts the executable inside the bundle as a child of
//! the process that is exiting. Right after an update replaced the bundle,
//! macOS can refuse that ("Security policy would not allow process" from
//! AppleSystemPolicy), so the app closed and did not come back. A child that
//! does start has the old process as its responsible process, and then
//! misses its Accessibility and Input Monitoring grants. `open` starts the
//! bundle the way Finder does.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// Waits for process `$1` to end, then starts bundle `$2` with `$3`. It gives
/// up after 60 seconds: a second instance beside a process that did not exit
/// would fight over the shortcuts and the recordings folder.
const WAIT_THEN_OPEN: &str = r#"tries=0
while kill -0 "$1" 2>/dev/null; do
  tries=$((tries + 1))
  [ "$tries" -gt 600 ] && exit 1
  /bin/sleep 0.1
done
exec "$3" -n "$2""#;

/// The `.app` bundle that holds `executable`, or `None` for a bare binary
/// such as a development build.
pub(crate) fn app_bundle(executable: &Path) -> Option<PathBuf> {
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    (macos.file_name()? == "MacOS" && contents.file_name()? == "Contents" && bundle.extension()? == "app")
        .then(|| bundle.to_path_buf())
}

/// Starts `bundle` again once this process has exited. The caller exits next.
pub(crate) fn after_exit(bundle: &Path) -> std::io::Result<()> {
    open_after(std::process::id(), bundle, Path::new("/usr/bin/open")).map(drop)
}

/// The helper is not this app's child process in the registry sense: it must
/// outlive the app. Its own process group keeps it out of the cleanup of the
/// app's group when the app exits.
fn open_after(pid: u32, bundle: &Path, opener: &Path) -> std::io::Result<Child> {
    use std::os::unix::process::CommandExt;
    Command::new("/bin/sh")
        .arg("-c").arg(WAIT_THEN_OPEN).arg("sh")
        .arg(pid.to_string()).arg(bundle).arg(opener)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .process_group(0)
        .spawn()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_executable_inside_an_app_bundle_has_a_bundle() {
        assert_eq!(app_bundle(Path::new("/Applications/SottoASR.app/Contents/MacOS/sottoasr")),
            Some(PathBuf::from("/Applications/SottoASR.app")));
        assert_eq!(app_bundle(Path::new("/Users/me/My Apps/Sotto ASR.app/Contents/MacOS/sottoasr")),
            Some(PathBuf::from("/Users/me/My Apps/Sotto ASR.app")));
        assert_eq!(app_bundle(Path::new("/work/sottoasr/src-tauri/target/debug/sottoasr")), None);
        assert_eq!(app_bundle(Path::new("/opt/tool/Contents/MacOS/sottoasr")), None);
        assert_eq!(app_bundle(Path::new("sottoasr")), None);
    }

    #[test]
    fn the_bundle_is_opened_only_after_the_old_process_has_ended() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let opened = directory.path().join("opened");
        let opener = directory.path().join("open");
        std::fs::write(&opener, format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n", opened.display())).unwrap();
        std::fs::set_permissions(&opener, std::fs::Permissions::from_mode(0o755)).unwrap();
        let bundle = directory.path().join("Sotto ASR.app");

        let mut old = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let mut helper = open_after(old.id(), &bundle, &opener).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(400));
        assert!(!opened.exists(), "a second instance beside the running one");

        old.kill().unwrap();
        old.wait().unwrap();
        assert!(helper.wait().unwrap().success());
        assert_eq!(std::fs::read_to_string(&opened).unwrap(), format!("-n\n{}\n", bundle.display()));
    }
}
