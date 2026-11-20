//! Amazon Transcribe as a speech engine: the optional cloud mode.
//!
//! The engine holds no model. Each dictation is one `StartStreamTranscription`
//! request: the recorded WAV is read in pieces, resampled to 16 kHz PCM and
//! sent as 100 ms events, and the final results are joined to the text. The
//! AWS SDK resolves the credentials of the selected profile and signs the
//! request; SottoASR stores no credential. The measurements behind each
//! choice are in docs/specs/2026-11-20-aws-transcribe-cloud-mode.md.

use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::{Duration, Instant};

use aws_config::profile::ProfileFileCredentialsProvider;
use aws_config::provider_config::ProviderConfig;
use aws_config::retry::RetryConfig;
use aws_config::timeout::TimeoutConfig;
use aws_config::{BehaviorVersion, Region};
use aws_credential_types::provider::error::CredentialsError;
use aws_sdk_transcribestreaming::error::{DisplayErrorContext, ProvideErrorMetadata, SdkError};
use aws_sdk_transcribestreaming::primitives::Blob;
use aws_sdk_transcribestreaming::types::error::AudioStreamError;
use aws_sdk_transcribestreaming::types::{AudioEvent, AudioStream, LanguageCode, MediaEncoding, TranscriptResultStream};
use aws_sdk_transcribestreaming::Client;
use aws_smithy_http_client::tls::{rustls_provider::CryptoMode, Provider};

use super::aws_profiles::{self, AwsProfile};
use super::engine::{AsrEngine, AsrResult};
use crate::audio::resample::{Resampler16k, TARGET_RATE};
use crate::models::AwsTranscribeSettings;

pub const BACKEND_NAME: &str = "AWS Transcribe";

/// The Regions with an Amazon Transcribe streaming endpoint
/// (docs.aws.amazon.com/general/latest/gr/transcribe.html, 2026-11-20).
pub const REGIONS: [(&str, &str); 20] = [
    ("us-east-1", "US East (N. Virginia)"),
    ("us-east-2", "US East (Ohio)"),
    ("us-west-2", "US West (Oregon)"),
    ("af-south-1", "Africa (Cape Town)"),
    ("ap-northeast-1", "Asia Pacific (Tokyo)"),
    ("ap-northeast-2", "Asia Pacific (Seoul)"),
    ("ap-south-1", "Asia Pacific (Mumbai)"),
    ("ap-southeast-1", "Asia Pacific (Singapore)"),
    ("ap-southeast-2", "Asia Pacific (Sydney)"),
    ("ap-southeast-5", "Asia Pacific (Malaysia)"),
    ("ap-southeast-7", "Asia Pacific (Thailand)"),
    ("ca-central-1", "Canada (Central)"),
    ("eu-central-1", "Europe (Frankfurt)"),
    ("eu-central-2", "Europe (Zurich)"),
    ("eu-west-1", "Europe (Ireland)"),
    ("eu-west-2", "Europe (London)"),
    ("mx-central-1", "Mexico (Central)"),
    ("sa-east-1", "South America (São Paulo)"),
    ("us-gov-east-1", "AWS GovCloud (US-East)"),
    ("us-gov-west-1", "AWS GovCloud (US-West)"),
];

/// 100 ms of 16 kHz audio per event: inside the 50 to 200 ms that AWS
/// recommends. 50 ms events changed a word in the measurements.
const EVENT_SAMPLES: usize = 1_600;

/// The language codes that the streaming API of this SDK version accepts.
pub fn languages() -> &'static [&'static str] {
    LanguageCode::values()
}

/// The format rules of the AWS options. They do not depend on the engine
/// that is selected, and they need no file or network.
pub fn validate(settings: &AwsTranscribeSettings) -> Result<(), String> {
    if let Some(profile) = &settings.profile {
        if profile.chars().count() > 128 || profile.chars().any(char::is_control) {
            return Err("The AWS profile name is not valid.".into());
        }
    }
    if let Some(region) = &settings.region {
        if !REGIONS.iter().any(|(code, _)| code == region) {
            return Err("Choose a Region where Amazon Transcribe streaming is available.".into());
        }
    }
    if !languages().contains(&settings.language.as_str()) {
        return Err("Choose a language that Amazon Transcribe streaming supports.".into());
    }
    if let Some(name) = &settings.vocabulary_name {
        let allowed = |character: char| character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-');
        if name.is_empty() || name.len() > 200 || !name.chars().all(allowed) {
            return Err("The custom vocabulary name can have letters, digits, dots, hyphens and underscores only.".into());
        }
    }
    Ok(())
}

