use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[path = "model/baseline.rs"]
mod baseline;
#[path = "model/execution_contract.rs"]
mod execution_contract;
#[path = "model/steps.rs"]
mod steps;
use steps::*;

pub(crate) use baseline::{BaselineContract, BaselineProvenance};
pub(crate) use execution_contract::{
    RESOURCE_GATE_DECISION_ENV, RESOURCE_GATE_DECISION_TEMPLATE, RESOURCE_GATE_TARGETED_ACTIVATION,
    RESOURCE_GATE_TARGETED_ENV,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum DevexSuite {
    RustColdBuild,
    RustIncremental,
    CargoDevSave,
    ResourceGenerator,
    ResourceGate,
    RustGate,
    RustSccache,
    FrontendFast,
    FrontendBuild,
}

impl DevexSuite {
    pub(crate) const ALL: [Self; 9] = [
        Self::RustColdBuild,
        Self::RustIncremental,
        Self::CargoDevSave,
        Self::ResourceGenerator,
        Self::ResourceGate,
        Self::RustGate,
        Self::RustSccache,
        Self::FrontendFast,
        Self::FrontendBuild,
    ];

    pub(crate) fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|suite| suite.as_str() == value)
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::RustColdBuild => "rust-cold-build",
            Self::RustIncremental => "rust-incremental",
            Self::CargoDevSave => "cargo-dev-save",
            Self::ResourceGenerator => "resource-generator",
            Self::ResourceGate => "resource-gate",
            Self::RustGate => "rust-gate",
            Self::RustSccache => "rust-sccache",
            Self::FrontendFast => "frontend-fast",
            Self::FrontendBuild => "frontend-build",
        }
    }

    pub(crate) fn definition(self, variant: &str) -> Result<SuiteDefinition, String> {
        match self {
            Self::RustColdBuild => Ok(SuiteDefinition {
                steps: rust_steps(variant, BuildProfile::Build)?,
                features: rust_features(variant)?,
                environment: &[("CARGO_INCREMENTAL", "0")],
                remove_environment: &[
                    "RUSTC_WRAPPER",
                    "SCCACHE_RECACHE",
                    "CMAKE_C_COMPILER_LAUNCHER",
                    "CMAKE_CXX_COMPILER_LAUNCHER",
                ],
                requirement: SuiteRequirement::Ready,
                requires_frontend: false,
            }),
            Self::RustIncremental => Ok(SuiteDefinition {
                steps: rust_steps(variant, BuildProfile::Check)?,
                features: rust_features(variant)?,
                environment: &[("CARGO_INCREMENTAL", "1")],
                remove_environment: &[],
                requirement: SuiteRequirement::Ready,
                requires_frontend: false,
            }),
            Self::CargoDevSave => Ok(SuiteDefinition {
                steps: CARGO_DEV_SAVE,
                features: &[],
                environment: save_case_environment(variant)?,
                remove_environment: &[],
                requirement: SuiteRequirement::Ready,
                requires_frontend: false,
            }),
            Self::ResourceGenerator => Ok(SuiteDefinition {
                steps: resource_generator_steps(variant)?,
                features: &["resource"],
                environment: &[],
                remove_environment: &[],
                requirement: SuiteRequirement::FrontendFiles,
                requires_frontend: true,
            }),
            Self::ResourceGate => Ok(SuiteDefinition {
                steps: exact_variant(variant, "auto", RESOURCE_GATE, self)?,
                features: &["resource"],
                environment: &[
                    ("RYFRAME_DEVEX_TARGET_ROOT", "{target}"),
                    (
                        RESOURCE_GATE_TARGETED_ENV,
                        RESOURCE_GATE_TARGETED_ACTIVATION,
                    ),
                    (RESOURCE_GATE_DECISION_ENV, RESOURCE_GATE_DECISION_TEMPLATE),
                ],
                remove_environment: &[],
                requirement: SuiteRequirement::FrontendFiles,
                requires_frontend: true,
            }),
            Self::RustGate => Ok(SuiteDefinition {
                steps: exact_variant(variant, "default", RUST_GATE, self)?,
                features: &[],
                environment: &[
                    ("CARGO_INCREMENTAL", "0"),
                    ("RUSTC_WRAPPER", "sccache"),
                    ("RYFRAME_CI_RUST_GATE_PROFILE", "standard"),
                    ("RYFRAME_DEVEX_TARGET_ROOT", "{target}"),
                ],
                remove_environment: &["RYFRAME_CI_FRONTEND_REF", "SCCACHE_RECACHE"],
                requirement: SuiteRequirement::Executable("sccache"),
                requires_frontend: true,
            }),
            Self::RustSccache => Ok(SuiteDefinition {
                steps: rust_steps(variant, BuildProfile::Check)?,
                features: rust_features(variant)?,
                environment: &[("CARGO_INCREMENTAL", "0"), ("RUSTC_WRAPPER", "sccache")],
                remove_environment: &["SCCACHE_RECACHE"],
                requirement: SuiteRequirement::Executable("sccache"),
                requires_frontend: false,
            }),
            Self::FrontendFast => Ok(SuiteDefinition {
                steps: exact_variant(variant, "default", FRONTEND_FAST, self)?,
                features: &[],
                environment: &[
                    ("CI", "true"),
                    ("FORCE_COLOR", "0"),
                    ("NO_COLOR", "1"),
                    ("RYFRAME_FAST_CHECK_CACHE_ROOT", "{target}/frontend"),
                ],
                remove_environment: &[],
                requirement: SuiteRequirement::Frontend,
                requires_frontend: true,
            }),
            Self::FrontendBuild => Ok(SuiteDefinition {
                steps: exact_variant(variant, "default", FRONTEND_BUILD, self)?,
                features: &[],
                environment: &[("CI", "true"), ("FORCE_COLOR", "0"), ("NO_COLOR", "1")],
                remove_environment: &[],
                requirement: SuiteRequirement::Frontend,
                requires_frontend: true,
            }),
        }
    }

    pub(crate) const fn variant_help(self) -> &'static str {
        match self {
            Self::RustIncremental => "application、api、worker、migrate 或 workspace",
            Self::RustColdBuild | Self::RustSccache => "api、worker、migrate 或 workspace",
            Self::CargoDevSave => {
                "config-only、api-only、worker-only、shared-runtime、locales、migration-only、resource-manifest 或 cancellation"
            }
            Self::ResourceGenerator => "all、post 或 notice",
            Self::ResourceGate => "auto",
            Self::RustGate => "default",
            Self::FrontendFast | Self::FrontendBuild => "default",
        }
    }

    pub(crate) fn incremental_source(self, variant: &str) -> Result<Option<&'static str>, String> {
        if self != Self::RustIncremental {
            return Ok(None);
        }
        let source = match variant {
            "application" => "crates/ryframe-application/src/lib.rs",
            "api" => "crates/ryframe-api/src/lib.rs",
            "worker" => "crates/ryframe/src/bin/ryframe_worker.rs",
            "migrate" => "crates/ryframe/src/bin/ryframe_migrate.rs",
            "workspace" => "crates/ryframe-kernel/src/lib.rs",
            _ => {
                return Err(
                    "rust-incremental 的 --variant 只允许 application、api、worker、migrate 或 workspace"
                        .into(),
                );
            }
        };
        Ok(Some(source))
    }

    pub(crate) const fn uses_sccache(self) -> bool {
        matches!(self, Self::RustGate | Self::RustSccache)
    }
}
#[derive(Debug, Clone, Copy)]
enum BuildProfile {
    Build,
    Check,
}

