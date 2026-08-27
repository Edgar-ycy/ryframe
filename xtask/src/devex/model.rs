use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum DevexSuite {
    RustColdBuild,
    RustIncremental,
    CargoDevSave,
    ResourceGenerator,
    ResourceGate,
    RustSccache,
    FrontendFast,
    FrontendBuild,
}

impl DevexSuite {
    pub(crate) const ALL: [Self; 8] = [
        Self::RustColdBuild,
        Self::RustIncremental,
        Self::CargoDevSave,
        Self::ResourceGenerator,
        Self::ResourceGate,
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
                remove_environment: &["RUSTC_WRAPPER", "SCCACHE_RECACHE"],
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
                environment: &[("RYFRAME_DEVEX_TARGET_ROOT", "{target}")],
                remove_environment: &[],
                requirement: SuiteRequirement::FrontendFiles,
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

    pub(crate) fn minimum_runs(self, variant: &str) -> usize {
        match self {
            Self::RustColdBuild | Self::RustIncremental | Self::RustSccache => 20,
            Self::CargoDevSave if !matches!(variant, "config-only" | "resource-manifest") => 20,
            _ => 5,
        }
    }

    pub(crate) const fn variant_help(self) -> &'static str {
        match self {
            Self::RustColdBuild | Self::RustIncremental | Self::RustSccache => {
                "api、worker、migrate 或 workspace"
            }
            Self::CargoDevSave => {
                "config-only、api-only、worker-only、shared-runtime、locales、migration-only 或 resource-manifest"
            }
            Self::ResourceGenerator => "all、post 或 notice",
            Self::ResourceGate => "auto",
            Self::FrontendFast | Self::FrontendBuild => "default",
        }
    }

    pub(crate) fn incremental_source(self, variant: &str) -> Result<Option<&'static str>, String> {
        if self != Self::RustIncremental {
            return Ok(None);
        }
        let source = match variant {
            "api" => "crates/ryframe-api/src/lib.rs",
            "worker" => "crates/ryframe/src/bin/ryframe_worker.rs",
            "migrate" => "crates/ryframe/src/bin/ryframe_migrate.rs",
            "workspace" => "crates/ryframe-kernel/src/lib.rs",
            _ => {
                return Err(
                    "Rust suite 的 --variant 只允许 api、worker、migrate 或 workspace".into(),
                );
            }
        };
        Ok(Some(source))
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DevexPairedOptions {
    pub(crate) run: DevexRunOptions,
    pub(crate) baseline_backend: PathBuf,
    pub(crate) candidate_backend: PathBuf,
    pub(crate) baseline_frontend: Option<PathBuf>,
    pub(crate) candidate_frontend: Option<PathBuf>,
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

const RUST_BUILD_WORKSPACE: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Backend,
    program: "cargo",
    args: &["build", "--locked", "--workspace"],
}];

const RUST_CHECK_WORKSPACE: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Backend,
    program: "cargo",
    args: &["check", "--locked", "--workspace"],
}];

const RUST_BUILD_API: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Backend,
    program: "cargo",
    args: &[
        "build",
        "--locked",
        "-p",
        "ryframe",
        "--no-default-features",
        "--features",
        "bin-api",
        "--bin",
        "ryframe",
    ],
}];
const RUST_BUILD_WORKER: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Backend,
    program: "cargo",
    args: &[
        "build",
        "--locked",
        "-p",
        "ryframe",
        "--no-default-features",
        "--features",
        "bin-worker",
        "--bin",
        "ryframe-worker",
    ],
}];
const RUST_BUILD_MIGRATE: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Backend,
    program: "cargo",
    args: &[
        "build",
        "--locked",
        "-p",
        "ryframe",
        "--no-default-features",
        "--features",
        "bin-migrate",
        "--bin",
        "ryframe-migrate",
    ],
}];
const RUST_CHECK_API: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Backend,
    program: "cargo",
    args: &[
        "check",
        "--locked",
        "-p",
        "ryframe",
        "--no-default-features",
        "--features",
        "bin-api",
        "--bin",
        "ryframe",
    ],
}];
const RUST_CHECK_WORKER: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Backend,
    program: "cargo",
    args: &[
        "check",
        "--locked",
        "-p",
        "ryframe",
        "--no-default-features",
        "--features",
        "bin-worker",
        "--bin",
        "ryframe-worker",
    ],
}];
const RUST_CHECK_MIGRATE: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Backend,
    program: "cargo",
    args: &[
        "check",
        "--locked",
        "-p",
        "ryframe",
        "--no-default-features",
        "--features",
        "bin-migrate",
        "--bin",
        "ryframe-migrate",
    ],
}];

const CARGO_DEV_SAVE: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Backend,
    program: "cargo",
    args: &["dev", "--measure-once"],
}];

const RESOURCE_GENERATOR_ALL: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Backend,
    program: "cargo",
    args: &[
        "run",
        "--locked",
        "--target-dir",
        "{target}",
        "-p",
        "xtask",
        "--features",
        "resource",
        "--",
        "resource",
        "--all",
        "--check",
        "--frontend-dir",
        "{frontend}",
    ],
}];
const RESOURCE_GENERATOR_POST: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Backend,
    program: "cargo",
    args: &[
        "run",
        "--locked",
        "--target-dir",
        "{target}",
        "-p",
        "xtask",
        "--features",
        "resource",
        "--",
        "resource",
        "post",
        "--check",
        "--frontend-dir",
        "{frontend}",
    ],
}];
const RESOURCE_GENERATOR_NOTICE: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Backend,
    program: "cargo",
    args: &[
        "run",
        "--locked",
        "--target-dir",
        "{target}",
        "-p",
        "xtask",
        "--features",
        "resource",
        "--",
        "resource",
        "notice",
        "--check",
        "--frontend-dir",
        "{frontend}",
    ],
}];

const RESOURCE_GATE: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Backend,
    program: "cargo",
    args: &[
        "run",
        "--locked",
        "--target-dir",
        "{target}/driver",
        "-p",
        "xtask",
        "--",
        "ci",
        "resource-gate",
        "--frontend-dir",
        "{frontend}",
    ],
}];

const FRONTEND_FAST: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Frontend,
    program: "corepack",
    args: &["pnpm", "check:fast"],
}];

const FRONTEND_BUILD: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Frontend,
    program: "corepack",
    args: &[
        "pnpm",
        "exec",
        "vite",
        "build",
        "--outDir",
        "{target}/frontend/dist",
        "--emptyOutDir",
    ],
}];