/// Where a request goes and what it asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    profile: String,
    region: String,
    language: String,
    vocabulary_name: Option<String>,
}

/// The selected profile must be in the AWS files. The Region is the one of
/// the settings, else the one of the profile.
fn resolve(settings: &AwsTranscribeSettings, profiles: &[AwsProfile]) -> Result<Target, String> {
    let profile = settings.profile.clone().ok_or("Choose an AWS profile for AWS Transcribe in Settings.")?;
    let found = profiles.iter().find(|candidate| candidate.name == profile)
        .ok_or_else(|| format!("AWS profile \"{profile}\" was not found in ~/.aws/config or ~/.aws/credentials."))?;
    let region = settings.region.clone().or_else(|| found.region.clone())
        .ok_or_else(|| format!("AWS profile \"{profile}\" has no Region. Choose a Region in Settings."))?;
    Ok(Target { profile, region, language: settings.language.clone(), vocabulary_name: settings.vocabulary_name.clone() })
}

/// What Settings says about the cloud engine: the Region and the profile in
/// use, or why no request can be made.
pub fn describe(settings: &AwsTranscribeSettings, profiles: &[AwsProfile]) -> Result<String, String> {
    let target = resolve(settings, profiles)?;
    Ok(format!("SottoASR sends each dictation to Amazon Transcribe in {} with the AWS profile {}.", target.region, target.profile))
}

/// The audio of one request.
enum Input {
    File(PathBuf),
    Samples { samples: Arc<Vec<f32>>, rate: u32 },
}

/// The next block of mono samples at the source rate. Empty at the end.
type ReadBlock = Box<dyn FnMut() -> Result<Vec<f32>, String> + Send>;

/// 16 kHz mono 16-bit PCM, made in pieces while the service accepts data, so
/// that a long recording is never in memory as a whole.
struct PcmSource {
    read: ReadBlock,
    resampler: Resampler16k,
    /// 16 kHz samples that are not sent yet.
    ready: Vec<f32>,
    finished: bool,
    duration_secs: f64,
}

impl Input {
    fn open(&self) -> Result<PcmSource, String> {
        let (read, rate, duration_secs): (ReadBlock, u32, f64) = match self {
            Input::File(path) => {
                let failed = |error: hound::Error| format!("Could not read the recording: {error}");
                let mut reader = hound::WavReader::open(path).map_err(failed)?;
                let spec = reader.spec();
                if spec.channels == 0 || spec.sample_rate == 0 {
                    return Err("Could not read the recording: it has no audio format.".into());
                }
                let channels = spec.channels as usize;
                let float = spec.sample_format == hound::SampleFormat::Float;
                let scale = 1.0 / (1_i64 << spec.bits_per_sample.clamp(1, 32).saturating_sub(1)) as f32;
                // About 100 ms of the recording per block.
                let count = (spec.sample_rate as usize / 10).max(1) * channels;
                let duration_secs = reader.duration() as f64 / spec.sample_rate as f64;
                let read = move || -> Result<Vec<f32>, String> {
                    let mut values = Vec::with_capacity(count);
                    if float {
                        for sample in reader.samples::<f32>().take(count) {
                            values.push(sample.map_err(failed)?);
                        }
                    } else {
                        for sample in reader.samples::<i32>().take(count) {
                            values.push(sample.map_err(failed)? as f32 * scale);
                        }
                    }
                    Ok(values.chunks_exact(channels).map(|frame| frame.iter().sum::<f32>() / channels as f32).collect())
                };
                (Box::new(read), spec.sample_rate, duration_secs)
            }
            Input::Samples { samples, rate } => {
                let (samples, rate) = (Arc::clone(samples), *rate);
                if rate == 0 {
                    return Err("The audio has no sample rate.".into());
                }
                let duration_secs = samples.len() as f64 / rate as f64;
                let block = (rate as usize / 10).max(1);
                let mut position = 0;
                let read = move || -> Result<Vec<f32>, String> {
                    let end = (position + block).min(samples.len());
                    let piece = samples[position..end].to_vec();
                    position = end;
                    Ok(piece)
                };
                (Box::new(read), rate, duration_secs)
            }
        };
        Ok(PcmSource {
            read,
            resampler: Resampler16k::new(rate)?,
            ready: Vec::with_capacity(2 * EVENT_SAMPLES),
            finished: false,
            duration_secs,
        })
    }
}

