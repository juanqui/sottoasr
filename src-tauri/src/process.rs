//! Owned child processes, bounded commands, and explicit application exit cleanup.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc, LazyLock, Mutex, MutexGuard, Weak};
use std::thread;
use std::time::Duration;

/// Tauri exits without guaranteed managed-state Drop. Keep weak references to
/// actual owned Child handles, never a list of potentially recycled PIDs.
#[derive(Default)]
struct ProcessRegistry {
    inner: Mutex<ProcessRegistryState>,
}

#[derive(Default)]
struct ProcessRegistryState {
    exiting: bool,
    children: Vec<Weak<Mutex<Child>>>,
}

static PROCESSES: LazyLock<ProcessRegistry> = LazyLock::new(ProcessRegistry::default);

impl ProcessRegistry {
    fn spawn(&self, command: &mut Command) -> Result<OwnedChild, String> {
        let mut registry = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if registry.exiting {
            return Err("Application is shutting down".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let child = command
            .spawn()
            .map_err(|e| format!("Could not start child process: {e}"))?;
        let child = Arc::new(Mutex::new(child));
        registry.children.retain(|entry| entry.strong_count() > 0);
        registry.children.push(Arc::downgrade(&child));
        Ok(OwnedChild(child))
    }

    fn terminate(&self, pid: u32) {
        let registry = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        for child in registry.children.iter().filter_map(Weak::upgrade) {
            let mut child = child.lock().unwrap_or_else(|e| e.into_inner());
            if child.id() == pid && matches!(child.try_wait(), Ok(None)) {
                terminate_child(&mut child);
                break;
            }
        }
    }

    fn is_alive(&self, pid: u32) -> bool {
        let registry = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        registry.children.iter().filter_map(Weak::upgrade).any(|child| {
            let mut child = child.lock().unwrap_or_else(|e| e.into_inner());
            child.id() == pid && matches!(child.try_wait(), Ok(None))
        })
    }

    fn shutdown(&self) {
        let children = {
            let mut registry = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            registry.exiting = true;
            std::mem::take(&mut registry.children)
        };
        for child in children.into_iter().filter_map(|child| child.upgrade()) {
            terminate_child(&mut child.lock().unwrap_or_else(|e| e.into_inner()));
        }
    }
}

/// Also owns initialization failures: an early `?` kills and reaps the child.
pub(crate) struct OwnedChild(Arc<Mutex<Child>>);

impl OwnedChild {
    pub(crate) fn spawn(command: &mut Command) -> Result<Self, String> {
        PROCESSES.spawn(command)
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, Child> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn id(&self) -> u32 {
        self.lock().id()
    }

    pub(crate) fn terminate(&self) {
        terminate_child(&mut self.lock());
    }
}

fn terminate_child(child: &mut Child) {
    // try_wait records completion on the Child, preventing a later Drop or
    // exit callback from signalling a PID that the OS has already recycled.
    loop {
        match child.try_wait() {
            Ok(None) => break,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            // An unknown/reaped child cannot safely authorize a PID signal.
            Ok(Some(_)) | Err(_) => return,
        }
    }
    #[cfg(unix)]
    unsafe {
        // Every registered process leads its own process group. Include
        // package-manager workers without touching unrelated processes.
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.terminate();
    }
}

/// Terminate all registered application children before Tauri exits or restarts.
pub(crate) fn shutdown() {
    PROCESSES.shutdown();
}

pub(crate) fn terminate_registered(pid: u32) {
    PROCESSES.terminate(pid);
}

pub(crate) fn registered_is_alive(pid: i32) -> bool {
    pid > 0 && PROCESSES.is_alive(pid as u32)
}

/// Bound a subprocess and its owned process group. Output readers keep pipes
/// flowing while try_wait enforces the deadline, retaining at most 1 MiB per pipe.
pub(crate) fn bounded_command(
    command: &mut Command,
    timeout: Duration,
) -> Result<std::process::Output, String> {
    let child = PROCESSES.spawn(command.stdout(Stdio::piped()).stderr(Stdio::piped()))?;
    fn drain(mut stream: impl Read + Send + 'static) -> mpsc::Receiver<Vec<u8>> {
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let mut saved = Vec::new();
            let mut buffer = [0; 4096];
            while let Ok(n) = stream.read(&mut buffer) {
                if n == 0 {
                    break;
                }
                let remaining = (1024usize * 1024).saturating_sub(saved.len());
                saved.extend_from_slice(&buffer[..n.min(remaining)]);
            }
            let _ = sender.send(saved);
        });
        receiver
    }
    let stdout = drain(child.lock().stdout.take().ok_or("Missing command stdout")?);
    let stderr = drain(child.lock().stderr.take().ok_or("Missing command stderr")?);
    let started = std::time::Instant::now();
    let status = loop {
        let result = child.lock().try_wait();
        match result {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < timeout => thread::sleep(Duration::from_millis(20)),
            result => {
                child.terminate();
                return Err(match result {
                    Err(error) => format!("Child process failed: {error}"),
                    _ => format!(
                        "Child process timed out after {} seconds",
                        timeout.as_secs()
                    ),
                });
            }
        }
    };
    Ok(std::process::Output {
        status,
        stdout: stdout
            .recv_timeout(timeout.saturating_sub(started.elapsed()))
            .unwrap_or_default(),
        stderr: stderr
            .recv_timeout(timeout.saturating_sub(started.elapsed()))
            .unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_uses_owned_live_child_state_and_detects_exit() {
        let child = OwnedChild::spawn(Command::new("sh").args(["-c", "sleep 20"])).unwrap();
        let pid = child.id() as i32;
        assert!(registered_is_alive(pid));
        assert!(!registered_is_alive(0));
        child.terminate();
        assert!(!registered_is_alive(pid));
    }

    #[test]
    #[cfg(unix)]
    fn incomplete_process_construction_kills_and_reaps_child() {
        fn incomplete(pid: &mut u32) -> Result<(), String> {
            let child = PROCESSES
                .spawn(Command::new("python3").args(["-c", "import time; time.sleep(20)"]))
                .unwrap();
            *pid = child.id();
            let _owned = child;
            Err("simulated pipe or reader initialization failure".into())
        }
        let mut pid = 0;
        assert!(incomplete(&mut pid).is_err());
        // A zombie remains addressable until it is reaped, so ESRCH verifies
        // both termination and wait(), rather than just delivery of SIGKILL.
        assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }

    #[test]
    fn exit_terminates_owned_children_and_rejects_late_spawns() {
        let registry = ProcessRegistry::default();
        let unrelated_registry = ProcessRegistry::default();
        let unrelated = unrelated_registry
            .spawn(Command::new("python3").args(["-c", "import time; time.sleep(20)"]))
            .unwrap();
        let child = registry
            .spawn(Command::new("python3").args(["-c", "import time; time.sleep(20)"]))
            .unwrap();
        registry.shutdown();
        assert!(child.lock().try_wait().unwrap().is_some());
        assert!(unrelated.lock().try_wait().unwrap().is_none());
        assert!(registry
            .spawn(Command::new("python3").args(["-c", "pass"]))
            .is_err());
        registry.shutdown(); // Repeated exit and subsequent Drop are harmless.
    }

    #[test]
    fn completed_process_is_safe_to_terminate_again() {
        let registry = ProcessRegistry::default();
        let child = registry
            .spawn(Command::new("python3").args(["-c", "pass"]))
            .unwrap();
        assert!(child.lock().wait().unwrap().success());
        registry.terminate(child.id());
        registry.shutdown();
        assert!(child.lock().try_wait().unwrap().unwrap().success());
    }

    #[test]
    fn command_deadline_and_full_pipes_are_bounded() {
        let result = bounded_command(
            Command::new("python3").args(["-c", "import time; time.sleep(20)"]),
            Duration::from_millis(80),
        );
        assert!(result.unwrap_err().contains("timed out"));
        let result = bounded_command(
            Command::new("python3").args([
                "-c",
                "import sys; sys.stderr.write('x'*100000); print('ready')",
            ]),
            Duration::from_secs(2),
        )
        .unwrap();
        assert!(result.status.success());
        assert_eq!(result.stdout, b"ready\n");
        assert_eq!(result.stderr.len(), 100000);
    }
}
