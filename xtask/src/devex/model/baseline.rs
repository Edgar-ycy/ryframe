use serde::{Deserialize, Serialize};

use super::{DevexRunOptions, DevexSuite};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum BaselineContract {
    LegacyCargoDevV1,
    LegacyCargoDevV2,
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

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "legacy-cargo-dev-v1" => Some(Self::LegacyCargoDevV1),
            "legacy-cargo-dev-v2" => Some(Self::LegacyCargoDevV2),
            _ => None,
        }
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::LegacyCargoDevV1 => "legacy-cargo-dev-v1",
            Self::LegacyCargoDevV2 => "legacy-cargo-dev-v2",
        }
    }

    pub(crate) fn validate(self, options: &DevexRunOptions) -> Result<(), String> {
        let valid = options.suite == DevexSuite::CargoDevSave
            && match self {
                Self::LegacyCargoDevV1 => options.variant == "config-only",
                Self::LegacyCargoDevV2 => {
                    matches!(options.variant.as_str(), "api-only" | "worker-only")
                }
            };
        valid.then_some(()).ok_or_else(|| {
            format!(
                "baseline contract `{}` 不允许当前 cargo-dev-save 变体",
                self.as_str()
            )
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct BaselineProvenance {
    pub(crate) base_commit: String,
    pub(crate) adapter_commit: String,
    pub(crate) patch_sha256: String,
}
