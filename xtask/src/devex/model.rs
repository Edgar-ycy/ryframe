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

    pub(super) const fn definition(self) -> SuiteDefinition {
        match self {
            Self::RustColdBuild => SuiteDefinition {
                steps: RUST_COLD_BUILD,
                features: &["workspace-default"],
                environment: &[("CARGO_INCREMENTAL", "0")],
                remove_environment: &["RUSTC_WRAPPER", "SCCACHE_RECACHE"],
                requirement: SuiteRequirement::Ready,
            },
            Self::RustIncremental => SuiteDefinition {
                steps: RUST_INCREMENTAL,
                features: &["workspace-default"],
                environment: &[("CARGO_INCREMENTAL", "1")],
                remove_environment: &[],
                requirement: SuiteRequirement::Ready,
            },
            Self::CargoDevSave => SuiteDefinition {
                steps: CARGO_DEV_SAVE,
                features: &[],
                environment: &[],
                remove_environment: &[],
                requirement: SuiteRequirement::Pending(
                    "`cargo dev --measure-once` 尚未提供一次性保存反馈探针；拒绝把常驻 watcher 误记为完成样本",
                ),
            },
            Self::ResourceGenerator => SuiteDefinition {
                steps: RESOURCE_GENERATOR,
                features: &["resource"],
                environment: &[],
                remove_environment: &[],
                requirement: SuiteRequirement::FrontendFiles,
            },
            Self::ResourceGate => SuiteDefinition {
                steps: RESOURCE_GATE,
                features: &["resource"],
                environment: &[],
                remove_environment: &[],
                requirement: SuiteRequirement::Pending(
                    "`cargo xtask ci resource-gate` 尚未落地；本阶段只登记测量面，不提前启用门禁",
                ),
            },
            Self::RustSccache => SuiteDefinition {
                steps: RUST_SCCACHE,
                features: &["workspace-default"],
                environment: &[("CARGO_INCREMENTAL", "0"), ("RUSTC_WRAPPER", "sccache")],
                remove_environment: &[],
                requirement: SuiteRequirement::Executable("sccache"),
            },
            Self::FrontendFast => SuiteDefinition {
                steps: FRONTEND_FAST,
                features: &[],
                environment: &[("CI", "true"), ("FORCE_COLOR", "0"), ("NO_COLOR", "1")],
                remove_environment: &[],
                requirement: SuiteRequirement::Frontend,
            },
            Self::FrontendBuild => SuiteDefinition {
                steps: FRONTEND_BUILD,
                features: &[],
                environment: &[("CI", "true"), ("FORCE_COLOR", "0"), ("NO_COLOR", "1")],
                remove_environment: &[],
                requirement: SuiteRequirement::Frontend,
            },
        }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DevexCommand {
    Run(DevexRunOptions),
    Summarize { run: String },
    Compare { baseline: String, candidate: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WorkingDirectory {
    Backend,
    Frontend,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct StepDefinition {
    pub(super) working_directory: WorkingDirectory,
    pub(super) program: &'static str,
    pub(super) args: &'static [&'static str],
}

#[derive(Debug, Clone, Copy)]
pub(super) enum SuiteRequirement {
    Ready,
    Frontend,
    FrontendFiles,
    Executable(&'static str),
    Pending(&'static str),
}

#[derive(Debug, Clone, Copy)]
pub(super) struct SuiteDefinition {
    pub(super) steps: &'static [StepDefinition],
    pub(super) features: &'static [&'static str],
    pub(super) environment: &'static [(&'static str, &'static str)],
    pub(super) remove_environment: &'static [&'static str],
    pub(super) requirement: SuiteRequirement,
}

const RUST_COLD_BUILD: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Backend,
    program: "cargo",
    args: &["build", "--locked", "--workspace"],
}];

const RUST_INCREMENTAL: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Backend,
    program: "cargo",
    args: &["check", "--locked", "--workspace"],
}];

const CARGO_DEV_SAVE: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Backend,
    program: "cargo",
    args: &["dev", "--measure-once"],
}];

const RESOURCE_GENERATOR: &[StepDefinition] = &[StepDefinition {
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

const RESOURCE_GATE: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Backend,
    program: "cargo",
    args: &["xtask", "ci", "resource-gate"],
}];

const RUST_SCCACHE: &[StepDefinition] = &[StepDefinition {
    working_directory: WorkingDirectory::Backend,
    program: "cargo",
    args: &["check", "--locked", "--workspace"],
}];

const FRONTEND_FAST: &[StepDefinition] = &[
    StepDefinition {
        working_directory: WorkingDirectory::Frontend,
        program: "corepack",
        args: &[
            "pnpm",
            "exec",
            "prettier",
            "--check",
            ".",
            "--cache",
            "--cache-location",
            "{target}/frontend/prettier/cache",
            "--cache-strategy",
            "content",
        ],
    },
    StepDefinition {
        working_directory: WorkingDirectory::Frontend,
        program: "node",
        args: &["scripts/check-source-size.mjs"],
    },
    StepDefinition {
        working_directory: WorkingDirectory::Frontend,
        program: "node",
        args: &["scripts/check-import-boundaries.mjs"],
    },
    StepDefinition {
        working_directory: WorkingDirectory::Frontend,
        program: "corepack",
        args: &[
            "pnpm",
            "exec",
            "eslint",
            ".",
            "--max-warnings=0",
            "--cache",
            "--cache-location",
            "{target}/frontend/eslint/cache",
        ],
    },
    StepDefinition {
        working_directory: WorkingDirectory::Frontend,
        program: "corepack",
        args: &[
            "pnpm",
            "exec",
            "stylelint",
            "src/**/*.{css,scss,vue}",
            "--max-warnings=0",
            "--cache",
            "--cache-location",
            "{target}/frontend/stylelint/cache",
        ],
    },
    StepDefinition {
        working_directory: WorkingDirectory::Frontend,
        program: "corepack",
        args: &["pnpm", "run", "typecheck:app"],
    },
    StepDefinition {
        working_directory: WorkingDirectory::Frontend,
        program: "corepack",
        args: &["pnpm", "run", "test:unit"],
    },
];

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
