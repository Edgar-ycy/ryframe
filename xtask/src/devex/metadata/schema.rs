use std::{collections::BTreeMap, path::Path};

use serde::{Deserialize, Serialize};

use super::super::model::{
    CacheState, DevexRunOptions, DevexSuite, PairingMetadata, SuiteDefinition,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct RunMetadata {
    pub(crate) schema_version: u8,
    pub(crate) run_id: String,
    pub(crate) started_at: String,
    pub(crate) suite: DevexSuite,
    pub(crate) variant: String,
    pub(crate) cache_state: CacheState,
    pub(crate) requested_runs: usize,
    pub(crate) backend: SourceState,
    pub(crate) frontend: Option<SourceState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) runner_frontend: Option<SourceState>,
    pub(crate) toolchain: Toolchain,
    pub(crate) target: String,
    pub(crate) features: Vec<String>,
    pub(crate) jobs: usize,
    pub(crate) environment: BTreeMap<String, String>,
    pub(crate) environment_hash: String,
    pub(crate) commands: Vec<CommandMetadata>,
    pub(crate) compile_surface_fingerprint: String,
    #[serde(default)]
    pub(crate) input_fingerprint: String,
    #[serde(default)]
    pub(crate) pairing: Option<PairingMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct SourceState {
    pub(crate) commit: Option<String>,
    pub(crate) dirty: Option<bool>,
    #[serde(default)]
    pub(crate) worktree_fingerprint: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct SourceFingerprints {
    pub(crate) backend: String,
    pub(crate) frontend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) runner_frontend: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct Toolchain {
    pub(crate) cargo: String,
    pub(crate) rustc: String,
    #[serde(default)]
    pub(crate) sccache: Option<String>,
    pub(crate) node: Option<String>,
    pub(crate) pnpm: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct CommandMetadata {
    pub(crate) working_directory: String,
    pub(crate) program: String,
    pub(crate) args: Vec<String>,
}

pub(crate) struct MetadataContext<'a> {
    pub(crate) backend_root: &'a Path,
    pub(crate) frontend_root: &'a Path,
    pub(crate) runner_frontend_root: &'a Path,
    pub(crate) devex_root: &'a Path,
    pub(crate) run_id: &'a str,
    pub(crate) options: &'a DevexRunOptions,
    pub(crate) definition: SuiteDefinition,
    pub(crate) effective_environment: &'a BTreeMap<String, String>,
    pub(crate) pairing: Option<PairingMetadata>,
}
