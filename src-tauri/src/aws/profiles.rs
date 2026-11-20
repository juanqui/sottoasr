//! The AWS profiles on this Mac, read with the parser of the AWS SDK. Only
//! names and regions are read here. The SDK reads the credentials itself
//! when it signs a request; SottoASR never reads, shows or stores them.

use aws_types::os_shim_internal::{Env, Fs};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AwsProfile {
    pub name: String,
    /// The `region` value of the profile.
    pub region: Option<String>,
}

/// The profiles of `~/.aws/config` and `~/.aws/credentials`, or of the files
/// that `AWS_CONFIG_FILE` and `AWS_SHARED_CREDENTIALS_FILE` name. `default`
/// comes first, then the others by name. No file gives an empty list.
pub async fn list() -> Result<Vec<AwsProfile>, String> {
    load(&Fs::real(), &Env::real()).await
}

async fn load(fs: &Fs, env: &Env) -> Result<Vec<AwsProfile>, String> {
    let set = aws_config::profile::load(fs, env, &Default::default(), None).await
        .map_err(|error| format!("Could not read the AWS profiles: {error}"))?;
    let mut profiles: Vec<AwsProfile> = set.profiles()
        .map(|name| AwsProfile {
            name: name.to_owned(),
            region: set.get_profile(name).and_then(|profile| profile.get("region")).map(str::to_owned),
        })
        .collect();
    profiles.sort_by(|left, right| (left.name != "default", &left.name).cmp(&(right.name != "default", &right.name)));
    Ok(profiles)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = "[profile work]\nregion = eu-west-1\ncredential_process = /usr/local/bin/tool print\n\n\
        [default]\nregion=us-east-1\n\n[sso-session company]\nsso_region = us-east-1\n\n[profile  spaced ]\noutput = json\n";
    const CREDENTIALS: &str = "[default]\naws_access_key_id = AKIAEXAMPLE\naws_secret_access_key = secret\n\n\
        [keys-only]\naws_access_key_id = AKIAEXAMPLE\naws_secret_access_key = secret\n";

    fn profile(name: &str, region: Option<&str>) -> AwsProfile {
        AwsProfile { name: name.into(), region: region.map(str::to_owned) }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn profiles_come_from_both_files_with_default_first_and_no_credential_value() {
        let fs = Fs::from_slice(&[("/home/.aws/config", CONFIG), ("/home/.aws/credentials", CREDENTIALS)]);
        let env = Env::from_slice(&[("HOME", "/home")]);
        let profiles = load(&fs, &env).await.unwrap();
        assert_eq!(profiles, [
            profile("default", Some("us-east-1")),
            profile("keys-only", None),
            profile("spaced", None),
            profile("work", Some("eu-west-1")),
        ]);
        let listed = serde_json::to_string(&profiles).unwrap();
        assert!(!listed.contains("AKIA") && !listed.contains("secret"), "{listed}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_named_files_replace_the_default_locations_and_no_file_gives_no_profile() {
        let fs = Fs::from_slice(&[("/elsewhere/config", "[profile moved]\nregion = us-west-2\n"), ("/home/.aws/config", CONFIG)]);
        let env = Env::from_slice(&[("HOME", "/home"), ("AWS_CONFIG_FILE", "/elsewhere/config")]);
        assert_eq!(load(&fs, &env).await.unwrap(), [profile("moved", Some("us-west-2"))]);

        let empty = Fs::from_slice(&[]);
        assert_eq!(load(&empty, &Env::from_slice(&[("HOME", "/home")])).await.unwrap(), []);
    }
}