fn rust_steps(variant: &str, profile: BuildProfile) -> Result<&'static [StepDefinition], String> {
    let steps = match (profile, variant) {
        (BuildProfile::Build, "api") => RUST_BUILD_API,
        (BuildProfile::Build, "worker") => RUST_BUILD_WORKER,
        (BuildProfile::Build, "migrate") => RUST_BUILD_MIGRATE,
        (BuildProfile::Build, "workspace") => RUST_BUILD_WORKSPACE,
        (BuildProfile::Check, "application") => RUST_CHECK_APPLICATION,
        (BuildProfile::Check, "api") => RUST_CHECK_API,
        (BuildProfile::Check, "worker") => RUST_CHECK_WORKER,
        (BuildProfile::Check, "migrate") => RUST_CHECK_MIGRATE,
        (BuildProfile::Check, "workspace") => RUST_CHECK_WORKSPACE,
        _ => return Err("Rust suite 的 --variant 只允许 api、worker、migrate 或 workspace".into()),
    };
    Ok(steps)
}

fn rust_features(variant: &str) -> Result<&'static [&'static str], String> {
    match variant {
        "application" => Ok(&["application"]),
        "api" => Ok(&["bin-api"]),
        "worker" => Ok(&["bin-worker"]),
        "migrate" => Ok(&["bin-migrate"]),
        "workspace" => Ok(&["workspace-default"]),
        _ => Err("Rust suite 的 --variant 只允许 api、worker、migrate 或 workspace".into()),
    }
}

