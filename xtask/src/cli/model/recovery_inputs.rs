use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RestoreInputSide {
    Base,
    Candidate,
}

impl RestoreInputSide {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::Candidate => "candidate",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RestoreInputPublication {
    pub(crate) output: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReferenceInputOptions {
    pub(crate) arm_input: PathBuf,
    pub(crate) fresh_target_verify: PathBuf,
    pub(crate) side: RestoreInputSide,
    pub(crate) id: String,
    pub(crate) work_dir: PathBuf,
    pub(crate) publication: Option<RestoreInputPublication>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProductInputOptions {
    pub(crate) reference_plan: PathBuf,
    pub(crate) backup_receipt: PathBuf,
    pub(crate) comparison_sources: PathBuf,
    pub(crate) arm_input: PathBuf,
    pub(crate) fresh_target_verify: PathBuf,
    pub(crate) side: RestoreInputSide,
    pub(crate) id: String,
    pub(crate) fault_at: String,
    pub(crate) publication: Option<RestoreInputPublication>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BindingsInputOptions {
    pub(crate) reference_plan: PathBuf,
    pub(crate) target_plan: PathBuf,
    pub(crate) backup_receipt: PathBuf,
    pub(crate) record: PathBuf,
    pub(crate) publication: Option<RestoreInputPublication>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RecoveryInputsCommand {
    Help,
    Reference(ReferenceInputOptions),
    Product(ProductInputOptions),
    Bindings(BindingsInputOptions),
}

impl RecoveryInputsCommand {
    pub(crate) const fn operation(&self) -> Option<&'static str> {
        Some(match self {
            Self::Help => return None,
            Self::Reference(_) => "reference",
            Self::Product(_) => "product",
            Self::Bindings(_) => "bindings",
        })
    }

    pub(crate) const fn writes(&self) -> bool {
        match self {
            Self::Help => false,
            Self::Reference(options) => options.publication.is_some(),
            Self::Product(options) => options.publication.is_some(),
            Self::Bindings(options) => options.publication.is_some(),
        }
    }
}
