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

By default, speech recognition and transcript cleanup run on this Mac, and
SottoASR sends no audio or transcript text over the network. Settings has two
optional cloud engines: AWS Transcribe for speech and AWS Bedrock for AI
cleanup. If you turn on AWS Transcribe, SottoASR sends the audio of your
dictations to Amazon Transcribe in the AWS Region that you select. If you turn
on AWS Bedrock, SottoASR sends the parts of your dictation text that cleanup
can change to Amazon Bedrock in the AWS Region that you select. The app makes
these outbound requests:

| Request | Destination | When |
| --- | --- | --- |
| App update check and download | `github.com` release assets, verified by the update signature | 15 s after launch, then every 4 h, if "Check for updates" is on in Settings. Also when you open "Check for Updates..." from the menu bar icon, and when you install an update. |
| Speech and vocabulary models | `huggingface.co` (file list and files), pinned revisions, verified by SHA-256 | When model files are missing, for example at first launch, or a newly downloaded file failed verification. When you save vocabulary words and the vocabulary model is not ready. When you confirm a re-download in Settings. |
| Cleanup runtime packages | PyPI (`pypi.org`, `files.pythonhosted.org`), hash-locked wheels only | At launch with AI cleanup on, or when you turn cleanup on, if the runtime is missing or differs from the bundled lock file. Each time you repair the runtime. |
| Cleanup model | `huggingface.co` (model information and files), pinned revision, verified by SHA-256 | When AI cleanup is on and the model is not downloaded: when you turn cleanup on, or at launch. |
| Standalone Python | `github.com` (python-build-standalone release), pinned URL, verified by SHA-256 | When SottoASR builds, updates or repairs the cleanup runtime and no native arm64 Python 3.11 to 3.14 is installed. |
| Dictation audio (AWS Transcribe only) | The Amazon Transcribe streaming endpoint of the selected Region (`transcribestreaming.<region>.amazonaws.com`), over TLS. The action is `transcribe:StartStreamTranscription`, signed with the credentials of your AWS profile. | Only when "AWS Transcribe" is selected in Settings → Dictation: after each completed dictation, when you recover a kept recording, and when you click "Test connection" (one second of silence). SottoASR sends a dictation after the recording ends, as 16 kHz 16-bit PCM. It does not send a cancelled recording. |
| Dictation text (AWS Bedrock only) | The Amazon Bedrock runtime endpoint of the selected Region (`bedrock-runtime.<region>.amazonaws.com`), over TLS. The action is `bedrock:InvokeModel` on an inference profile, signed with the credentials of your AWS profile. AWS processes the text in the geography of the inference profile, or in any commercial AWS Region for a global profile. Settings shows which. | Only when AI cleanup is on and "AWS Bedrock" is selected in Settings → Dictation: after each completed dictation, when you reprocess a kept recording, and when you click "Test connection" (one fixed sentence). SottoASR sends only the windows of the text that cleanup can change, of at most 100 words each. It sends nothing for a text with nothing to clean, a text in another language, or a cancelled or interrupted recording. It never sends audio, History, your vocabulary words or your replacement rules. |

These downloads follow HTTP redirects to the file servers of GitHub and
Hugging Face. The cleanup model runs in a separate process with the Hugging
Face offline mode on. Thus, in the default configuration, dictation makes no
network request.

SottoASR stores no AWS credentials. For both cloud engines, the AWS SDK for
Rust reads your AWS files (`~/.aws/config` and `~/.aws/credentials`) when it
signs a request. If your profile uses IAM Identity Center or assumes a role,
the SDK first gets temporary credentials from AWS, as the AWS CLI does. If
your profile has a `credential_process` command, the SDK runs that command
with your user rights. Logs name the AWS profile, the Region and the Amazon
Bedrock model, never a key or a token. The AWS Bedrock engine logs no
dictation text.

By default, AWS can keep the audio that Amazon Transcribe processes to improve
its services. An AWS Organizations opt-out policy stops that
([opt-out](https://docs.aws.amazon.com/transcribe/latest/dg/opt-out.html)).
AWS bills your account for each second of audio.

Amazon Bedrock does not give your text to the model provider
([data protection](https://docs.aws.amazon.com/bedrock/latest/userguide/data-protection.html)).
For GPT-6.2 Sol, AWS can keep text that its abuse classifier flags for up to
30 days, for automated abuse detection. The three Claude models are not in the
list of models with this retention
([abuse detection](https://docs.aws.amazon.com/bedrock/latest/userguide/abuse-detection.html),
[data retention](https://docs.aws.amazon.com/bedrock/latest/userguide/data-retention.html)).
AWS bills your account for the input and output tokens.

SottoASR accepts only deletions from the model in Amazon Bedrock. Its
validator examines each change that the model proposes, and SottoASR rebuilds
the delivered text from your original text. Thus a model that follows an
instruction in a dictation cannot add its answer to your text.

The app windows cannot connect to other hosts: their content security policy
allows only the app itself. Only the Rust code of the app connects to AWS. The
app opens links in your browser only for `https` addresses on
`huggingface.co`, `github.com`, `sottoasr.app`, `docs.aws.amazon.com` and
`aws.amazon.com`.

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
