//! Amazon Bedrock as a cleanup engine: the optional cloud mode of AI cleanup.
//!
//! The backend starts no process and holds no model. Each cleanup window is
//! one Converse request to an inference profile, with the prompt that the
//! sidecar pins. The planner and the frozen validator do not know which
//! backend made a proposal: the delivered text is rebuilt from the source
//! bytes, so a cloud model can only cause deletions that the validator
//! authorizes. The AWS SDK resolves the credentials of the selected profile;
//! SottoASR stores no credential. The measurements behind each choice are in
//! docs/specs/2026-11-20-bedrock-cleanup.md.

use std::collections::HashMap;
use std::future::Future;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use aws_config::retry::RetryConfig;
use aws_sdk_bedrockruntime::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_bedrockruntime::types::{
    ContentBlock, ConversationRole, ConverseOutput, InferenceConfiguration, Message, StopReason, SystemContentBlock,
};
use aws_sdk_bedrockruntime::Client;
use aws_smithy_types::Document;

use super::engine::{BatchItem, LlmBackend};
use crate::aws::profiles::{self as aws_profiles, AwsProfile};
use crate::aws::{self, Cause};
use crate::models::{AwsBedrockSettings, BedrockModel, CleanupMode};

pub const BACKEND_NAME: &str = "AWS Bedrock";

/// The Regions where `ListInferenceProfiles` returned a profile for each of
/// the four models (2026-11-20).
pub const REGIONS: [(&str, &str); 17] = [
    ("us-east-1", "US East (N. Virginia)"),
    ("us-east-2", "US East (Ohio)"),
    ("us-west-1", "US West (N. California)"),
    ("us-west-2", "US West (Oregon)"),
    ("ap-northeast-1", "Asia Pacific (Tokyo)"),
    ("ap-northeast-2", "Asia Pacific (Seoul)"),
    ("ap-northeast-3", "Asia Pacific (Osaka)"),
    ("ap-south-1", "Asia Pacific (Mumbai)"),
    ("ap-southeast-1", "Asia Pacific (Singapore)"),
    ("ap-southeast-2", "Asia Pacific (Sydney)"),
    ("ca-central-1", "Canada (Central)"),
    ("eu-central-1", "Europe (Frankfurt)"),
    ("eu-north-1", "Europe (Stockholm)"),
    ("eu-west-1", "Europe (Ireland)"),
    ("eu-west-2", "Europe (London)"),
    ("eu-west-3", "Europe (Paris)"),
    ("sa-east-1", "South America (São Paulo)"),
];

impl BedrockModel {
    pub const ALL: [BedrockModel; 4] = [Self::Haiku55, Self::Sonnet55, Self::Opus55, Self::Gpt62Sol];

    pub fn name(self) -> &'static str {
        match self {
            Self::Haiku55 => "Claude Haiku 5.5",
            Self::Sonnet55 => "Claude Sonnet 5.5",
            Self::Opus55 => "Claude Opus 5.5",
            Self::Gpt62Sol => "GPT-6.2 Sol",
        }
    }

    /// The foundation model identifier. A request names an inference
    /// profile of it: these models have no on-demand access by this name.
    fn model_id(self) -> &'static str {
        match self {
            Self::Haiku55 => "anthropic.claude-haiku-5-5",
            Self::Sonnet55 => "anthropic.claude-sonnet-5-5",
            Self::Opus55 => "anthropic.claude-opus-5-5",
            Self::Gpt62Sol => "openai.gpt-6.2-sol",
        }
    }

    fn is_claude(self) -> bool {
        !matches!(self, Self::Gpt62Sol)
    }
}

/// An inference profile that keeps a request inside one geography.
struct Geography {
    prefix: &'static str,
    /// As in "AWS processes the text in ...".
    place: &'static str,
    regions: &'static [&'static str],
    models: &'static [BedrockModel],
}

const CLAUDE: &[BedrockModel] = &[BedrockModel::Haiku55, BedrockModel::Sonnet55, BedrockModel::Opus55];

