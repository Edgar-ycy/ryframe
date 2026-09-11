use serde::{Deserialize, Serialize};

use super::{DevexRunOptions, DevexSuite};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum BaselineContract {
    LegacyCargoDevV1,
    LegacyCargoDevV2,
    LegacyStableReadinessB0V1,
}

impl BaselineContract {
    pub(crate) const LEGACY_CARGO_DEV_BASE_COMMIT: &'static str =
        "7477b30133356eba16276bb0bcb80e356222afbd";
    pub(crate) const LEGACY_CARGO_DEV_ADAPTER_COMMIT: &'static str =
        "f37e5d25d2268f69712ba22d4e7083ef19d14a5e";
    pub(crate) const LEGACY_CARGO_DEV_V2_BASE_COMMIT: &'static str =
        "f37e5d25d2268f69712ba22d4e7083ef19d14a5e";
    pub(crate) const LEGACY_CARGO_DEV_V2_ADAPTER_COMMIT: &'static str =
        "47ee7937aed3ac035049abb360cc235716377a65";
    pub(crate) const STABLE_READINESS_B0_BASE_COMMIT: &'static str =
        "815c5eafb09d4b493319d255fb7ab88ddd02c8b6";
    pub(crate) const STABLE_READINESS_B0_FRONTEND_COMMIT: &'static str =
        "0087ea2ecf62530d042b9e52f5c950fb34c66d78";
    pub(crate) const STABLE_READINESS_B0_PATCH_SHA256: &'static str =
        "sha256:28ad29f21ca9fd58356d059700bcc871172466e0a62286c5ccdbf68b1ed3fba3";
    pub(crate) const STABLE_READINESS_B0_ADAPTER_PATHS: [&'static str; 1] = ["xtask/src/cli.rs"];
    pub(crate) const STABLE_READINESS_B0_ADAPTER_PATCH: &'static [u8] =
        include_bytes!("../../../assets/baseline-adapters/stable-readiness-b0-v1.patch");

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "legacy-cargo-dev-v1" => Some(Self::LegacyCargoDevV1),
            "legacy-cargo-dev-v2" => Some(Self::LegacyCargoDevV2),
            "legacy-stable-readiness-b0-v1" => Some(Self::LegacyStableReadinessB0V1),
            _ => None,
        }
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::LegacyCargoDevV1 => "legacy-cargo-dev-v1",
            Self::LegacyCargoDevV2 => "legacy-cargo-dev-v2",
            Self::LegacyStableReadinessB0V1 => "legacy-stable-readiness-b0-v1",
        }
    }

    pub(crate) fn validate(self, options: &DevexRunOptions) -> Result<(), String> {
        let valid = match self {
            Self::LegacyCargoDevV1 => {
                options.suite == DevexSuite::CargoDevSave && options.variant == "config-only"
            }
            Self::LegacyCargoDevV2 => {
                options.suite == DevexSuite::CargoDevSave
                    && matches!(options.variant.as_str(), "api-only" | "worker-only")
            }
            Self::LegacyStableReadinessB0V1 => matches!(
                options.suite,
                DevexSuite::ResourceGenerator | DevexSuite::ResourceGate | DevexSuite::RustGate
            ),
        };
        valid.then_some(()).ok_or_else(|| {
            format!(
                "baseline contract `{}` 不允许 suite `{}` 的变体 `{}`",
                self.as_str(),
                options.suite.as_str(),
                options.variant,
            )
        })
    }

    pub(crate) const fn base_commit(self) -> &'static str {
        match self {
            Self::LegacyCargoDevV1 => Self::LEGACY_CARGO_DEV_BASE_COMMIT,
            Self::LegacyCargoDevV2 => Self::LEGACY_CARGO_DEV_V2_BASE_COMMIT,
            Self::LegacyStableReadinessB0V1 => Self::STABLE_READINESS_B0_BASE_COMMIT,
        }
    }

    pub(crate) const fn required_adapter_commit(self) -> Option<&'static str> {
        match self {
            Self::LegacyCargoDevV1 => Some(Self::LEGACY_CARGO_DEV_ADAPTER_COMMIT),
            Self::LegacyCargoDevV2 => Some(Self::LEGACY_CARGO_DEV_V2_ADAPTER_COMMIT),
            Self::LegacyStableReadinessB0V1 => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct BaselineProvenance {
    pub(crate) base_commit: String,
    pub(crate) adapter_commit: String,
    pub(crate) patch_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) frontend_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) adapter_paths: Vec<String>,
}
