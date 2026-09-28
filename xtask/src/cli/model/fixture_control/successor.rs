use std::path::PathBuf;

use super::FixtureSide;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FixtureSuccessorCommand {
    Help,
    Relationship(FixtureSuccessorRelationship),
    GenerationRequest(FixtureSuccessorGeneration),
    ArmRequest(FixtureSuccessorArm),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FixtureSuccessorRelationship {
    pub(crate) source_result: PathBuf,
    pub(crate) predecessor_review: PathBuf,
    pub(crate) predecessor_request: PathBuf,
    pub(crate) successor_review: PathBuf,
    pub(crate) seed_request: PathBuf,
    pub(crate) base_request: PathBuf,
    pub(crate) candidate_request: PathBuf,
    pub(crate) id: String,
    pub(crate) output: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FixtureSuccessorGeneration {
    pub(crate) successor: PathBuf,
    pub(crate) source_backend: PathBuf,
    pub(crate) expected_head: String,
    pub(crate) backend_build: PathBuf,
    pub(crate) maintenance_build: PathBuf,
    pub(crate) source_environment: PathBuf,
    pub(crate) id: String,
    pub(crate) adapter_contract: Option<String>,
    pub(crate) product_backend: Option<PathBuf>,
    pub(crate) output: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FixtureSuccessorArm {
    pub(crate) successor: PathBuf,
    pub(crate) source_export_result: PathBuf,
    pub(crate) workspace: PathBuf,
    pub(crate) id: String,
    pub(crate) side: FixtureSide,
    pub(crate) copy_directory: PathBuf,
    pub(crate) output: Option<PathBuf>,
}

impl FixtureSuccessorCommand {
    pub(crate) const fn operation(&self) -> &'static str {
        match self {
            Self::Help => "help",
            Self::Relationship(_) => "relationship",
            Self::GenerationRequest(_) => "generation-request",
            Self::ArmRequest(_) => "arm-request",
        }
    }

    pub(crate) const fn writes(&self) -> bool {
        match self {
            Self::Help => false,
            Self::Relationship(_) => true,
            Self::GenerationRequest(options) => options.output.is_some(),
            Self::ArmRequest(options) => options.output.is_some(),
        }
    }
}
