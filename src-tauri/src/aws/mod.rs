//! What the two optional AWS engines share: the profile list, the SDK
//! configuration for one profile, and the reading of an SDK error. AWS
//! Transcribe (`asr/aws_transcribe.rs`) and Amazon Bedrock
//! (`llm/bedrock.rs`) put their own sentences around these.

pub mod profiles;

use std::time::Duration;

use aws_config::profile::ProfileFileCredentialsProvider;
use aws_config::provider_config::ProviderConfig;
use aws_config::retry::RetryConfig;
use aws_config::timeout::TimeoutConfig;
use aws_config::{BehaviorVersion, Region, SdkConfig};
use aws_credential_types::provider::error::CredentialsError;
// The SDK crates of the services share these types.
use aws_sdk_transcribestreaming::error::{ProvideErrorMetadata, SdkError};
use aws_smithy_http_client::tls::{rustls_provider::CryptoMode, Provider};

use profiles::AwsProfile;

/// The profile and the Region of a request. The selected profile must be in
/// the AWS files. The Region is the selected one, else the one of the
/// profile. `service` is the name of the engine in Settings.
pub fn resolve(
    profile: Option<&str>,
    region: Option<&str>,
    profiles: &[AwsProfile],
    service: &str,
) -> Result<(String, String), String> {
    let profile = profile.ok_or_else(|| format!("Choose an AWS profile for {service} in Settings."))?;
    let found = profiles.iter().find(|candidate| candidate.name == profile)
        .ok_or_else(|| format!("AWS profile \"{profile}\" was not found in ~/.aws/config or ~/.aws/credentials."))?;
    let region = region.or(found.region.as_deref())
        .ok_or_else(|| format!("AWS profile \"{profile}\" has no Region. Choose a Region in Settings."))?;
    Ok((profile.to_owned(), region.to_owned()))
}

/// The SDK configuration for one profile and Region. The credentials come
/// from the selected profile only. The HTTP client uses rustls with ring,
/// the provider that the rest of the application links; the credentials
/// provider gets the same client, because a provider without one cannot
/// resolve an SSO or role profile.
pub async fn sdk_config(profile: &str, region: &str, retry: RetryConfig) -> SdkConfig {
    let http = aws_smithy_http_client::Builder::new()
        .tls_provider(Provider::Rustls(CryptoMode::Ring))
        .build_https();
    let region = Region::new(region.to_owned());
    let provider_config = ProviderConfig::default()
        .with_http_client(http.clone())
        .with_region(Some(region.clone()));
    let credentials = ProfileFileCredentialsProvider::builder()
        .configure(&provider_config)
        .profile_name(profile)
        .build();
    aws_config::defaults(BehaviorVersion::latest())
        .http_client(http)
        .credentials_provider(credentials)
        .region(region)
        .retry_config(retry)
        .timeout_config(TimeoutConfig::builder().connect_timeout(Duration::from_secs(10)).build())
        .load()
        .await
}

/// What an SDK error means.
#[derive(Debug, PartialEq, Eq)]
pub enum Cause {
    /// The service answered with an error.
    Service { code: Option<String>, message: Option<String> },
    /// The credentials provider failed. The text is the last cause.
    Credentials(String),
    /// The request did not get to the service, or no answer came in time.
    Connection,
    /// Another failure. The text is the last cause.
    Other(String),
}

