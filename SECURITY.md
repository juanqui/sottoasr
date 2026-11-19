# Security Policy

## Supported versions

We release security fixes for the latest published version of SottoASR. The
app updates itself from GitHub Releases. Before it installs an update, it
verifies the update against the signing key that is built into the app.

## Reporting a vulnerability

Do not open a public issue for a security problem.

- Preferred: use the private report form of GitHub at
  <https://github.com/juanqui/sottoasr/security/advisories/new>.
- Or send an email to the maintainer at <juan@villafam.com>.

Include the app version (About window), your macOS version, the steps to
reproduce, and what an attacker gets. Do not attach real dictation audio,
transcripts, or your history file. Use synthetic examples.

## What SottoASR does on the network

Speech recognition and transcript cleanup run on this Mac. SottoASR never
sends audio or transcript text over the network. The app makes these outbound
requests:

| Request | Destination | When |
| --- | --- | --- |
| App update check and download | `github.com` release assets, verified by the update signature | 15 s after launch, then every 4 h, if "Check for updates" is on in Settings. Also when you open "Check for Updates..." from the menu bar icon, and when you install an update. |
| Speech and vocabulary models | `huggingface.co` (file list and files), pinned revisions, verified by SHA-256 | When model files are missing, for example at first launch, or a newly downloaded file failed verification. When you save vocabulary words and the vocabulary model is not ready. When you confirm a re-download in Settings. |
| Cleanup runtime packages | PyPI (`pypi.org`, `files.pythonhosted.org`), hash-locked wheels only | At launch with AI cleanup on, or when you turn cleanup on, if the runtime is missing or differs from the bundled lock file. Each time you repair the runtime. |
| Cleanup model | `huggingface.co` (model information and files), pinned revision, verified by SHA-256 | When AI cleanup is on and the model is not downloaded: when you turn cleanup on, or at launch. |
| Standalone Python | `github.com` (python-build-standalone release), pinned URL, verified by SHA-256 | When SottoASR builds, updates or repairs the cleanup runtime and no native arm64 Python 3.11 to 3.14 is installed. |

These downloads follow HTTP redirects to the file servers of GitHub and
Hugging Face. The cleanup model runs in a separate process with the Hugging
Face offline mode on, so dictation itself makes no network request.

The app windows cannot connect to other hosts: their content security policy
allows only the app itself. The app opens links in your browser only for
`https` addresses on `huggingface.co`, `github.com` and `sottoasr.app`.

## Data on disk

SottoASR keeps settings, history and kept recordings in
`~/Library/Application Support/com.sottoasr.app/`. Only your user account can
read the settings file, the history file, the `recordings` folder and each
recording. CSV exports in your Downloads folder have the same permissions.

After SottoASR saves a dictation in History, it deletes the recording of that
dictation. A recording stays on disk when the dictation failed or was
interrupted, so that you can recover it. History and kept recordings stay until
you delete them or set a retention period in Settings.

When SottoASR pastes and then restores your clipboard, it marks the transcript
on the clipboard as transient and concealed. Clipboard managers that obey these
markers do not record it, and the transcript does not go to Universal
Clipboard. If you turn off clipboard restore, the transcript stays on the
clipboard as a normal copy.

Models are stored outside the app folder:

- Speech and vocabulary models: `~/Library/Application Support/FluidAudio/Models/`.
- Cleanup model: `~/.cache/huggingface/hub/models--openbmb--MiniCPM5-2B-MLX/`.
- Cleanup runtime and standalone Python: `llm-venv/` and `python/` in the app
  folder.
