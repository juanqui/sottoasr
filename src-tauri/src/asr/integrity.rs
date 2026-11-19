//! Pinned model manifest: the files the bridge loads and their SHA-256.
//!
//! - A file is fresh when the current preparation wrote it: it was absent or
//!   different when the preparation started. The start is saved before any
//!   download, so a preparation that stops early (quit, network loss, read
//!   error) is judged against it later.
//! - A fresh file that fails the manifest stops loading and stays on disk. It
//!   was never in use, so the next preparation that may download deletes it
//!   and fetches it again.
//! - A failing file that was already present loads with a warning: blocking
//!   it adds no protection for a file already in use. Only the user-confirmed
//!   re-download replaces it.
//! - A record next to the files keeps each file's verdict by identity (size,
//!   modification time, inode), so a file is hashed again only after it changes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// Hashes a file to lowercase hexadecimal SHA-256.
pub type Hasher<'a> = dyn Fn(&Path) -> Result<String, String> + 'a;

const RECORD: &str = ".sotto-integrity.json";
const ASIDE_PREFIX: &str = ".sotto-replaced-";

#[derive(Debug, serde::Deserialize)]
pub struct ModelFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct Manifest {
    pub repository: String,
    pub revision: String,
    /// The SDK's cache folder name under `FluidAudio/Models`.
    pub directory: String,
    pub files: Vec<ModelFile>,
}

#[derive(serde::Deserialize)]
struct Bundled {
    speech: Manifest,
    previous_speech: Manifest,
    vocabulary: Manifest,
}

static BUNDLED: LazyLock<Bundled> = LazyLock::new(|| {
    serde_json::from_str(fluidaudio_rs::MODEL_MANIFEST)
        .expect("the bundled model manifest is valid; a unit test parses it")
});

/// The speech model this build downloads and prefers.
pub fn speech() -> &'static Manifest {
    &BUNDLED.speech
}

/// The speech model earlier releases downloaded. It is never downloaded
/// again: an installation that has it keeps dictating with it until `speech`
/// is downloaded and verified.
pub fn previous_speech() -> &'static Manifest {
    &BUNDLED.previous_speech
}

pub fn vocabulary() -> &'static Manifest {
    &BUNDLED.vocabulary
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Fingerprint {
    size: u64,
    modified_ns: u64,
    inode: u64,
}

impl Fingerprint {
    fn of(path: &Path) -> Option<Self> {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::metadata(path).ok().filter(std::fs::Metadata::is_file)?;
        let modified_ns = metadata.modified().ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |elapsed| u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX));
        Some(Self { size: metadata.len(), modified_ns, inode: metadata.ino() })
    }
}

/// Fingerprints of the manifest files that are present, by path.
type Snapshot = BTreeMap<String, Fingerprint>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum Verdict {
    Verified,
    /// Failing, but already present when preparation started: in use.
    Warned,
    /// Failing and fresh: never loaded.
    Rejected,
}

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
struct Checked {
    fingerprint: Fingerprint,
    verdict: Verdict,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Record {
    revision: String,
    /// The files present when a download began. Kept until a verification
    /// has found every file, so a preparation that stopped early is judged
    /// against its real start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    started_with: Option<Snapshot>,
    #[serde(default)]
    files: BTreeMap<String, Checked>,
}

#[derive(Debug, Default)]
pub struct Report {
    pub missing: Vec<String>,
    /// Written by this or an unfinished earlier preparation, and not matching.
    pub fresh_mismatch: Vec<String>,
    /// Present before preparation started, and not matching.
    pub existing_mismatch: Vec<String>,
}

impl Report {
    pub fn is_clean(&self) -> bool {
        self.missing.is_empty() && !self.has_mismatch()
    }

    pub fn has_mismatch(&self) -> bool {
        !self.fresh_mismatch.is_empty() || !self.existing_mismatch.is_empty()
    }

