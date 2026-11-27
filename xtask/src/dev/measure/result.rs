//! 保存测量的结果结构与文件读写。
use std::{fs, path::Path};

use serde::{Deserialize, Serialize};

use crate::{Result, utc_timestamp::rfc3339_order_key};

pub(crate) const RESULT_FILE_NAME: &str = "cargo-dev-save-result.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SaveCase {
    ConfigOnly,
    ApiOnly,
    WorkerOnly,
    SharedRuntime,
    Locales,
    MigrationOnly,
    ResourceManifest,
    Cancellation,
}

impl SaveCase {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "config-only" => Some(Self::ConfigOnly),
            "api-only" => Some(Self::ApiOnly),
            "worker-only" => Some(Self::WorkerOnly),
            "shared-runtime" => Some(Self::SharedRuntime),
            "locales" => Some(Self::Locales),
            "migration-only" => Some(Self::MigrationOnly),
            "resource-manifest" => Some(Self::ResourceManifest),
            "cancellation" => Some(Self::Cancellation),
            _ => None,
        }
    }

    pub(crate) const fn source(self) -> SaveSource {
        match self {
            Self::ConfigOnly => SaveSource::toml("config/app.dev.toml"),
            Self::ApiOnly => SaveSource::rust("crates/ryframe/src/app.rs"),
            Self::WorkerOnly => SaveSource::rust("crates/ryframe/src/bin/ryframe_worker.rs"),
            Self::SharedRuntime => SaveSource::rust("crates/ryframe-kernel/src/lib.rs"),
            Self::Locales => SaveSource::toml("locales/zh-CN.toml"),
            Self::MigrationOnly => SaveSource::rust("crates/ryframe/src/bin/ryframe_migrate.rs"),
            Self::ResourceManifest => SaveSource::toml("catalog/resources/post.toml"),
            Self::Cancellation => SaveSource::rust("crates/ryframe/src/app.rs"),
        }
    }

    pub(crate) const fn expected_ready(self) -> ReadyKind {
        match self {
            Self::ApiOnly | Self::WorkerOnly | Self::SharedRuntime | Self::Locales => {
                ReadyKind::Promoted
            }
            Self::ConfigOnly | Self::MigrationOnly | Self::ResourceManifest => {
                ReadyKind::VerifiedNoRestart
            }
            Self::Cancellation => ReadyKind::Superseded,
        }
    }

    pub(crate) const fn expected_cargo_invocations(self) -> usize {
        match self {
            Self::ConfigOnly => 0,
            Self::ApiOnly | Self::WorkerOnly | Self::MigrationOnly | Self::ResourceManifest => 1,
            Self::SharedRuntime | Self::Locales => 2,
            Self::Cancellation => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SaveSource {
    pub(crate) relative: &'static str,
    pub(super) comment_prefix: &'static str,
}

impl SaveSource {
    const fn rust(relative: &'static str) -> Self {
        Self {
            relative,
            comment_prefix: "//",
        }
    }

    const fn toml(relative: &'static str) -> Self {
        Self {
            relative,
            comment_prefix: "#",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ReadyKind {
    Promoted,
    VerifiedNoRestart,
    Superseded,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub(crate) struct SaveMeasurement {
    pub(crate) schema_version: u8,
    pub(crate) case: SaveCase,
    pub(crate) started_at: String,
    pub(crate) save_to_ready_ms: f64,
    pub(crate) cargo_invocations: usize,
    pub(crate) ready_kind: ReadyKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SaveMeasurementContract {
    Current,
    LegacyConfigOnlyBaseline,
    LegacyApiWorkerBaseline,
}

pub(super) fn remove_stale_result(path: Option<&Path>) -> Result<()> {
    if let Some(path) = path {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub(super) fn write_measurement(path: &Path, measurement: &SaveMeasurement) -> Result<()> {
    let mut document = serde_json::to_vec_pretty(measurement)?;
    document.push(b'\n');
    fs::write(path, document)?;
    Ok(())
}

#[allow(dead_code)]
pub(crate) fn read_measurement(path: &Path) -> Result<SaveMeasurement> {
    read_measurement_with_contract(path, SaveMeasurementContract::Current)
}

pub(crate) fn read_measurement_with_contract(
    path: &Path,
    contract: SaveMeasurementContract,
) -> Result<SaveMeasurement> {
    let measurement: SaveMeasurement = serde_json::from_slice(&fs::read(path)?)?;
    let (expected_cargo_invocations, expected_ready_kind) = match contract {
        SaveMeasurementContract::Current => (
            measurement.case.expected_cargo_invocations(),
            measurement.case.expected_ready(),
        ),
        SaveMeasurementContract::LegacyConfigOnlyBaseline
            if measurement.case == SaveCase::ConfigOnly =>
        {
            (1, ReadyKind::Promoted)
        }
        SaveMeasurementContract::LegacyConfigOnlyBaseline => {
            return Err("legacy-cargo-dev-v1 只允许 config-only 测量结果".into());
        }
        SaveMeasurementContract::LegacyApiWorkerBaseline
            if matches!(measurement.case, SaveCase::ApiOnly | SaveCase::WorkerOnly) =>
        {
            (1, ReadyKind::Promoted)
        }
        SaveMeasurementContract::LegacyApiWorkerBaseline => {
            return Err("legacy-cargo-dev-v2 只允许 api-only 或 worker-only 测量结果".into());
        }
    };
    if measurement.schema_version != 1
        || !measurement.save_to_ready_ms.is_finite()
        || measurement.save_to_ready_ms < 0.0
        || rfc3339_order_key(&measurement.started_at).is_err()
        || measurement.ready_kind != expected_ready_kind
        || measurement.cargo_invocations != expected_cargo_invocations
    {
        return Err("cargo-dev-save 结果 schema、时间或场景不变量无效".into());
    }
    Ok(measurement)
}