impl PcmSource {
    /// The next event of at most 100 ms, as little-endian 16-bit samples.
    /// `None` after the last one.
    fn next_event(&mut self) -> Result<Option<Vec<u8>>, String> {
        while self.ready.len() < EVENT_SAMPLES && !self.finished {
            let block = (self.read)()?;
            if block.is_empty() {
                self.resampler.finish(&mut self.ready)?;
                self.finished = true;
            } else {
                self.resampler.push(&block, &mut self.ready)?;
            }
        }
        if self.ready.is_empty() {
            return Ok(None);
        }
        let count = self.ready.len().min(EVENT_SAMPLES);
        Ok(Some(self.ready.drain(..count)
            .flat_map(|sample| ((sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16).to_le_bytes())
            .collect()))
    }
}

/// The audio events of a request. The SDK polls this only while the service
/// accepts data, so the file is read at the speed of the service. A read
/// error ends the audio and is left in `read_error`.
fn audio_events(
    source: PcmSource,
    read_error: Arc<Mutex<Option<String>>>,
) -> impl futures_util::Stream<Item = Result<AudioStream, AudioStreamError>> + Send + Sync + 'static {
    // The SDK requires `Sync`. Only this closure uses the source.
    let mut source = Mutex::new(source);
    futures_util::stream::poll_fn(move |_| {
        let source = source.get_mut().unwrap_or_else(|error| error.into_inner());
        Poll::Ready(match source.next_event() {
            Ok(Some(bytes)) => Some(Ok(AudioStream::AudioEvent(AudioEvent::builder().audio_chunk(Blob::new(bytes)).build()))),
            Ok(None) => None,
            Err(error) => {
                *read_error.lock().unwrap_or_else(|error| error.into_inner()) = Some(error);
                None
            }
        })
    })
}

/// Why a request gave no text, as a sentence for the user.
struct Failed {
    text: String,
    /// A connection problem before any result: the same request can go again.
    retry: bool,
}

/// One request: sends the audio and joins the final results.
async fn request(client: &Client, target: &Target, source: PcmSource) -> Result<String, Failed> {
    let read_error = Arc::new(Mutex::new(None));
    let mut call = client.start_stream_transcription()
        .language_code(LanguageCode::from(target.language.as_str()))
        .media_sample_rate_hertz(TARGET_RATE as i32)
        .media_encoding(MediaEncoding::Pcm)
        .audio_stream(audio_events(source, Arc::clone(&read_error)).into());
    if let Some(name) = &target.vocabulary_name {
        call = call.vocabulary_name(name);
    }
    let received = async {
        let mut output = call.send().await.map_err(|error| failed(&error, target, true))?;
        let mut parts: Vec<String> = Vec::new();
        loop {
            match output.transcript_result_stream.recv().await {
                Ok(Some(TranscriptResultStream::TranscriptEvent(event))) => {
                    let results = event.transcript.and_then(|transcript| transcript.results).unwrap_or_default();
                    for result in results.into_iter().filter(|result| !result.is_partial) {
                        let text = result.alternatives.unwrap_or_default().into_iter().next()
                            .and_then(|alternative| alternative.transcript)
                            .map(|text| text.trim().to_owned())
                            .filter(|text| !text.is_empty());
                        parts.extend(text);
                    }
                }
                Ok(Some(_)) => {}
                Ok(None) => return Ok(parts),
                Err(error) => return Err(failed(&error, target, false)),
            }
        }
    }.await;
    // A recording that could not be read ends the audio early. That is the
    // cause to report, whatever the service then answered.
    if let Some(error) = read_error.lock().unwrap_or_else(|error| error.into_inner()).take() {
        return Err(Failed { text: error, retry: false });
    }
    Ok(received?.join(separator(&target.language)))
}