    /// Missing or fresh failing files: nothing may load.
    pub fn blocks_loading(&self) -> bool {
        !self.missing.is_empty() || !self.fresh_mismatch.is_empty()
    }
}

impl Manifest {
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.size).sum()
    }

    /// Bytes on disk for the manifest files, counting the SDK's `.partial`
    /// staging files while they download.
    pub fn downloaded_bytes(&self, directory: &Path) -> u64 {
        self.files.iter().map(|file| {
            let path = directory.join(&file.path);
            std::fs::metadata(&path).or_else(|_| std::fs::metadata(with_suffix(&path, ".partial")))
                .map_or(0, |metadata| metadata.len().min(file.size))
        }).sum()
    }

    /// Whether any of this model's files exist, including files a stopped
    /// re-download moved aside.
    pub fn is_present(&self, directory: &Path) -> bool {
        !self.snapshot(directory).is_empty() || !self.stranded(directory).is_empty()
    }

    /// Downloads missing files at the pinned revision, then verifies. Deletes
    /// rejected files first so they are fetched again. Runs without the engine.
    pub fn prepare(
        &self,
        directory: &Path,
        download: impl FnOnce() -> Result<(), String>,
        hash: &Hasher<'_>,
    ) -> Result<Report, String> {
        self.run(directory, Some(download), hash)
    }

    /// Verifies the files on disk without downloading or deleting any.
    pub fn check(&self, directory: &Path, hash: &Hasher<'_>) -> Result<Report, String> {
        self.run(directory, None::<fn() -> Result<(), String>>, hash)
    }

    fn run<F: FnOnce() -> Result<(), String>>(
        &self,
        directory: &Path,
        download: Option<F>,
        hash: &Hasher<'_>,
    ) -> Result<Report, String> {
        let mut record = self.read_record(directory);
        if download.is_some() {
            self.delete_rejected(directory, &mut record);
        }
        let recovered = self.recover_stranded(directory);
        let current = self.snapshot(directory);
        let mut started_with = record.started_with.take().unwrap_or_else(|| current.clone());
        // Files a stopped re-download moved aside were in use before it.
        for path in recovered {
            if let Some(fingerprint) = current.get(&path) {
                started_with.insert(path, *fingerprint);
            }
        }
        if let Some(download) = download {
            if !self.is_complete(&current) {
                // The start must be on disk before anything downloads, or a stop
                // part-way (even on a first install, before the SDK creates the
                // folder) would make the finished files look as if in use.
                record.started_with = Some(started_with.clone());
                std::fs::create_dir_all(directory)
                    .map_err(|error| format!("Could not create {}: {error}", directory.display()))?;
                self.write_record(directory, &record)?;
                download()?;
            }
        }
        let report = self.verify(directory, &mut record, started_with, hash)?;
        if report.is_clean() {
            self.delete_stranded(directory);
        }
        Ok(report)
    }

    /// Gives every present file a verdict, hashing only files whose identity
    /// changed since the record was written, and saves the record. Entries of
    /// absent files stay, so a file moved aside and put back keeps its verdict.
    fn verify(
        &self,
        directory: &Path,
        record: &mut Record,
        started_with: Snapshot,
        hash: &Hasher<'_>,
    ) -> Result<Report, String> {
        let current = self.snapshot(directory);
        let mut report = Report::default();
        for file in &self.files {
            let Some(now) = current.get(&file.path) else {
                report.missing.push(file.path.clone());
                continue;
            };
            let known = record.files.get(&file.path)
                .filter(|checked| checked.fingerprint == *now)
                .map(|checked| checked.verdict);
            let verdict = match known {
                Some(verdict) => verdict,
                None => {
                    if self.matches(directory, file, now, hash)? {
                        Verdict::Verified
                    } else if started_with.get(&file.path) == Some(now) {
                        Verdict::Warned
                    } else {
                        Verdict::Rejected
                    }
                }
            };
            match verdict {
                Verdict::Verified => {}
                Verdict::Warned => report.existing_mismatch.push(file.path.clone()),
                Verdict::Rejected => report.fresh_mismatch.push(file.path.clone()),
            }
            record.files.insert(file.path.clone(), Checked { fingerprint: *now, verdict });
        }
        if !report.is_clean() {
            log::warn!("{} at {}: missing {:?}, new files failing {:?}, existing files failing {:?}",
                self.repository, self.revision, report.missing, report.fresh_mismatch, report.existing_mismatch);
        }
        record.started_with = (!report.missing.is_empty()).then_some(started_with);
        self.save_record(directory, record);
        Ok(report)
    }

    /// User-confirmed repair. Deletes rejected files (never in use), moves the
    /// other files that fail the manifest aside, downloads them again and
    /// verifies the whole model. The moved copies are deleted only after that
    /// succeeds and are moved back otherwise. Returns whether anything was
    /// replaced.
    pub fn replace_failing(
        &self,
        directory: &Path,
        label: &str,
        download: impl FnOnce() -> Result<(), String>,
        hash: &Hasher<'_>,
    ) -> Result<bool, String> {
        let mut record = self.read_record(directory);
        self.delete_rejected(directory, &mut record);
        let recovered = self.recover_stranded(directory);
        let current = self.snapshot(directory);
        // What counted as present before this repair, kept if it fails.
        let mut began_with = record.started_with.clone().unwrap_or_else(|| current.clone());
        for path in recovered {
            if let Some(fingerprint) = current.get(&path) {
                began_with.insert(path, *fingerprint);
            }
        }
        // The user asked for a repair, so every present file is hashed again.
        let mut failing = Vec::new();
        for file in &self.files {
            if let Some(now) = current.get(&file.path) {
                if !self.matches(directory, file, now, hash)? {
                    failing.push(file.path.as_str());
                }
            }
        }
        if failing.is_empty() && self.is_complete(&current) {
            record.started_with = None;
            record.files = current.into_iter()
                .map(|(path, fingerprint)| (path, Checked { fingerprint, verdict: Verdict::Verified }))
                .collect();
            self.save_record(directory, &record);
            self.delete_stranded(directory);
            return Ok(false);
        }
        let previous = record.files.clone();
        let aside = directory.join(format!("{ASIDE_PREFIX}{}", uuid::Uuid::new_v4()));
        let mut moved = Vec::new();
        for path in failing {
            let target = aside.join(path);
            let result = target.parent().map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::rename(directory.join(path), &target));
            if let Err(error) = result {
                let unrestored = restore(directory, &aside, &moved);
                return Err(restore_message(format!("Could not move {path} aside: {error}."), &unrestored, &aside));
            }
            moved.push(path);
        }
        // The moved files are now absent, so their replacements count as fresh.
        let mut started_with = began_with.clone();
        for path in &moved {
            started_with.remove(*path);
        }
        record.started_with = Some(started_with.clone());
        let saved = std::fs::create_dir_all(directory)
            .map_err(|error| format!("Could not create {}: {error}", directory.display()))
            .and_then(|()| self.write_record(directory, &record));
        if let Err(error) = saved {
            let unrestored = restore(directory, &aside, &moved);
            return Err(restore_message(error, &unrestored, &aside));
        }
        let outcome = download().and_then(|()| {
            let report = self.verify(directory, &mut record, started_with, hash)?;
            if !report.missing.is_empty() {
                Err(format!("The {label} download is incomplete."))
            } else if report.has_mismatch() {
                Err(format!("The downloaded {label} failed verification."))
            } else {
                Ok(())
            }
        });
        match outcome {
            Ok(()) => {
                // Deletes the moved copies, and any a stopped earlier repair left.
                self.delete_stranded(directory);
                Ok(true)
            }
            Err(error) => {
                let unrestored = restore(directory, &aside, &moved);
                for path in &moved {
                    delete_staging(&directory.join(path));
                }
                // Restored files get their earlier verdicts back, and the start
                // is what it was before the repair. New files that stayed, such
                // as a formerly missing file that failed, are absent from that
                // start and keep their rejection, so they stay fresh.
                for path in moved.iter().copied().filter(|path| !unrestored.contains(path)) {
                    match previous.get(path) {
                        Some(checked) => record.files.insert(path.to_owned(), *checked),
                        None => record.files.remove(path),
                    };
                }
                record.started_with = Some(began_with);
                self.save_record(directory, &record);
                Err(restore_message(error, &unrestored, &aside))
            }
        }
    }

    fn snapshot(&self, directory: &Path) -> Snapshot {
        self.files.iter()
            .filter_map(|file| Some((file.path.clone(), Fingerprint::of(&directory.join(&file.path))?)))
            .collect()
    }

    fn is_complete(&self, snapshot: &Snapshot) -> bool {
        self.files.iter().all(|file| snapshot.contains_key(&file.path))
    }

    fn matches(&self, directory: &Path, file: &ModelFile, now: &Fingerprint, hash: &Hasher<'_>) -> Result<bool, String> {
        if now.size != file.size {
            return Ok(false);
        }
        let digest = hash(&directory.join(&file.path))
            .map_err(|error| format!("Could not read model file {}: {error}", file.path))?;
        Ok(digest == file.sha256)
    }

    /// A rejected file was never in use: delete it, with any download staging
    /// files, so the next download fetches it again.
    fn delete_rejected(&self, directory: &Path, record: &mut Record) {
        for file in &self.files {
            let path = directory.join(&file.path);
            let rejected = record.files.get(&file.path).is_some_and(|checked| {
                checked.verdict == Verdict::Rejected && Fingerprint::of(&path) == Some(checked.fingerprint)
            });
            if !rejected {
                continue;
            }
            match std::fs::remove_file(&path) {
                Ok(()) => {
                    record.files.remove(&file.path);
                    delete_staging(&path);
                }
                Err(error) => log::warn!("Could not delete rejected model file {}: {error}", file.path),
            }
        }
    }

    /// Folders a re-download moved files into, left behind if it stopped
    /// (for example, the app quit) before restoring or deleting them.
    fn stranded(&self, directory: &Path) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(directory) else { return Vec::new() };
        entries.flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(ASIDE_PREFIX)
                && entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .map(|entry| entry.path())
            .collect()
    }

    /// Moves back every stranded file whose place in the model folder is
    /// empty. Returns the restored paths.
    fn recover_stranded(&self, directory: &Path) -> Vec<String> {
        let mut recovered = Vec::new();
        for aside in self.stranded(directory) {
            for file in &self.files {
                let (moved, original) = (aside.join(&file.path), directory.join(&file.path));
                if Fingerprint::of(&moved).is_none() || original.symlink_metadata().is_ok() {
                    continue;
                }
                let result = original.parent().map_or(Ok(()), std::fs::create_dir_all)
                    .and_then(|()| std::fs::rename(&moved, &original));
                match result {
                    Ok(()) => recovered.push(file.path.clone()),
                    Err(error) => log::warn!("Could not put model file {} back: {error}", file.path),
                }
            }
            remove_empty_directories(&aside);
        }
        recovered
    }

    /// Once the model verifies, stranded copies are superseded.
    fn delete_stranded(&self, directory: &Path) {
        for aside in self.stranded(directory) {
            if let Err(error) = std::fs::remove_dir_all(&aside) {
                log::warn!("Could not delete replaced model files at {}: {error}", aside.display());
            }
        }
    }

    fn read_record(&self, directory: &Path) -> Record {
        let Some(mut record) = std::fs::read(directory.join(RECORD)).ok()
            .and_then(|bytes| serde_json::from_slice::<Record>(&bytes).ok())
        else {
            return Record { revision: self.revision.clone(), ..Record::default() };
        };
        if record.revision != self.revision {
            // A new pin judges every file again, but a file that was never in
            // use stays fresh: keep rejections and the saved start.
            record.files.retain(|_, checked| checked.verdict == Verdict::Rejected);
            record.revision.clone_from(&self.revision);
        }
        record
    }

    fn write_record(&self, directory: &Path, record: &Record) -> Result<(), String> {
        let path = directory.join(RECORD);
        let staging = directory.join(format!("{RECORD}.tmp"));
        serde_json::to_vec(record).map_err(|error| error.to_string())
            .and_then(|bytes| std::fs::write(&staging, bytes).map_err(|error| error.to_string()))
            .and_then(|()| std::fs::rename(&staging, &path).map_err(|error| error.to_string()))
            .map_err(|error| format!("Could not record model verification at {}: {error}", path.display()))
    }

    /// Saves verdicts when the folder exists. A lost record costs only a hash
    /// at the next launch: the start that decides freshness is saved first.
    fn save_record(&self, directory: &Path, record: &Record) {
        if !directory.is_dir() {
            return;
        }
        if let Err(error) = self.write_record(directory, record) {
            log::warn!("{error}");
        }
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    name.into()
}

