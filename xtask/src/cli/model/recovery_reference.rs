use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExistingReferenceSide {
    Source,
    Target,
}

impl ExistingReferenceSide {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Target => "target",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReferenceTargetInputs {
    pub(crate) backup_receipt: PathBuf,
    pub(crate) comparison_sources: PathBuf,
    pub(crate) arm_input: PathBuf,
    pub(crate) fresh_target_verify: PathBuf,
    pub(crate) product_plan: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReferencePlanAction {
    Summary,
    Verify {
        target_plan: PathBuf,
    },
    Preview {
        inputs: ReferenceTargetInputs,
    },
    Publish {
        inputs: ReferenceTargetInputs,
        output: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReferencePlanOptions {
    pub(crate) plan: PathBuf,
    pub(crate) action: ReferencePlanAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RecoveryReferenceCommand {
    Help,
    Plan(ReferencePlanOptions),
    CheckDataset {
        plan: PathBuf,
    },
    CheckExisting {
        plan: PathBuf,
        side: ExistingReferenceSide,
    },
    Dataset {
        plan: PathBuf,
    },
    Backup {
        plan: PathBuf,
        inventory: PathBuf,
        source_generation: PathBuf,
        source_export_result: PathBuf,
    },
    Restore {
        plan: PathBuf,
        backup_root: PathBuf,
        record: PathBuf,
        target_plan: PathBuf,
        runtime_registration: PathBuf,
    },
    Copy {
        plan: PathBuf,
        backup_root: PathBuf,
        copy_id: String,
    },
    Damage {
        plan: PathBuf,
        backup_root: PathBuf,
        artifact: String,
        missing: bool,
    },
}

impl RecoveryReferenceCommand {
    pub(crate) const fn operation(&self) -> Option<&'static str> {
        Some(match self {
            Self::Help => return None,
            Self::Plan(_) => "plan",
            Self::CheckDataset { .. } => "check-dataset",
            Self::CheckExisting { .. } => "check-existing",
            Self::Dataset { .. } => "dataset",
            Self::Backup { .. } => "backup",
            Self::Restore { .. } => "restore",
            Self::Copy { .. } => "copy",
            Self::Damage { .. } => "damage",
        })
    }

    pub(crate) const fn writes(&self) -> bool {
        matches!(
            self,
            Self::Plan(ReferencePlanOptions {
                action: ReferencePlanAction::Publish { .. },
                ..
            }) | Self::Dataset { .. }
                | Self::Backup { .. }
                | Self::Restore { .. }
                | Self::Copy { .. }
                | Self::Damage { .. }
        )
    }
}