/// Japanese and Chinese have no space between the parts of a text.
fn separator(language: &str) -> &'static str {
    if language.starts_with("ja") || language.starts_with("zh") { "" } else { " " }
}

/// 30 s plus half the audio, for one request. It ends before the deadline of
/// the pipeline (60 s plus half the audio), so a request that hangs returns
/// its own error and releases the engine. The service needs about a quarter
/// of the audio duration.
fn time_limit(duration_secs: f64) -> Duration {
    Duration::from_secs(30) + Duration::from_secs_f64(duration_secs.max(0.0) / 2.0)
}

fn failed<E, R>(error: &SdkError<E, R>, target: &Target, at_start: bool) -> Failed
where
    E: ProvideErrorMetadata + std::error::Error + 'static,
    R: std::fmt::Debug + 'static,
{
    if matches!(error, SdkError::ServiceError(_)) {
        return Failed { text: service_text(error.code(), error.message(), target), retry: false };
    }
    // The cause is the last error of the chain. A credential provider
    // error is somewhere in the chain of a dispatch failure.
    let mut cause: &(dyn std::error::Error + 'static) = error;
    let mut credentials = false;
    while let Some(source) = cause.source() {
        credentials |= source.is::<CredentialsError>();
        cause = source;
    }
    if credentials {
        return Failed { text: credentials_text(&cause.to_string(), target), retry: false };
    }
    log::warn!("AWS Transcribe request failed: {}", DisplayErrorContext(error));
    match error {
        SdkError::DispatchFailure(_) | SdkError::TimeoutError(_) => Failed {
            text: format!("Could not connect to Amazon Transcribe in {}. Examine your internet connection.", target.region),
            retry: at_start,
        },
        _ => Failed { text: format!("Amazon Transcribe had an error: {}", shortened(&cause.to_string())), retry: false },
    }
}

/// The first line, at most 300 characters: a cause can carry a whole
/// standard-error text or a list of every valid value.
fn shortened(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    match line.char_indices().nth(300) {
        Some((end, _)) => format!("{}…", &line[..end]),
        None => line.to_owned(),
    }
}

fn credentials_text(cause: &str, target: &Target) -> String {
    let mut text = format!("Could not get credentials for AWS profile \"{}\": {}", target.profile, shortened(cause));
    if !text.ends_with('.') {
        text.push('.');
    }
    // An application that Finder starts has only /usr/bin:/bin:/usr/sbin:/sbin,
    // so the shell does not find a command that is given without its path.
    if cause.contains("command not found") {
        text.push_str(" SottoASR runs the command without the PATH of your shell. Use the full path of the command in ~/.aws/config.");
    }
    text
}

fn service_text(code: Option<&str>, message: Option<&str>, target: &Target) -> String {
    let detail = shortened(message.or(code).unwrap_or("no detail"));
    match code.unwrap_or_default() {
        "UnrecognizedClientException" | "ExpiredTokenException" | "ExpiredToken" | "InvalidSignatureException"
        | "InvalidClientTokenId" | "IncompleteSignature" => {
            format!("The credentials of AWS profile \"{}\" are not valid or are expired. Sign in again.", target.profile)
        }
        "AccessDeniedException" | "AccessDenied" => format!(
            "AWS profile \"{}\" cannot use Amazon Transcribe. It needs the permission transcribe:StartStreamTranscription.",
            target.profile
        ),
        "NotAuthorizedException" => format!("Amazon Transcribe is not available to this account in {}.", target.region),
        "BadRequestException" => format!("Amazon Transcribe did not accept the request: {detail}"),
        "LimitExceededException" | "ThrottlingException" | "TooManyRequestsException" => {
            "Amazon Transcribe has too many requests for this account. Try again in a moment.".into()
        }
        _ => format!("Amazon Transcribe had an error: {detail}"),
    }
}

/// The client for one target. The credentials come from the selected profile
/// only. The HTTP client uses rustls with ring, the provider that the rest
/// of the application links; the credentials provider gets the same client,
/// because a provider without one cannot resolve an SSO or role profile.
async fn client(target: &Target) -> Client {
    let http = aws_smithy_http_client::Builder::new()
        .tls_provider(Provider::Rustls(CryptoMode::Ring))
        .build_https();
    let region = Region::new(target.region.clone());
    let provider_config = ProviderConfig::default()
        .with_http_client(http.clone())
        .with_region(Some(region.clone()));
    let credentials = ProfileFileCredentialsProvider::builder()
        .configure(&provider_config)
        .profile_name(&target.profile)
        .build();
    let config = aws_config::defaults(BehaviorVersion::latest())
        .http_client(http)
        .credentials_provider(credentials)
        .region(region)
        // An audio stream cannot be sent again by the SDK; `transcribe` does it.
        .retry_config(RetryConfig::disabled())
        .timeout_config(TimeoutConfig::builder().connect_timeout(Duration::from_secs(10)).build())
        .load()
        .await;
    Client::new(&config)
}

struct Session {
    target: Target,
    /// Kept after each request that succeeds: it caches the credentials
    /// until they expire and keeps the connection.
    client: Client,
}

/// The cloud speech engine. It never holds a model.
pub struct TranscribeEngine {
    settings: AwsTranscribeSettings,
    session: Option<Session>,
}

impl TranscribeEngine {
    pub fn new(settings: AwsTranscribeSettings) -> Self {
        Self { settings, session: None }
    }

    async fn transcribe(&mut self, input: Input) -> Result<AsrResult, String> {
        let started = Instant::now();
        // The session leaves the engine for the request and returns only
        // after a success. After a failure, the next request reads the AWS
        // files again: the user can sign in or repair them and try again.
        let session = match self.session.take() {
            Some(session) => session,
            None => {
                let target = resolve(&self.settings, &aws_profiles::list().await?)?;
                let client = client(&target).await;
                Session { target, client }
            }
        };
        let mut retried = false;
        let (text, duration_secs) = loop {
            let source = input.open()?;
            let duration_secs = source.duration_secs;
            let attempt = request(&session.client, &session.target, source);
            match tokio::time::timeout(time_limit(duration_secs), attempt).await {
                Ok(Ok(text)) => break (text, duration_secs),
                Ok(Err(failure)) if failure.retry && !retried => {
                    retried = true;
                    log::warn!("AWS Transcribe: no connection; sending the request once more");
                }
                Ok(Err(failure)) => return Err(failure.text),
                Err(_) => return Err("Amazon Transcribe did not answer in time.".into()),
            }
        };
        let processing_time_secs = started.elapsed().as_secs_f64();
        log::info!(
            "AWS Transcribe: {duration_secs:.1}s of audio in {processing_time_secs:.1}s ({}, profile {})",
            session.target.region, session.target.profile
        );
        self.session = Some(session);
        Ok(AsrResult {
            text,
            unboosted_text: None,
            duration_secs,
            processing_time_secs,
            rtfx: if processing_time_secs > 0.0 { (duration_secs / processing_time_secs) as f32 } else { 0.0 },
        })
    }
}

/// The engine is called on a blocking worker (`with_engine`), where the
/// handle of the application runtime can drive the request.
fn block_on<T>(request: impl Future<Output = Result<T, String>>) -> Result<T, String> {
    tokio::runtime::Handle::try_current()
        .map_err(|_| "AWS Transcribe needs the application runtime.".to_owned())?
        .block_on(request)
}

impl AsrEngine for TranscribeEngine {
    /// Nothing to load, and no request: the first dictation makes the client.
    fn init(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn is_ready(&self) -> bool {
        true
    }

    fn transcribe_file(&mut self, path: &str) -> Result<AsrResult, String> {
        block_on(self.transcribe(Input::File(path.into())))
    }

    fn transcribe_samples(&mut self, samples: &[f32], sample_rate: u32) -> Result<AsrResult, String> {
        block_on(self.transcribe(Input::Samples { samples: Arc::new(samples.to_vec()), rate: sample_rate }))
    }

    fn is_model_available(&self) -> bool {
        true
    }

    fn backend_name(&self) -> &'static str {
        BACKEND_NAME
    }
}

/// The result of a connection test.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ConnectionTest {
    pub region: String,
    pub seconds: f64,
}