/// Removes the SDK's resumable download files for `path`.
fn delete_staging(path: &Path) {
    for staged in [with_suffix(path, ".partial"), with_suffix(path, ".partial.etag")] {
        let _ = std::fs::remove_file(staged);
    }
}

/// Moves files back from `aside`, replacing whatever a failed download left.
/// Returns the paths that could not be put back.
fn restore<'a>(directory: &Path, aside: &Path, moved: &[&'a str]) -> Vec<&'a str> {
    let mut unrestored = Vec::new();
    for path in moved {
        if let Err(error) = std::fs::rename(aside.join(path), directory.join(path)) {
            log::error!("Could not put model file {path} back: {error}");
            unrestored.push(*path);
        }
    }
    remove_empty_directories(aside);
    unrestored
}

fn restore_message(error: String, unrestored: &[&str], aside: &Path) -> String {
    if unrestored.is_empty() {
        format!("{error} Your existing files were kept.")
    } else {
        format!("{error} Some existing files could not be put back yet; they are in {} and are restored at the next launch.",
            aside.display())
    }
}

/// Removes `path` and its subfolders if they hold no files.
fn remove_empty_directories(path: &Path) {
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                remove_empty_directories(&entry.path());
            }
        }
    }
    let _ = std::fs::remove_dir(path);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const GOOD: &[u8] = b"pinned model bytes";
    const GOOD_SHA256: &str = "7826512858078d3d070641414fc406bd3d4f82b60d83968427eb3e5dd7d1377e";
    // Same length as GOOD, so only the digest can tell them apart.
    const TAMPERED: &[u8] = b"pinned model BYTES";
    const WEIGHTS: &str = "Encoder.mlmodelc/weights/weight.bin";
    const VOCABULARY: &str = "vocab.json";

    fn manifest() -> Manifest {
        let file = |path: &str| ModelFile { path: path.into(), size: GOOD.len() as u64, sha256: GOOD_SHA256.into() };
        Manifest {
            repository: "test/model".into(),
            revision: "0".repeat(40),
            directory: "model".into(),
            files: vec![file(WEIGHTS), file(VOCABULARY)],
        }
    }

    fn put(directory: &Path, path: &str, bytes: &[u8]) {
        let path = directory.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        std::fs::write(path, bytes).unwrap();
    }

    fn read(directory: &Path, path: &str) -> Vec<u8> {
        std::fs::read(directory.join(path)).unwrap()
    }

    fn counting(calls: &Cell<usize>) -> impl Fn(&Path) -> Result<String, String> + '_ {
        move |path: &Path| {
            calls.set(calls.get() + 1);
            fluidaudio_rs::sha256_file(path)
        }
    }

    fn sha256(path: &Path) -> Result<String, String> {
        fluidaudio_rs::sha256_file(path)
    }

    fn offline() -> Result<(), String> {
        Err("offline".into())
    }

    fn aside_directories(directory: &Path) -> usize {
        std::fs::read_dir(directory).unwrap().flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(ASIDE_PREFIX))
            .count()
    }

    #[test]
    fn bridge_hashes_files_across_read_chunks() {
        let directory = tempfile::tempdir().unwrap();
        let bytes: Vec<u8> = (0..3 * 1_048_576 + 7).map(|index| (index % 251) as u8).collect();
        put(directory.path(), "large.bin", &bytes);
        put(directory.path(), "empty.bin", b"");
        assert_eq!(sha256(&directory.path().join("large.bin")).unwrap(),
            "f578a61853ca2f4272dba551bd868420e22302fbb6d6dfc0cc80da1d2c7b779f");
        assert_eq!(sha256(&directory.path().join("empty.bin")).unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert!(sha256(&directory.path().join("absent.bin")).is_err());
    }

    #[test]
    fn fresh_mismatched_download_is_an_error_and_stays_on_disk() {
        let directory = tempfile::tempdir().unwrap();
        let manifest = manifest();
        put(directory.path(), VOCABULARY, GOOD);
        let report = manifest.prepare(directory.path(), || {
            put(directory.path(), WEIGHTS, TAMPERED);
            Ok(())
        }, &sha256).unwrap();
        assert_eq!(report.fresh_mismatch, [WEIGHTS]);
        assert!(report.existing_mismatch.is_empty() && report.missing.is_empty());
        assert!(report.blocks_loading());
        assert_eq!(read(directory.path(), WEIGHTS), TAMPERED);
        // Verifying without a download never deletes it and never loads it.
        let report = manifest.check(directory.path(), &sha256).unwrap();
        assert_eq!(report.fresh_mismatch, [WEIGHTS]);
        assert_eq!(read(directory.path(), WEIGHTS), TAMPERED);
    }

    #[test]
    fn stopped_preparation_still_judges_its_files_as_fresh() {
        let directory = tempfile::tempdir().unwrap();
        let manifest = manifest();
        put(directory.path(), VOCABULARY, GOOD);
        // The tampered file lands, then the network drops on the next one.
        let error = manifest.prepare(directory.path(), || {
            put(directory.path(), WEIGHTS, TAMPERED);
            Err("network lost".into())
        }, &sha256).unwrap_err();
        assert_eq!(error, "network lost");
        let report = manifest.prepare(directory.path(), offline, &sha256).unwrap();
        assert_eq!(report.fresh_mismatch, [WEIGHTS]);
        assert!(report.blocks_loading());

        // A read error that stops verification part-way behaves the same.
        let directory = tempfile::tempdir().unwrap();
        put(directory.path(), VOCABULARY, GOOD);
        let unreadable = |_: &Path| Err::<String, String>("read error".into());
        assert!(manifest.prepare(directory.path(), || {
            put(directory.path(), WEIGHTS, TAMPERED);
            Ok(())
        }, &unreadable).is_err());
        let report = manifest.prepare(directory.path(), offline, &sha256).unwrap();
        assert_eq!(report.fresh_mismatch, [WEIGHTS]);

        // On a first install the folder does not exist until the download
        // creates it; the start is saved before that anyway.
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("model");
        let error = manifest.prepare(&directory, || {
            put(&directory, VOCABULARY, GOOD);
            put(&directory, WEIGHTS, TAMPERED);
            Err("app quit".into())
        }, &sha256).unwrap_err();
        assert_eq!(error, "app quit");
        let report = manifest.prepare(&directory, offline, &sha256).unwrap();
        assert_eq!(report.fresh_mismatch, [WEIGHTS]);
        assert!(report.existing_mismatch.is_empty());

        // The folder made for the saved start is not taken for a cache.
        let never_downloaded = root.path().join("never-downloaded");
        assert!(manifest.prepare(&never_downloaded, offline, &sha256).is_err());
        assert!(never_downloaded.is_dir() && !manifest.is_present(&never_downloaded));
    }

    #[test]
    fn rejected_download_is_deleted_and_fetched_again() {
        let directory = tempfile::tempdir().unwrap();
        let manifest = manifest();
        put(directory.path(), VOCABULARY, GOOD);
        let report = manifest.prepare(directory.path(), || {
            put(directory.path(), WEIGHTS, TAMPERED);
            Ok(())
        }, &sha256).unwrap();
        assert!(report.blocks_loading());
        let leftover = format!("{WEIGHTS}.partial");
        put(directory.path(), &leftover, b"stale");

        let downloads = Cell::new(0);
        let report = manifest.prepare(directory.path(), || {
            downloads.set(downloads.get() + 1);
            assert!(!directory.path().join(WEIGHTS).exists() && !directory.path().join(&leftover).exists());
            put(directory.path(), WEIGHTS, GOOD);
            Ok(())
        }, &sha256).unwrap();
        assert!(report.is_clean());
        assert_eq!(downloads.get(), 1);
        assert_eq!(read(directory.path(), WEIGHTS), GOOD);
    }

    #[test]
    fn existing_mismatch_warns_every_launch_without_hashing_again() {
        let directory = tempfile::tempdir().unwrap();
        let manifest = manifest();
        put(directory.path(), WEIGHTS, TAMPERED);
        put(directory.path(), VOCABULARY, GOOD);
        for expected_hashes in [2, 0] {
            let calls = Cell::new(0);
            let report = manifest.prepare(directory.path(), offline, &counting(&calls)).unwrap();
            assert_eq!(report.existing_mismatch, [WEIGHTS]);
            assert!(report.fresh_mismatch.is_empty() && report.missing.is_empty());
            assert!(report.has_mismatch() && !report.blocks_loading());
            assert_eq!(calls.get(), expected_hashes);
        }
        assert_eq!(read(directory.path(), WEIGHTS), TAMPERED);

        // Only a file whose identity changed is hashed again.
        put(directory.path(), WEIGHTS, GOOD);
        let calls = Cell::new(0);
        assert!(manifest.prepare(directory.path(), offline, &counting(&calls)).unwrap().is_clean());
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn clean_verification_is_recorded_until_a_file_changes() {
        let directory = tempfile::tempdir().unwrap();
        let manifest = manifest();
        let report = manifest.prepare(directory.path(), || {
            put(directory.path(), WEIGHTS, GOOD);
            put(directory.path(), VOCABULARY, GOOD);
            Ok(())
        }, &sha256).unwrap();
        assert!(report.is_clean());
        assert_eq!(manifest.downloaded_bytes(directory.path()), manifest.total_bytes());

        let calls = Cell::new(0);
        assert!(manifest.prepare(directory.path(), offline, &counting(&calls)).unwrap().is_clean());
        assert_eq!(calls.get(), 0);

        put(directory.path(), WEIGHTS, TAMPERED);
        let report = manifest.prepare(directory.path(), offline, &counting(&calls)).unwrap();
        assert_eq!(report.existing_mismatch, [WEIGHTS]);
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn stranded_replacement_is_restored_offline() {
        let directory = tempfile::tempdir().unwrap();
        let manifest = manifest();
        put(directory.path(), VOCABULARY, GOOD);
        // A re-download moved the in-use file aside, saved a start without
        // it, and then the app quit.
        let mut started_with = Snapshot::new();
        started_with.insert(VOCABULARY.into(), Fingerprint::of(&directory.path().join(VOCABULARY)).unwrap());
        let record = Record { revision: manifest.revision.clone(), started_with: Some(started_with), files: BTreeMap::new() };
        manifest.write_record(directory.path(), &record).unwrap();
        let aside = format!("{ASIDE_PREFIX}stopped");
        put(&directory.path().join(&aside), WEIGHTS, TAMPERED);
        assert!(manifest.is_present(directory.path()));

        let report = manifest.prepare(directory.path(), offline, &sha256).unwrap();
        assert_eq!(report.existing_mismatch, [WEIGHTS], "a restored file was in use");
        assert!(!report.blocks_loading());
        assert_eq!(read(directory.path(), WEIGHTS), TAMPERED);
        assert_eq!(aside_directories(directory.path()), 0);

        // A stranded old copy whose replacement verifies is deleted.
        put(directory.path(), WEIGHTS, GOOD);
        put(&directory.path().join(&aside), WEIGHTS, TAMPERED);
        assert!(manifest.check(directory.path(), &sha256).unwrap().is_clean());
        assert_eq!(read(directory.path(), WEIGHTS), GOOD);
        assert_eq!(aside_directories(directory.path()), 0);
    }

    #[test]
    fn replacement_restores_files_on_failure_and_deletes_them_after_success() {
        let directory = tempfile::tempdir().unwrap();
        let manifest = manifest();
        put(directory.path(), WEIGHTS, TAMPERED);
        put(directory.path(), VOCABULARY, GOOD);

        let error = manifest.replace_failing(directory.path(), "test model", || {
            put(directory.path(), WEIGHTS, b"partial");
            put(directory.path(), &format!("{WEIGHTS}.partial"), b"partial");
            put(directory.path(), &format!("{WEIGHTS}.partial.etag"), b"etag");
            Err("Network unavailable.".into())
        }, &sha256).unwrap_err();
        assert!(error.starts_with("Network unavailable."), "{error}");
        assert_eq!(read(directory.path(), WEIGHTS), TAMPERED);
        assert_eq!(aside_directories(directory.path()), 0);
        assert!(!directory.path().join(format!("{WEIGHTS}.partial")).exists());
        assert!(!directory.path().join(format!("{WEIGHTS}.partial.etag")).exists());
        // The restored file is the one in use: it still only warns.
        let report = manifest.prepare(directory.path(), offline, &sha256).unwrap();
        assert_eq!(report.existing_mismatch, [WEIGHTS]);
        assert!(!report.blocks_loading());

        assert!(manifest.replace_failing(directory.path(), "test model", || {
            put(directory.path(), WEIGHTS, GOOD);
            Ok(())
        }, &sha256).unwrap());
        assert_eq!(read(directory.path(), WEIGHTS), GOOD);
        assert_eq!(aside_directories(directory.path()), 0);
        let calls = Cell::new(0);
        assert!(manifest.prepare(directory.path(), offline, &counting(&calls)).unwrap().is_clean());
        assert_eq!(calls.get(), 0);

        assert!(!manifest.replace_failing(directory.path(), "test model", offline, &sha256).unwrap());
    }

    #[test]
    fn failed_replacement_keeps_new_files_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let manifest = manifest();
        put(directory.path(), WEIGHTS, TAMPERED);
        // The vocabulary file was missing, so its download is new.
        let error = manifest.replace_failing(directory.path(), "test model", || {
            put(directory.path(), WEIGHTS, TAMPERED);
            put(directory.path(), VOCABULARY, TAMPERED);
            Ok(())
        }, &sha256).unwrap_err();
        assert!(error.contains("failed verification"), "{error}");
        assert_eq!(read(directory.path(), VOCABULARY), TAMPERED);

        let report = manifest.check(directory.path(), &sha256).unwrap();
        assert_eq!(report.existing_mismatch, [WEIGHTS]);
        assert_eq!(report.fresh_mismatch, [VOCABULARY]);
        let downloads = Cell::new(0);
        let report = manifest.prepare(directory.path(), || {
            downloads.set(downloads.get() + 1);
            put(directory.path(), VOCABULARY, GOOD);
            Ok(())
        }, &sha256).unwrap();
        assert_eq!(downloads.get(), 1, "the rejected new file is fetched again");
        assert_eq!(report.existing_mismatch, [WEIGHTS]);
        assert!(!report.blocks_loading());
    }

    #[test]
    fn replacement_deletes_a_rejected_file_instead_of_moving_it_aside() {
        let directory = tempfile::tempdir().unwrap();
        let manifest = manifest();
        put(directory.path(), VOCABULARY, GOOD);
        assert!(manifest.prepare(directory.path(), || {
            put(directory.path(), WEIGHTS, TAMPERED);
            Ok(())
        }, &sha256).unwrap().blocks_loading());

        // The replacement lands, then the app quits while it is hashed.
        let quit_while_hashing = |path: &Path| {
            if path.ends_with(WEIGHTS) { Err("app quit".into()) } else { sha256(path) }
        };
        assert!(manifest.replace_failing(directory.path(), "test model", || {
            put(directory.path(), WEIGHTS, TAMPERED);
            Ok(())
        }, &quit_while_hashing).is_err());
        assert_eq!(aside_directories(directory.path()), 0, "a rejected file is never moved aside");

        // Nothing that was never in use can come back as in use.
        let report = manifest.check(directory.path(), &sha256).unwrap();
        assert_eq!(report.fresh_mismatch, [WEIGHTS]);
        assert!(report.existing_mismatch.is_empty());
        assert!(manifest.prepare(directory.path(), || {
            put(directory.path(), WEIGHTS, GOOD);
            Ok(())
        }, &sha256).unwrap().is_clean());
    }

    #[test]
    fn new_pin_keeps_rejections_and_the_saved_start() {
        let old = manifest();
        let mut new = manifest();
        new.revision = "1".repeat(40);

        let directory = tempfile::tempdir().unwrap();
        put(directory.path(), VOCABULARY, GOOD);
        assert!(old.prepare(directory.path(), || {
            put(directory.path(), WEIGHTS, TAMPERED);
            Ok(())
        }, &sha256).unwrap().blocks_loading());
        let calls = Cell::new(0);
        let report = new.check(directory.path(), &counting(&calls)).unwrap();
        assert_eq!(report.fresh_mismatch, [WEIGHTS], "a rejected file stays fresh under a new pin");
        assert_eq!(calls.get(), 1, "the verified file is judged again");

        // A start saved under the old pin still decides what is fresh.
        let directory = tempfile::tempdir().unwrap();
        put(directory.path(), VOCABULARY, GOOD);
        assert!(old.prepare(directory.path(), || {
            put(directory.path(), WEIGHTS, TAMPERED);
            Err("app quit".into())
        }, &sha256).is_err());
        let report = new.prepare(directory.path(), offline, &sha256).unwrap();
        assert_eq!(report.fresh_mismatch, [WEIGHTS]);
    }

    #[test]
    fn bundled_manifest_matches_the_pinned_download_revisions() {
        let patch = include_str!("../../vendor/fluidaudio-rs/patches/model-registry-revision.patch");
        for manifest in [speech(), previous_speech(), vocabulary()] {
            assert_eq!(manifest.revision.len(), 40);
            let mut paths: Vec<_> = manifest.files.iter().map(|file| file.path.as_str()).collect();
            paths.sort_unstable();
            paths.dedup();
            assert_eq!(paths.len(), manifest.files.len());
            assert!(manifest.files.iter().all(|file| file.size > 0 && file.sha256.len() == 64));
        }
        for manifest in [speech(), vocabulary()] {
            assert!(patch.contains(&format!("\"{}\": \"{}\"", manifest.repository, manifest.revision)),
                "{} is not pinned to the manifest revision", manifest.repository);
        }
        // The previous model is only verified and loaded, never downloaded.
        assert!(!patch.contains(&previous_speech().repository));
        assert_eq!(speech().directory, "parakeet-ultra");
        assert_eq!(previous_speech().directory, "parakeet-tdt-0.6b-v3");
        assert_eq!(vocabulary().directory, "parakeet-ctc-110m-coreml");
        for manifest in [speech(), previous_speech()] {
            assert!(manifest.files.iter().any(|file| file.path == "parakeet_vocab.json"));
        }
        assert!(vocabulary().files.iter().any(|file| file.path == "tokenizer.json"));
    }
}