/// Only the rows that `ListInferenceProfiles` returned (2026-11-20). Each
/// other Region and model uses the global profile.
const GEOGRAPHIES: [Geography; 5] = [
    Geography { prefix: "us", place: "the United States", regions: &["us-east-1", "us-east-2", "us-west-1", "us-west-2"], models: &BedrockModel::ALL },
    Geography { prefix: "ca", place: "Canada", regions: &["ca-central-1"], models: &[BedrockModel::Haiku55, BedrockModel::Sonnet55] },
    Geography { prefix: "eu", place: "the European Union", regions: &["eu-central-1", "eu-north-1", "eu-west-1", "eu-west-2", "eu-west-3"], models: CLAUDE },
    Geography { prefix: "jp", place: "Japan", regions: &["ap-northeast-1", "ap-northeast-3"], models: &[BedrockModel::Haiku55, BedrockModel::Opus55] },
    Geography { prefix: "au", place: "Australia", regions: &["ap-southeast-2"], models: CLAUDE },
];

/// Which inference profile a request names, and where AWS processes it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Route {
    pub model: BedrockModel,
    pub inference_profile: String,
    /// A sentence for the user.
    pub processed_in: String,
}

/// The profile of the geography of `region` if AWS has one for `model`,
/// else the global profile.
pub fn route(region: &str, model: BedrockModel) -> Route {
    let geography = GEOGRAPHIES.iter()
        .find(|geography| geography.regions.contains(&region) && geography.models.contains(&model));
    let (prefix, processed_in) = match geography {
        Some(geography) => (geography.prefix, format!("AWS processes the text in {}.", geography.place)),
        None => ("global", "AWS can process the text in any commercial AWS Region.".to_owned()),
    };
    Route { model, inference_profile: format!("{prefix}.{}", model.model_id()), processed_in }
}

/// The format rules of the Bedrock options. They do not depend on the
/// engine that is selected, and they need no file or network.
pub fn validate(settings: &AwsBedrockSettings) -> Result<(), String> {
    if let Some(profile) = &settings.profile {
        if profile.chars().count() > 128 || profile.chars().any(char::is_control) {
            return Err("The AWS profile name is not valid.".into());
        }
    }
    if let Some(region) = &settings.region {
        if !REGIONS.iter().any(|(code, _)| code == region) {
            return Err("Choose a Region where Amazon Bedrock has the cleanup models.".into());
        }
    }
    Ok(())
}

/// Where a request goes and which model it asks.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    profile: String,
    region: String,
    model: BedrockModel,
    inference_profile: String,
}

fn resolve(settings: &AwsBedrockSettings, profiles: &[AwsProfile]) -> Result<Target, String> {
    let (profile, region) = aws::resolve(settings.profile.as_deref(), settings.region.as_deref(), profiles, BACKEND_NAME)?;
    // The Region of a profile can be one that the list does not have.
    if !REGIONS.iter().any(|(code, _)| *code == region) {
        return Err(format!("Amazon Bedrock has no cleanup model in {region}. Choose a Region in Settings."));
    }
    let inference_profile = route(&region, settings.model).inference_profile;
    Ok(Target { profile, region, model: settings.model, inference_profile })
}

/// What Settings says about the cloud engine: where the text goes, or why
/// no request can be made.
pub fn describe(settings: &AwsBedrockSettings, profiles: &[AwsProfile]) -> Result<String, String> {
    let target = resolve(settings, profiles)?;
    Ok(format!(
        "SottoASR sends dictation text to Amazon Bedrock in {} with the AWS profile {}. The model is {}.",
        target.region, target.profile, target.model.name()
    ))
}

#[derive(serde::Deserialize)]
struct Example {
    raw: String,
    cleaned: String,
}

/// One mode of the prompt pair that the sidecar pins (`PROMPTS` in
/// `sidecar/llm_cleanup.py`). A test holds this copy to the same hash.
#[derive(serde::Deserialize)]
struct Prompt {
    system: String,
    fewshot: Vec<Example>,
    user_prefix: String,
    user_suffix: String,
}

#[derive(serde::Deserialize)]
struct Prompts {
    retype: Prompt,
    replace: Prompt,
}