/// Sends one second of silence with these options through a new client. The
/// service then accepted the credentials, the permission, the Region, the
/// language and the custom vocabulary. Silence gives no text.
pub async fn test_connection(settings: &AwsTranscribeSettings) -> Result<ConnectionTest, String> {
    let mut settings = settings.clone();
    settings.normalize();
    validate(&settings)?;
    let mut engine = TranscribeEngine::new(settings);
    let silence = Input::Samples { samples: Arc::new(vec![0.0; TARGET_RATE as usize]), rate: TARGET_RATE };
    let result = engine.transcribe(silence).await?;
    let region = engine.session.map(|session| session.target.region).unwrap_or_default();
    Ok(ConnectionTest { region, seconds: result.processing_time_secs })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(profile: Option<&str>, region: Option<&str>) -> AwsTranscribeSettings {
        AwsTranscribeSettings {
            profile: profile.map(str::to_owned),
            region: region.map(str::to_owned),
            ..Default::default()
        }
    }

    fn target() -> Target {
        Target { profile: "work".into(), region: "eu-west-1".into(), language: "en-US".into(), vocabulary_name: None }
    }

    #[test]
    fn the_format_rules_accept_the_defaults_and_name_each_bad_value() {
        assert_eq!(validate(&AwsTranscribeSettings::default()), Ok(()));
        assert_eq!(validate(&settings(Some("work"), Some("us-west-2"))), Ok(()));
        let message = |settings: &AwsTranscribeSettings| validate(settings).unwrap_err();
        assert!(message(&settings(Some("bad\nname"), None)).contains("profile name"));
        assert!(message(&settings(Some(&"p".repeat(129)), None)).contains("profile name"));
        // A Region that exists but has no streaming endpoint.
        assert!(message(&settings(None, Some("eu-north-1"))).contains("Region"));
        let language = AwsTranscribeSettings { language: "xx-YY".into(), ..Default::default() };
        assert!(message(&language).contains("language"));
        for name in ["with space", "slash/name", ""] {
            let vocabulary = AwsTranscribeSettings { vocabulary_name: Some(name.into()), ..Default::default() };
            assert!(message(&vocabulary).contains("custom vocabulary"), "{name:?}");
        }
        let vocabulary = AwsTranscribeSettings { vocabulary_name: Some("team.words_2-b".into()), ..Default::default() };
        assert_eq!(validate(&vocabulary), Ok(()));
        assert!(languages().contains(&"en-US") && languages().len() > 50);
    }

    #[test]
    fn the_region_of_the_settings_wins_and_the_profile_must_exist() {
        let profiles = [
            AwsProfile { name: "work".into(), region: Some("eu-west-1".into()) },
            AwsProfile { name: "bare".into(), region: None },
        ];
        assert_eq!(resolve(&settings(Some("work"), None), &profiles).unwrap().region, "eu-west-1");
        assert_eq!(resolve(&settings(Some("work"), Some("us-east-2")), &profiles).unwrap().region, "us-east-2");
        assert_eq!(resolve(&settings(Some("bare"), Some("us-east-2")), &profiles).unwrap().region, "us-east-2");
        assert!(resolve(&settings(Some("bare"), None), &profiles).unwrap_err().contains("has no Region"));
        assert!(resolve(&settings(Some("gone"), Some("us-east-2")), &profiles).unwrap_err().contains("was not found"));
        assert!(resolve(&settings(None, Some("us-east-2")), &profiles).unwrap_err().contains("Choose an AWS profile"));
    }

    /// Every event of a source, and the count of 16-bit samples in them.
    fn all_events(input: &Input) -> (Vec<Vec<u8>>, usize) {
        let mut source = input.open().unwrap();
        let mut events = Vec::new();
        while let Some(event) = source.next_event().unwrap() {
            events.push(event);
        }
        assert!(source.next_event().unwrap().is_none(), "the end stays the end");
        let samples = events.iter().map(|event| event.len() / 2).sum();
        (events, samples)
    }

    fn write_wav(path: &std::path::Path, spec: hound::WavSpec, frames: usize) {
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for index in 0..frames * spec.channels as usize {
            let value = ((index % 200) as f32 / 100.0 - 1.0) * 0.5;
            match spec.sample_format {
                hound::SampleFormat::Float => writer.write_sample(value).unwrap(),
                hound::SampleFormat::Int => writer.write_sample((value * f32::from(i16::MAX)) as i16).unwrap(),
            }
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn a_recording_becomes_100_ms_events_of_16_khz_16_bit_mono() {
        let directory = tempfile::tempdir().unwrap();
        // The format of a SottoASR recording: mono 32-bit float at the device rate.
        let recording = directory.path().join("recording.wav");
        let float = hound::WavSpec { channels: 1, sample_rate: 48_000, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
        write_wav(&recording, float, 48_000 + 4_800 / 2);
        let input = Input::File(recording);
        assert!((input.open().unwrap().duration_secs - 1.05).abs() < 1e-9);
        let (events, samples) = all_events(&input);
        assert_eq!(samples, 16_800, "1.05 s at 16 kHz");
        assert_eq!(events.len(), 11);
        assert!(events[..10].iter().all(|event| event.len() == 2 * EVENT_SAMPLES));
        assert_eq!(events[10].len(), 2 * 800);

        // Another program's file: 16-bit stereo at 44.1 kHz.
        let stereo = directory.path().join("stereo.wav");
        let int = hound::WavSpec { channels: 2, sample_rate: 44_100, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        write_wav(&stereo, int, 44_100);
        assert_eq!(all_events(&Input::File(stereo)).1, 16_000);

        assert!(Input::File(directory.path().join("missing.wav")).open().err().unwrap().contains("Could not read the recording"));
    }

    #[test]
    fn samples_in_memory_keep_their_values_and_silence_stays_silence() {
        let tone: Vec<f32> = (0..16_000).map(|index| if index % 2 == 0 { 0.5 } else { -0.5 }).collect();
        let (events, samples) = all_events(&Input::Samples { samples: Arc::new(tone), rate: TARGET_RATE });
        assert_eq!(samples, 16_000);
        assert_eq!(&events[0][..4], &[0xFF, 0x3F, 0x01, 0xC0], "0.5 and -0.5 as little-endian 16-bit");

        // The audio of a connection test.
        let (events, samples) = all_events(&Input::Samples { samples: Arc::new(vec![0.0; TARGET_RATE as usize]), rate: TARGET_RATE });
        assert_eq!(samples, 16_000);
        assert!(events.iter().flatten().all(|byte| *byte == 0));

        let (events, samples) = all_events(&Input::Samples { samples: Arc::new(Vec::new()), rate: 48_000 });
        assert!(events.is_empty() && samples == 0);
    }

    #[test]
    fn each_service_error_has_its_own_sentence() {
        let target = target();
        let text = |code: &str, message: &str| service_text(Some(code), Some(message), &target);
        assert_eq!(
            text("AccessDeniedException", "User: arn:aws:sts::1:assumed-role/x is not authorized to perform: transcribe:StartStreamTranscription"),
            "AWS profile \"work\" cannot use Amazon Transcribe. It needs the permission transcribe:StartStreamTranscription."
        );
        for code in ["UnrecognizedClientException", "ExpiredTokenException", "InvalidSignatureException"] {
            assert_eq!(text(code, "The security token included in the request is invalid"),
                "The credentials of AWS profile \"work\" are not valid or are expired. Sign in again.");
        }
        assert_eq!(text("NotAuthorizedException", "Your account isn't authorized to call this operation."),
            "Amazon Transcribe is not available to this account in eu-west-1.");
        assert_eq!(text("BadRequestException", "The specified vocabulary doesn't exist. Check the name and try your request again."),
            "Amazon Transcribe did not accept the request: The specified vocabulary doesn't exist. Check the name and try your request again.");
        assert!(text("LimitExceededException", "Too many").contains("too many requests"));
        assert_eq!(text("InternalFailureException", "boom"), "Amazon Transcribe had an error: boom");
        assert_eq!(service_text(None, None, &target), "Amazon Transcribe had an error: no detail");
        // The list of every valid language is longer than a message can be.
        let long = text("BadRequestException", &"x".repeat(2_000));
        assert!(long.chars().count() < 360 && long.ends_with('…'), "{}", long.chars().count());
    }

    #[test]
    fn a_credential_error_names_the_profile_and_explains_a_command_that_was_not_found() {
        let target = target();
        let missing = credentials_text(
            "Error retrieving credentials: external process exited with code exit status: 127. Stderr: sh: aws-vault: command not found\nmore", &target);
        assert!(missing.starts_with("Could not get credentials for AWS profile \"work\": Error retrieving credentials"), "{missing}");
        assert!(missing.contains("command not found") && !missing.contains("more"), "{missing}");
        assert!(missing.ends_with("Use the full path of the command in ~/.aws/config."), "{missing}");

        // A full path that does not exist is not a PATH problem.
        let absent = credentials_text(
            "Error retrieving credentials: external process exited with code exit status: 127. Stderr: sh: /opt/tool: No such file or directory", &target);
        assert!(absent.ends_with("No such file or directory."), "{absent}");

        let expired = credentials_text("the SSO token has expired", &target);
        assert_eq!(expired, "Could not get credentials for AWS profile \"work\": the SSO token has expired.");
    }

    #[test]
    fn an_sdk_error_becomes_the_sentence_of_its_cause() {
        use aws_sdk_transcribestreaming::error::{ConnectorError, ErrorMetadata};
        use aws_sdk_transcribestreaming::operation::start_stream_transcription::StartStreamTranscriptionError;
        type Error = SdkError<StartStreamTranscriptionError, ()>;
        let target = target();

        let metadata = ErrorMetadata::builder().code("AccessDeniedException").message("User is not authorized").build();
        let denied = failed(&Error::service_error(StartStreamTranscriptionError::generic(metadata), ()), &target, true);
        assert!(denied.text.contains("transcribe:StartStreamTranscription"), "{}", denied.text);
        assert!(!denied.retry);

        // A credential provider error is inside a dispatch failure. It is
        // not a connection problem, and a second request cannot repair it.
        let provider = CredentialsError::provider_error("external process exited with code exit status: 1. Stderr: sign in first");
        let credentials = failed(&Error::dispatch_failure(ConnectorError::other(provider.into(), None)), &target, true);
        assert_eq!(
            credentials.text,
            "Could not get credentials for AWS profile \"work\": external process exited with code exit status: 1. Stderr: sign in first."
        );
        assert!(!credentials.retry);

        let refused = || Error::dispatch_failure(ConnectorError::io("connection refused".into()));
        let at_start = failed(&refused(), &target, true);
        assert_eq!(at_start.text, "Could not connect to Amazon Transcribe in eu-west-1. Examine your internet connection.");
        assert!(at_start.retry, "no result arrived yet: the request can go again");
        assert!(!failed(&refused(), &target, false).retry, "results arrived: no second request");

        let timeout = failed(&Error::timeout_error("the connection took too long"), &target, true);
        assert!(timeout.text.starts_with("Could not connect to Amazon Transcribe") && timeout.retry);
    }

    #[test]
    fn results_join_with_a_space_except_in_japanese_and_chinese() {
        assert_eq!(separator("en-US"), " ");
        assert_eq!(separator("de-CH"), " ");
        assert_eq!(separator("ja-JP"), "");
        assert_eq!(separator("zh-TW"), "");
    }

    #[test]
    fn the_request_ends_before_the_deadline_of_the_pipeline() {
        assert_eq!(time_limit(0.0), Duration::from_secs(30));
        // The longest recording: 20 minutes.
        assert_eq!(time_limit(1_200.0), Duration::from_secs(630));
        assert!(time_limit(1_200.0) < Duration::from_secs(60) + Duration::from_secs(600));
    }

    #[test]
    fn the_engine_needs_no_model_and_reports_a_missing_runtime() {
        let mut engine = TranscribeEngine::new(settings(Some("work"), Some("us-west-2")));
        assert_eq!(engine.init(), Ok(()));
        assert_eq!(engine.reload(), Ok(()));
        assert!(engine.is_ready() && engine.is_model_available());
        assert_eq!(engine.backend_name(), "AWS Transcribe");
        // Outside the application runtime nothing can be sent.
        assert!(engine.transcribe_file("/nonexistent.wav").unwrap_err().contains("runtime"));
    }
}