fn resource_generator_steps(variant: &str) -> Result<&'static [StepDefinition], String> {
    match variant {
        "all" => Ok(RESOURCE_GENERATOR_ALL),
        "post" => Ok(RESOURCE_GENERATOR_POST),
        "notice" => Ok(RESOURCE_GENERATOR_NOTICE),
        _ => Err("resource-generator 的 --variant 只允许 all、post 或 notice".into()),
    }
}

fn save_case_environment(variant: &str) -> Result<&'static [(&'static str, &'static str)], String> {
    const RESULT: (&str, &str) = (
        "RYFRAME_DEVEX_SAVE_RESULT_PATH",
        "{target}/cargo-dev-save-result.json",
    );
    match variant {
        "config-only" => Ok(&[("RYFRAME_DEVEX_SAVE_CASE", "config-only"), RESULT]),
        "api-only" => Ok(&[("RYFRAME_DEVEX_SAVE_CASE", "api-only"), RESULT]),
        "worker-only" => Ok(&[("RYFRAME_DEVEX_SAVE_CASE", "worker-only"), RESULT]),
        "shared-runtime" => Ok(&[("RYFRAME_DEVEX_SAVE_CASE", "shared-runtime"), RESULT]),
        "locales" => Ok(&[("RYFRAME_DEVEX_SAVE_CASE", "locales"), RESULT]),
        "migration-only" => Ok(&[("RYFRAME_DEVEX_SAVE_CASE", "migration-only"), RESULT]),
        "resource-manifest" => Ok(&[("RYFRAME_DEVEX_SAVE_CASE", "resource-manifest"), RESULT]),
        "cancellation" => Ok(&[("RYFRAME_DEVEX_SAVE_CASE", "cancellation"), RESULT]),
        _ => Err("cargo-dev-save 的 --variant 不是已登记保存场景".into()),
    }
}

fn exact_variant(
    variant: &str,
    expected: &str,
    steps: &'static [StepDefinition],
    suite: DevexSuite,
) -> Result<&'static [StepDefinition], String> {
    if variant == expected {
        Ok(steps)
    } else {
        Err(format!("{} 的 --variant 只允许 {expected}", suite.as_str()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CacheState {
    Cold,
    Warm,
}

impl CacheState {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "cold" => Some(Self::Cold),
            "warm" => Some(Self::Warm),
            _ => None,
        }
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Cold => "cold",
            Self::Warm => "warm",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DevexRunOptions {
    pub(crate) suite: DevexSuite,
    pub(crate) variant: String,
    pub(crate) cache_state: CacheState,
    pub(crate) runs: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum PairedArm {
    Baseline,
    Candidate,
}

impl PairedArm {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Candidate => "candidate",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PairingMetadata {
    pub(crate) comparison_id: String,
    pub(crate) arm: PairedArm,
    #[serde(default)]
    pub(crate) baseline_contract: Option<BaselineContract>,
    #[serde(default)]
    pub(crate) baseline_provenance: Option<BaselineProvenance>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DevexPairedOptions {
    pub(crate) run: DevexRunOptions,
    pub(crate) baseline_backend: PathBuf,
    pub(crate) candidate_backend: PathBuf,
    pub(crate) baseline_frontend: Option<PathBuf>,
    pub(crate) candidate_frontend: Option<PathBuf>,
    pub(crate) baseline_contract: Option<BaselineContract>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DevexCommand {
    Run(DevexRunOptions),
    Paired(DevexPairedOptions),
    Summarize { run: String },
    Compare { baseline: String, candidate: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkingDirectory {
    Backend,
    Frontend,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct StepDefinition {
    pub(crate) working_directory: WorkingDirectory,
    pub(crate) program: &'static str,
    pub(crate) args: &'static [&'static str],
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum SuiteRequirement {
    Ready,
    Frontend,
    FrontendFiles,
    Executable(&'static str),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SuiteDefinition {
    pub(crate) steps: &'static [StepDefinition],
    pub(crate) features: &'static [&'static str],
    pub(crate) environment: &'static [(&'static str, &'static str)],
    pub(crate) remove_environment: &'static [&'static str],
    pub(crate) requirement: SuiteRequirement,
    pub(crate) requires_frontend: bool,
}