impl Prompt {
    fn user(&self, text: &str) -> String {
        format!("{}{text}{}", self.user_prefix, self.user_suffix)
    }

    /// The system text with the examples, as `build_prompt` of the sidecar
    /// makes it.
    fn system_with_examples(&self) -> String {
        let mut text = format!("{}\n\n<examples>", self.system);
        for example in &self.fewshot {
            text.push_str("\n<example>\nInput:\n");
            text.push_str(&self.user(&example.raw));
            text.push_str("\nOutput:\n");
            text.push_str(&example.cleaned);
            text.push_str("\n</example>");
        }
        text.push_str("\n</examples>");
        text
    }
}

/// The prompt pair and the system text of each mode, made once.
static PROMPTS: LazyLock<(Prompts, String, String)> = LazyLock::new(|| {
    let prompts: Prompts = serde_json::from_str(include_str!("cleanup-prompts.json"))
        .expect("cleanup-prompts.json is a file of this crate");
    let (retype, replace) = (prompts.retype.system_with_examples(), prompts.replace.system_with_examples());
    (prompts, retype, replace)
});

/// The prompt of a mode and its system text.
fn prompt(mode: CleanupMode) -> (&'static Prompt, &'static str) {
    let (prompts, retype, replace) = &*PROMPTS;
    match mode {
        CleanupMode::Retype => (&prompts.retype, retype),
        CleanupMode::Replace => (&prompts.replace, replace),
    }
}

/// The limit of one request. It is the generation limit of a local batch, so
/// a request ends before the deadline that the planner gives a batch.
const REQUEST_LIMIT: Duration = Duration::from_secs(10);

/// More than a window of 4,096 bytes can need. An answer that stops here is
/// not complete.
const MAX_OUTPUT_TOKENS: i32 = 4_096;

/// A proposal cannot be longer than the validator accepts.
const MAX_PROPOSAL_BYTES: usize = 32_000;

/// The answer to one window, or the reason why the whole batch fails.
#[derive(Debug, PartialEq, Eq)]
enum Answer {
    Item(BatchItem),
    /// AWS refused this request only, for example for too many requests.
    /// The sentence is for the user. The other windows keep their answers.
    Refused(String),
    /// A sentence for the user. Each other window would fail in the same way.
    Batch(String),
}

/// The items of a batch. A window that AWS refused stays as it was.
fn items(answers: Vec<Answer>) -> Result<Vec<BatchItem>, String> {
    answers.into_iter()
        .map(|answer| match answer {
            Answer::Item(item) => Ok(item),
            Answer::Refused(text) => Ok(BatchItem::Failed(text)),
            Answer::Batch(text) => Err(text),
        })
        .collect()
}

/// The proposal in a Converse answer. Reasoning blocks are not part of it.
fn proposal(stop: &StopReason, blocks: &[ContentBlock]) -> BatchItem {
    if matches!(stop, StopReason::MaxTokens | StopReason::ModelContextWindowExceeded) {
        return BatchItem::Failed("incomplete_generation".into());
    }
    let mut texts = blocks.iter().filter_map(|block| block.as_text().ok()).peekable();
    if !matches!(stop, StopReason::EndTurn | StopReason::StopSequence) || texts.peek().is_none() {
        return BatchItem::Failed("no_text".into());
    }
    let text: String = texts.map(String::as_str).collect();
    if text.len() > MAX_PROPOSAL_BYTES {
        return BatchItem::Failed("proposal_limit".into());
    }
    BatchItem::Proposal(Some(text.trim_end_matches(['\n', '\r']).to_owned()))
}

fn failed<E, R>(error: &SdkError<E, R>, target: &Target) -> Answer
where
    E: ProvideErrorMetadata + std::error::Error + 'static,
    R: std::fmt::Debug + 'static,
{
    match aws::cause(error) {
        Cause::Service { code, message } => service_answer(code.as_deref(), message.as_deref(), target),
        Cause::Credentials(cause) => Answer::Batch(aws::credentials_text(&cause, &target.profile)),
        Cause::Connection => {
            log::warn!("AWS Bedrock request failed: {}", aws::causes(error));
            Answer::Batch(format!("Could not connect to Amazon Bedrock in {}. Examine your internet connection.", target.region))
        }
        Cause::Other(cause) => {
            log::warn!("AWS Bedrock request failed: {}", aws::causes(error));
            Answer::Batch(format!("Amazon Bedrock had an error: {}", aws::shortened(&cause)))
        }
    }
}

