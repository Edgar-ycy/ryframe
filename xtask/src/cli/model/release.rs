use std::path::PathBuf;

use super::CliError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReleaseCommand {
    Source(ReleaseSourceOptions),
    Ci(ReleaseCiOptions),
}

impl ReleaseCommand {
    pub(crate) const fn plan(&self) -> bool {
        match self {
            Self::Source(options) => options.plan,
            Self::Ci(options) => options.plan,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReleaseSourceOptions {
    pub(crate) tag: ReleaseTag,
    pub(crate) backend_repository: RepositorySlug,
    pub(crate) backend_commit: GitSha,
    pub(crate) frontend_repository: RepositorySlug,
    pub(crate) frontend_commit: GitSha,
    pub(crate) manifest_path: PathBuf,
    pub(crate) plan: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReleaseCiOptions {
    pub(crate) operation: ReleaseCiOperation,
    pub(crate) plan: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReleaseCiOperation {
    Evidence(ReleaseCiEvidenceOptions),
    RecordPair { output: PathBuf },
    VerifyPair { input: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReleaseCiEvidenceOptions {
    pub(crate) backend_repository: RepositorySlug,
    pub(crate) frontend_repository: RepositorySlug,
    pub(crate) backend_sha: GitSha,
    pub(crate) frontend_sha: GitSha,
    pub(crate) tag: ReleaseTag,
    pub(crate) timeout_seconds: u16,
    pub(crate) output: PathBuf,
    pub(crate) tag_oids: Option<ReleaseTagOids>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReleaseTagOids {
    pub(crate) backend: GitSha,
    pub(crate) frontend: GitSha,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepositorySlug(String);

impl RepositorySlug {
    pub(crate) fn parse(value: &str, option: &str) -> Result<Self, CliError> {
        let mut components = value.split('/');
        let owner = components.next().unwrap_or_default();
        let repository = components.next().unwrap_or_default();
        if components.next().is_some()
            || !valid_repository_component(owner)
            || !valid_repository_component(repository)
        {
            return Err(CliError::new(format!(
                "{option} 必须是有效的 owner/repository"
            )));
        }
        Ok(Self(value.to_owned()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

fn valid_repository_component(value: &str) -> bool {
    value
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitSha(String);

impl GitSha {
    pub(crate) fn parse(value: &str, option: &str) -> Result<Self, CliError> {
        if value.len() != 40
            || value.bytes().all(|byte| byte == b'0')
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(CliError::new(format!(
                "{option} 必须是非零的 40 位小写十六进制 SHA"
            )));
        }
        Ok(Self(value.to_owned()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReleaseTag(String);

impl ReleaseTag {
    pub(crate) fn parse(value: &str) -> Result<Self, CliError> {
        let components = value
            .strip_prefix('v')
            .map(|version| version.split('.').collect::<Vec<_>>())
            .unwrap_or_default();
        if components.len() != 3 || components.iter().any(|part| !valid_version_part(part)) {
            return Err(CliError::new(
                "--tag 必须是规范的 vMAJOR.MINOR.PATCH，且不允许预发布后缀",
            ));
        }
        Ok(Self(value.to_owned()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

fn valid_version_part(value: &str) -> bool {
    value == "0"
        || (value
            .bytes()
            .next()
            .is_some_and(|byte| matches!(byte, b'1'..=b'9'))
            && value.bytes().all(|byte| byte.is_ascii_digit()))
}