pub fn cause<E, R>(error: &SdkError<E, R>) -> Cause
where
    E: ProvideErrorMetadata + std::error::Error + 'static,
    R: std::fmt::Debug + 'static,
{
    if matches!(error, SdkError::ServiceError(_)) {
        return Cause::Service { code: error.code().map(str::to_owned), message: error.message().map(str::to_owned) };
    }
    // The cause is the last error of the chain. A credential provider
    // error is somewhere in the chain of a dispatch failure.
    let mut last: &(dyn std::error::Error + 'static) = error;
    let mut credentials = false;
    while let Some(source) = last.source() {
        credentials |= source.is::<CredentialsError>();
        last = source;
    }
    if credentials {
        return Cause::Credentials(last.to_string());
    }
    match error {
        SdkError::DispatchFailure(_) | SdkError::TimeoutError(_) => Cause::Connection,
        _ => Cause::Other(last.to_string()),
    }
}

/// The causes of an error for the log, from the first to the last. The
/// `DisplayErrorContext` of the SDK adds the `Debug` text, which contains the
/// body of an answer that the SDK could not read. That body can have the
/// words of a dictation, so it stays out of the log.
pub fn causes(error: &(dyn std::error::Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut last = error;
    while let Some(source) = last.source() {
        text.push_str(": ");
        text.push_str(&source.to_string());
        last = source;
    }
    text
}

/// The first line, at most 300 characters: a cause can carry a whole
/// standard-error text or a list of every valid value.
pub fn shortened(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    match line.char_indices().nth(300) {
        Some((end, _)) => format!("{}…", &line[..end]),
        None => line.to_owned(),
    }
}

/// The sentence for a credentials provider that failed.
pub fn credentials_text(cause: &str, profile: &str) -> String {
    let mut text = format!("Could not get credentials for AWS profile \"{profile}\": {}", shortened(cause));
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

/// The sentence for credentials that the service did not accept, if `code`
/// is such an error.
pub fn rejected_credentials_text(code: &str, profile: &str) -> Option<String> {
    matches!(
        code,
        "UnrecognizedClientException" | "ExpiredTokenException" | "ExpiredToken" | "InvalidSignatureException"
            | "InvalidClientTokenId" | "IncompleteSignature"
    )
    .then(|| format!("The credentials of AWS profile \"{profile}\" are not valid or are expired. Sign in again."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_selected_region_wins_and_the_profile_must_exist() {
        let profiles = [
            AwsProfile { name: "work".into(), region: Some("eu-west-1".into()) },
            AwsProfile { name: "bare".into(), region: None },
        ];
        assert_eq!(resolve(Some("work"), None, &profiles, "AWS Bedrock"), Ok(("work".into(), "eu-west-1".into())));
        assert_eq!(resolve(Some("work"), Some("us-east-2"), &profiles, "AWS Bedrock"), Ok(("work".into(), "us-east-2".into())));
        assert_eq!(resolve(Some("bare"), Some("us-east-2"), &profiles, "AWS Bedrock"), Ok(("bare".into(), "us-east-2".into())));
        assert_eq!(
            resolve(None, Some("us-east-2"), &profiles, "AWS Bedrock").unwrap_err(),
            "Choose an AWS profile for AWS Bedrock in Settings."
        );
        assert!(resolve(Some("gone"), None, &profiles, "AWS Bedrock").unwrap_err().contains("\"gone\" was not found"));
        assert!(resolve(Some("bare"), None, &profiles, "AWS Bedrock").unwrap_err().contains("has no Region"));
    }

    #[test]
    fn a_credential_error_names_the_profile_and_explains_a_command_that_was_not_found() {
        let missing = credentials_text(
            "Error retrieving credentials: external process exited with code exit status: 127. Stderr: sh: aws-vault: command not found\nmore", "work");
        assert!(missing.starts_with("Could not get credentials for AWS profile \"work\": Error retrieving credentials"), "{missing}");
        assert!(missing.contains("command not found") && !missing.contains("more"), "{missing}");
        assert!(missing.ends_with("Use the full path of the command in ~/.aws/config."), "{missing}");

        // A full path that does not exist is not a PATH problem.
        let absent = credentials_text(
            "Error retrieving credentials: external process exited with code exit status: 127. Stderr: sh: /opt/tool: No such file or directory", "work");
        assert!(absent.ends_with("No such file or directory."), "{absent}");

        let expired = credentials_text("the SSO token has expired", "work");
        assert_eq!(expired, "Could not get credentials for AWS profile \"work\": the SSO token has expired.");
    }

    #[test]
    fn the_log_text_of_an_error_has_each_cause_and_not_the_answer_of_the_service() {
        let unreadable: SdkError<std::io::Error, String> =
            SdkError::response_error(std::io::Error::other("the answer was not complete"), "the words of a dictation".to_owned());
        let text = causes(&unreadable);
        assert!(text.ends_with(": the answer was not complete"), "{text}");
        assert!(!text.contains("dictation"), "{text}");
    }
}