fn service_answer(code: Option<&str>, message: Option<&str>, target: &Target) -> Answer {
    let detail = aws::shortened(message.or(code).unwrap_or("no detail"));
    let code = code.unwrap_or_default();
    if let Some(text) = aws::rejected_credentials_text(code, &target.profile) {
        return Answer::Batch(text);
    }
    let model = target.model.name();
    Answer::Batch(match code {
        // The model gave up on this window. The other windows can still answer.
        "ModelTimeoutException" => return Answer::Item(BatchItem::TimedOut),
        "AccessDeniedException" | "AccessDenied" => format!(
            "AWS profile \"{}\" cannot use {model} in Amazon Bedrock. It needs the permission bedrock:InvokeModel and access to the model.",
            target.profile
        ),
        "ResourceNotFoundException" => format!("Amazon Bedrock does not have {model} in {} for this account.", target.region),
        "ValidationException" => format!("Amazon Bedrock did not accept the request: {detail}"),
        "ThrottlingException" | "ServiceQuotaExceededException" | "TooManyRequestsException" => {
            return Answer::Refused("Amazon Bedrock has too many requests for this account. Try again in a moment.".into())
        }
        // An error of the model or of the service for this request.
        _ => return Answer::Refused(format!("Amazon Bedrock had an error: {detail}")),
    })
}

/// The cloud cleanup backend. It never holds a model.
pub struct BedrockCleanup {
    /// The options that it was made for. A change of the settings makes a
    /// new backend.
    options: AwsBedrockSettings,
    target: Target,
    /// It caches the credentials until they expire and keeps the connection.
    client: Client,
}

impl BedrockCleanup {
    /// Reads the AWS files and makes the client. No request goes to AWS.
    pub async fn connect(options: &AwsBedrockSettings) -> Result<Self, String> {
        let target = resolve(options, &aws_profiles::list().await?)?;
        // The SDK sends a request that AWS refused for a moment one more time.
        let config = aws::sdk_config(&target.profile, &target.region, RetryConfig::standard().with_max_attempts(2)).await;
        Ok(Self { options: options.clone(), target, client: Client::new(&config) })
    }

    async fn request(&self, text: &str, mode: CleanupMode) -> Answer {
        let (prompt, system) = prompt(mode);
        let message = match Message::builder().role(ConversationRole::User).content(ContentBlock::Text(prompt.user(text))).build() {
            Ok(message) => message,
            Err(error) => return Answer::Batch(format!("Amazon Bedrock had an error: {error}")),
        };
        let mut call = self.client.converse()
            .model_id(&self.target.inference_profile)
            .system(SystemContentBlock::Text(system.to_owned()))
            .messages(message)
            // No temperature: the four models reject the field.
            .inference_config(InferenceConfiguration::builder().max_tokens(MAX_OUTPUT_TOKENS).build());
        if self.target.model.is_claude() {
            // Cleanup needs no reasoning, and reasoning adds time.
            let effort = Document::Object(HashMap::from([("effort".to_owned(), Document::String("low".to_owned()))]));
            call = call.additional_model_request_fields(Document::Object(HashMap::from([("output_config".to_owned(), effort)])));
        }
        match tokio::time::timeout(REQUEST_LIMIT, call.send()).await {
            Err(_) => Answer::Item(BatchItem::TimedOut),
            Ok(Err(error)) => failed(&error, &self.target),
            Ok(Ok(output)) => Answer::Item(match &output.output {
                Some(ConverseOutput::Message(message)) => proposal(&output.stop_reason, &message.content),
                _ => BatchItem::Failed("no_text".into()),
            }),
        }
    }

