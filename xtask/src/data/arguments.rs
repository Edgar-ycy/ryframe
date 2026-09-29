use std::path::Path;

use crate::cli::{
    BackupCommand, DataCommand, FILE_APPLY_CONFIRMATION, FileMaintenanceMode,
    FileMaintenanceOptions, ResetCommand, RestoreCommand, TargetInventoryOptions,
};

pub(super) struct DataInvocation {
    pub(super) feature: &'static str,
    pub(super) binary: &'static str,
    pub(super) arguments: Vec<String>,
}

pub(super) fn invocation(command: &DataCommand) -> Option<DataInvocation> {
    match command {
        DataCommand::Backup(command) => Some(tenant_data(backup_arguments(command))),
        DataCommand::Restore(command) => Some(tenant_data(restore_arguments(command))),
        DataCommand::TargetInventory(options) => {
            Some(tenant_data(target_inventory_arguments(options)))
        }
        DataCommand::File(options) => Some(DataInvocation {
            feature: "bin-file-maintenance",
            binary: "ryframe-file-maintenance",
            arguments: file_arguments(options),
        }),
        DataCommand::Reset(command) => Some(DataInvocation {
            feature: "bin-reset",
            binary: "ryframe-reset",
            arguments: reset_arguments(command),
        }),
        DataCommand::Help | DataCommand::Migrate(_) | DataCommand::PerformanceIdentities(_) => None,
    }
}

fn tenant_data(arguments: Vec<String>) -> DataInvocation {
    DataInvocation {
        feature: "bin-tenant-data",
        binary: "ryframe-tenant-data",
        arguments,
    }
}

pub(crate) fn backup_arguments(command: &BackupCommand) -> Vec<String> {
    match command {
        BackupCommand::Inventory(options) => {
            let (observation_flag, observation_value) = options.observation.flag_and_value();
            vec![
                "backup-inventory".into(),
                "--output".into(),
                path_argument(&options.output),
                "--source-sha".into(),
                options.source_sha.clone(),
                observation_flag.into(),
                observation_value.into(),
            ]
        }
        BackupCommand::Register(options) => vec![
            "backup-register".into(),
            "--manifest".into(),
            path_argument(&options.manifest),
            "--backup-root".into(),
            path_argument(&options.backup_root),
        ],
        BackupCommand::Status => vec!["backup-status".into()],
    }
}

pub(crate) fn restore_arguments(command: &RestoreCommand) -> Vec<String> {
    match command {
        RestoreCommand::Begin(options) => vec![
            "restore-begin".into(),
            "--plan".into(),
            path_argument(&options.plan),
            "--output".into(),
            path_argument(&options.output),
            "--restore-config-dir".into(),
            path_argument(&options.restore_config_dir),
        ],
        RestoreCommand::VerifyData(options) => vec![
            "restore-verify-data".into(),
            "--id".into(),
            options.id.clone(),
            "--backup-root".into(),
            path_argument(&options.backup_root),
            "--output".into(),
            path_argument(&options.output),
            "--restore-config-dir".into(),
            path_argument(&options.restore_config_dir),
        ],
        RestoreCommand::Verify(options) => vec![
            "restore-verify".into(),
            "--id".into(),
            options.id.clone(),
            "--proof".into(),
            path_argument(&options.proof),
            "--tests-receipt".into(),
            path_argument(&options.tests_receipt),
            "--runtime-receipt".into(),
            path_argument(&options.runtime_receipt),
            "--target-plan".into(),
            path_argument(&options.target_plan),
            "--runner-root".into(),
            path_argument(&options.runner_root),
            "--restore-config-dir".into(),
            path_argument(&options.restore_config_dir),
        ],
    }
}

pub(crate) fn target_inventory_arguments(options: &TargetInventoryOptions) -> Vec<String> {
    vec![
        "target-inventory".into(),
        "--target".into(),
        options.target.clone(),
        "--output".into(),
        path_argument(&options.output),
    ]
}

pub(crate) fn file_arguments(options: &FileMaintenanceOptions) -> Vec<String> {
    let mut arguments = vec![
        options.operation.as_str().into(),
        options.mode.as_str().into(),
        "--database".into(),
        options.database.clone(),
        "--batch-size".into(),
        options.batch_size.to_string(),
        "--start-after".into(),
        options.start_after.to_string(),
    ];
    if options.mode == FileMaintenanceMode::Apply {
        arguments.extend(["--confirm-apply".into(), FILE_APPLY_CONFIRMATION.into()]);
    }
    arguments
}

pub(crate) fn reset_arguments(command: &ResetCommand) -> Vec<String> {
    match command {
        ResetCommand::Plan => vec!["plan".into()],
        ResetCommand::Execute(options) => vec![
            "execute".into(),
            "--plan-hash".into(),
            options.plan_hash.clone(),
            "--confirm-reset".into(),
            options.confirmation.clone(),
        ],
    }
}

fn path_argument(path: &Path) -> String {
    path.to_str()
        .expect("CLI 已从 Unicode 参数构造并校验路径")
        .to_owned()
}
