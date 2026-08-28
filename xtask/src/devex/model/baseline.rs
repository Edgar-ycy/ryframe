use serde::{Deserialize, Serialize};

use super::{DevexRunOptions, DevexSuite};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum BaselineContract {
    LegacyCargoDevV1,
}

impl BaselineContract {
    pub(crate) const LEGACY_CARGO_DEV_BASE_COMMIT: &'static str =
        "7477b30133356eba16276bb0bcb80e356222afbd";
    pub(crate) const LEGACY_CARGO_DEV_ADAPTER_COMMIT: &'static str =
        "f37e5d25d2268f69712ba22d4e7083ef19d14a5e";

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "legacy-cargo-dev-v1" => Some(Self::LegacyCargoDevV1),
            _ => None,
        }
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::LegacyCargoDevV1 => "legacy-cargo-dev-v1",
        }
    }

    pub(crate) fn validate(self, options: &DevexRunOptions) -> Result<(), String> {
        if options.suite == DevexSuite::CargoDevSave && options.variant == "config-only" {
            Ok(())
        } else {
            Err(format!(
                "baseline contract `{}` 仅允许 cargo-dev-save/config-only",
                self.as_str()
            ))
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct BaselineProvenance {
    pub(crate) base_commit: String,
    pub(crate) adapter_commit: String,
    pub(crate) patch_sha256: String,
}