    /// One request for each text, all at the same time.
    async fn request_batch(&self, texts: &[String], mode: CleanupMode) -> Result<Vec<BatchItem>, String> {
        let started = Instant::now();
        let answers = futures_util::future::join_all(texts.iter().map(|text| self.request(text, mode))).await;
        let items = items(answers)?;
        log::info!(
            "AWS Bedrock cleanup: {} window(s) in {:.1}s ({}, {}, profile {})",
            items.len(), started.elapsed().as_secs_f64(), self.target.inference_profile, self.target.region, self.target.profile
        );
        Ok(items)
    }
}

/// A backend is called on a blocking worker, where the handle of the
/// application runtime can drive the requests.
fn block_on<T>(requests: impl Future<Output = Result<T, String>>) -> Result<T, String> {
    tokio::runtime::Handle::try_current()
        .map_err(|_| "AWS Bedrock needs the application runtime.".to_owned())?
        .block_on(requests)
}

impl LlmBackend for BedrockCleanup {
    fn cleanup_batch(&mut self, texts: &[String], mode: CleanupMode) -> Result<Vec<BatchItem>, String> {
        block_on(self.request_batch(texts, mode))
    }

    /// Only the load of the local model sends a raw request.
    fn request_raw(&mut self, _: &serde_json::Value) -> Result<serde_json::Value, String> {
        Err("AWS Bedrock has no local model to load.".into())
    }

    fn bedrock_options(&self) -> Option<&AwsBedrockSettings> {
        Some(&self.options)
    }
}

/// The result of a connection test.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ConnectionTest {
    pub region: String,
    /// The display name of the model.
    pub model: &'static str,
    pub inference_profile: String,
    pub seconds: f64,
}

/// A fixed sentence, not dictation text.
const TEST_SENTENCE: &str = "Please um keep this readiness check short.";

/// Sends the fixed sentence with these options through a new backend. The
/// service then accepted the credentials, the permission, the Region and
/// the model, and the model returned a proposal.
pub async fn test_connection(settings: &AwsBedrockSettings) -> Result<ConnectionTest, String> {
    let mut settings = settings.clone();
    settings.normalize();
    validate(&settings)?;
    let backend = BedrockCleanup::connect(&settings).await?;
    let started = Instant::now();
    match backend.request(TEST_SENTENCE, CleanupMode::Retype).await {
        Answer::Item(BatchItem::Proposal(Some(_))) => Ok(ConnectionTest {
            region: backend.target.region,
            model: backend.target.model.name(),
            inference_profile: backend.target.inference_profile,
            seconds: started.elapsed().as_secs_f64(),
        }),
        Answer::Item(BatchItem::TimedOut) => Err("Amazon Bedrock did not answer in time.".into()),
        Answer::Item(_) => Err("Amazon Bedrock answered without a usable text.".into()),
        Answer::Refused(text) | Answer::Batch(text) => Err(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::engine::{is_zombie_error, PROMPT_SHA256};

    fn options(profile: Option<&str>, region: Option<&str>, model: BedrockModel) -> AwsBedrockSettings {
        AwsBedrockSettings { profile: profile.map(str::to_owned), region: region.map(str::to_owned), model }
    }

    fn target() -> Target {
        Target { profile: "work".into(), region: "eu-west-1".into(), model: BedrockModel::Sonnet55, inference_profile: "eu.anthropic.claude-sonnet-5-5".into() }
    }

    #[test]
    fn the_region_selects_the_geography_and_the_rest_is_global() {
        let profile = |region, model| route(region, model).inference_profile;
        assert_eq!(profile("us-west-1", BedrockModel::Gpt62Sol), "us.openai.gpt-6.2-sol");
        assert_eq!(profile("us-east-2", BedrockModel::Opus55), "us.anthropic.claude-opus-5-5");
        assert_eq!(profile("ca-central-1", BedrockModel::Haiku55), "ca.anthropic.claude-haiku-5-5");
        // Canada has no profile for these two. They do not go to the United States without a statement.
        assert_eq!(profile("ca-central-1", BedrockModel::Opus55), "global.anthropic.claude-opus-5-5");
        assert_eq!(profile("ca-central-1", BedrockModel::Gpt62Sol), "global.openai.gpt-6.2-sol");
        assert_eq!(profile("eu-west-2", BedrockModel::Sonnet55), "eu.anthropic.claude-sonnet-5-5");
        assert_eq!(profile("eu-north-1", BedrockModel::Gpt62Sol), "global.openai.gpt-6.2-sol");
        assert_eq!(profile("ap-northeast-3", BedrockModel::Opus55), "jp.anthropic.claude-opus-5-5");
        assert_eq!(profile("ap-northeast-1", BedrockModel::Sonnet55), "global.anthropic.claude-sonnet-5-5");
        assert_eq!(profile("ap-southeast-2", BedrockModel::Haiku55), "au.anthropic.claude-haiku-5-5");
        assert_eq!(profile("sa-east-1", BedrockModel::Haiku55), "global.anthropic.claude-haiku-5-5");

        assert_eq!(route("eu-west-1", BedrockModel::Haiku55).processed_in, "AWS processes the text in the European Union.");
        assert_eq!(route("ap-south-1", BedrockModel::Haiku55).processed_in, "AWS can process the text in any commercial AWS Region.");
        // Each Region of a geography is a Region that Settings offers.
        for geography in &GEOGRAPHIES {
            for region in geography.regions {
                assert!(REGIONS.iter().any(|(code, _)| code == region), "{region}");
            }
        }
    }

    #[test]
    fn the_format_rules_accept_the_defaults_and_name_each_bad_value() {
        assert_eq!(validate(&AwsBedrockSettings::default()), Ok(()));
        assert_eq!(validate(&options(Some("work"), Some("ap-south-1"), BedrockModel::Opus55)), Ok(()));
        assert!(validate(&options(Some("bad\nname"), None, BedrockModel::Haiku55)).unwrap_err().contains("profile name"));
        assert!(validate(&options(Some(&"p".repeat(129)), None, BedrockModel::Haiku55)).unwrap_err().contains("profile name"));
        // A Region that exists but is not in the list.
        assert!(validate(&options(None, Some("me-central-1"), BedrockModel::Haiku55)).unwrap_err().contains("Region"));
    }

    #[test]
    fn the_target_needs_a_listed_profile_and_a_region_of_the_list() {
        let profiles = [
            AwsProfile { name: "work".into(), region: Some("us-west-2".into()) },
            AwsProfile { name: "far".into(), region: Some("me-central-1".into()) },
        ];
        let work = resolve(&options(Some("work"), None, BedrockModel::Gpt62Sol), &profiles).unwrap();
        assert_eq!((work.region.as_str(), work.inference_profile.as_str()), ("us-west-2", "us.openai.gpt-6.2-sol"));
        assert_eq!(
            describe(&options(Some("work"), Some("eu-west-3"), BedrockModel::Opus55), &profiles).unwrap(),
            "SottoASR sends dictation text to Amazon Bedrock in eu-west-3 with the AWS profile work. The model is Claude Opus 5.5."
        );
        assert_eq!(
            resolve(&options(Some("far"), None, BedrockModel::Haiku55), &profiles).unwrap_err(),
            "Amazon Bedrock has no cleanup model in me-central-1. Choose a Region in Settings."
        );
        assert_eq!(resolve(&options(Some("far"), Some("us-east-1"), BedrockModel::Haiku55), &profiles).unwrap().region, "us-east-1");
        assert_eq!(resolve(&options(None, None, BedrockModel::Haiku55), &profiles).unwrap_err(), "Choose an AWS profile for AWS Bedrock in Settings.");
        assert!(resolve(&options(Some("gone"), None, BedrockModel::Haiku55), &profiles).unwrap_err().contains("was not found"));
    }

    /// `json.dumps(prompt, ensure_ascii=False, sort_keys=True, separators=(",", ":"))`
    /// of the sidecar, for the four fields that it hashes.
    fn canonical(prompt: &Prompt) -> String {
        let text = |value: &str| serde_json::to_string(value).unwrap();
        let examples: Vec<String> = prompt.fewshot.iter()
            .map(|example| format!("{{\"cleaned\":{},\"raw\":{}}}", text(&example.cleaned), text(&example.raw)))
            .collect();
        format!(
            "{{\"fewshot\":[{}],\"system\":{},\"user_prefix\":{},\"user_suffix\":{}}}",
            examples.join(","), text(&prompt.system), text(&prompt.user_prefix), text(&prompt.user_suffix)
        )
    }

    #[test]
    fn the_prompts_are_the_pair_that_the_sidecar_pins() {
        use sha2::{Digest, Sha256};
        let (retype, _) = prompt(CleanupMode::Retype);
        let (replace, _) = prompt(CleanupMode::Replace);
        let digest = Sha256::digest(format!("{}\n{}", canonical(retype), canonical(replace)).as_bytes());
        let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(hex, PROMPT_SHA256);
    }

    #[test]
    fn the_request_text_has_the_shape_that_the_sidecar_builds() {
        let (prompt, system) = prompt(CleanupMode::Retype);
        assert_eq!(
            prompt.user("I um agree."),
            "<transcript>\nI um agree.\n</transcript>\nThe transcript above contains disfluencies that must be cleaned. \
             Write the cleaned transcript for every one of them now. Reply with nothing but the cleaned transcript."
        );
        assert!(system.starts_with("Clean up speech disfluencies in the transcript."));
        // The examples follow the system text: an opening tag, one block for each, a closing tag.
        let (rules, examples) = system.split_once("\n\n<examples>\n").unwrap();
        assert_eq!(rules, prompt.system);
        let first = format!(
            "<example>\nInput:\n{}\nOutput:\nI need the green notebook tomorrow.\n</example>\n<example>\n",
            prompt.user("I um need uh the the green notebook tomorrow.")
        );
        assert!(examples.starts_with(&first), "{examples}");
        assert!(examples.ends_with("\nOutput:\nSet the field named um to zero.\n</example>\n</examples>"));
        assert_eq!(examples.matches("<example>").count(), 5);

        let (_, replace) = super::prompt(CleanupMode::Replace);
        assert!(replace.contains("OLD|||NEW") && replace.contains("\nOutput:\npack the uh um those|||pack those\n</example>\n</examples>"));
    }

    #[test]
    fn an_answer_becomes_a_proposal_only_when_it_is_complete_text() {
        let text = |value: &str| ContentBlock::Text(value.to_owned());
        assert_eq!(
            proposal(&StopReason::EndTurn, &[text("I need the report.\n")]),
            BatchItem::Proposal(Some("I need the report.".into()))
        );
        // An empty text is a valid answer: the model erased the window.
        assert_eq!(proposal(&StopReason::EndTurn, &[text("")]), BatchItem::Proposal(Some(String::new())));
        // Text in more than one block is one text.
        assert_eq!(proposal(&StopReason::StopSequence, &[text("I need "), text("it.")]), BatchItem::Proposal(Some("I need it.".into())));
        assert_eq!(proposal(&StopReason::MaxTokens, &[text("I need the")]), BatchItem::Failed("incomplete_generation".into()));
        assert_eq!(proposal(&StopReason::EndTurn, &[]), BatchItem::Failed("no_text".into()));
        assert_eq!(proposal(&StopReason::ContentFiltered, &[text("I need it.")]), BatchItem::Failed("no_text".into()));
        assert_eq!(proposal(&StopReason::EndTurn, &[text(&"a".repeat(32_001))]), BatchItem::Failed("proposal_limit".into()));
    }

    #[test]
    fn each_service_error_has_its_own_sentence() {
        let target = target();
        let text = |code: &str, message: &str| match service_answer(Some(code), Some(message), &target) {
            Answer::Batch(text) => text,
            answer => panic!("{code} gave {answer:?}"),
        };
        assert_eq!(
            text("AccessDeniedException", "User: arn:aws:sts::1:assumed-role/x is not authorized to perform: bedrock:InvokeModel"),
            "AWS profile \"work\" cannot use Claude Sonnet 5.5 in Amazon Bedrock. It needs the permission bedrock:InvokeModel and access to the model."
        );
        for code in ["UnrecognizedClientException", "ExpiredTokenException", "InvalidSignatureException"] {
            assert_eq!(text(code, "The security token included in the request is invalid"),
                "The credentials of AWS profile \"work\" are not valid or are expired. Sign in again.");
        }
        assert_eq!(text("ResourceNotFoundException", "Model not found"), "Amazon Bedrock does not have Claude Sonnet 5.5 in eu-west-1 for this account.");
        assert_eq!(text("ValidationException", "`temperature` is deprecated for this model."),
            "Amazon Bedrock did not accept the request: `temperature` is deprecated for this model.");
        // AWS refused one request, or the model failed for it: the other windows still count.
        for code in ["ThrottlingException", "ServiceQuotaExceededException"] {
            assert_eq!(
                service_answer(Some(code), Some("Too many requests"), &target),
                Answer::Refused("Amazon Bedrock has too many requests for this account. Try again in a moment.".into())
            );
        }
        assert_eq!(service_answer(Some("ModelErrorException"), Some("boom"), &target), Answer::Refused("Amazon Bedrock had an error: boom".into()));
        assert_eq!(service_answer(Some("ModelTimeoutException"), Some("Model has timed out"), &target), Answer::Item(BatchItem::TimedOut));

        let kept = BatchItem::Proposal(Some("I agree.".into()));
        assert_eq!(
            items(vec![Answer::Item(kept.clone()), Answer::Refused("Amazon Bedrock had an error: boom".into()), Answer::Item(BatchItem::TimedOut)]),
            Ok(vec![kept.clone(), BatchItem::Failed("Amazon Bedrock had an error: boom".into()), BatchItem::TimedOut])
        );
        // A failure that each window would have fails the batch.
        assert_eq!(items(vec![Answer::Item(kept), Answer::Batch("no permission".into())]), Err("no permission".into()));
    }

    #[test]
    fn an_sdk_error_fails_the_batch_with_the_sentence_of_its_cause() {
        use aws_credential_types::provider::error::CredentialsError;
        use aws_sdk_bedrockruntime::error::{ConnectorError, ErrorMetadata};
        use aws_sdk_bedrockruntime::operation::converse::ConverseError;
        type Error = SdkError<ConverseError, ()>;
        let target = target();
        let sentence = |error: Error| match failed(&error, &target) {
            Answer::Batch(text) => text,
            answer => panic!("{answer:?}"),
        };

        let metadata = ErrorMetadata::builder().code("AccessDeniedException").message("not authorized").build();
        let denied = sentence(Error::service_error(ConverseError::generic(metadata), ()));
        assert!(denied.contains("bedrock:InvokeModel"), "{denied}");

        let provider = CredentialsError::provider_error("external process exited with code exit status: 1. Stderr: sign in first");
        let credentials = sentence(Error::dispatch_failure(ConnectorError::other(provider.into(), None)));
        assert_eq!(credentials, "Could not get credentials for AWS profile \"work\": external process exited with code exit status: 1. Stderr: sign in first.");

        let refused = sentence(Error::dispatch_failure(ConnectorError::io("connection refused".into())));
        assert_eq!(refused, "Could not connect to Amazon Bedrock in eu-west-1. Examine your internet connection.");

        // The planner keeps the backend after these sentences: none looks like a dead sidecar.
        for text in [&denied, &credentials, &refused, &sentence(Error::timeout_error("slow"))] {
            assert!(!is_zombie_error(text), "{text}");
        }
    }

    #[test]
    fn the_backend_needs_the_application_runtime_and_loads_nothing() {
        let config = aws_config::SdkConfig::builder().behavior_version(aws_config::BehaviorVersion::latest()).build();
        let mut backend = BedrockCleanup {
            options: options(Some("work"), None, BedrockModel::Sonnet55),
            target: target(),
            client: Client::new(&config),
        };
        assert!(backend.is_alive());
        assert_eq!(backend.bedrock_options(), Some(&options(Some("work"), None, BedrockModel::Sonnet55)));
        assert!(backend.request_raw(&serde_json::json!({"action": "load"})).is_err());
        // Outside the runtime no request can be made, and none is tried.
        assert_eq!(
            backend.cleanup_batch(&["I um agree.".into()], CleanupMode::Retype),
            Err("AWS Bedrock needs the application runtime.".into())
        );
    }
}
