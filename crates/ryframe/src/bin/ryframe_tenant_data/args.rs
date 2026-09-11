use std::{collections::BTreeMap, path::PathBuf};

pub const USAGE: &str = "用法：
  ryframe-tenant-data target-inventory --target <已配置目标key> --output <target.json>
  ryframe-tenant-data backup-inventory --output <inventory.json> --source-sha <SHA> --quiesced-at <RFC3339>
  ryframe-tenant-data backup-register --manifest <manifest.json> --backup-root <目录>
  ryframe-tenant-data backup-status
  ryframe-tenant-data restore-begin --plan <plan.json> --restore-config-dir <目录>
  ryframe-tenant-data restore-verify-data --id <演练ID> --backup-root <目录> --restore-config-dir <目录>
  ryframe-tenant-data restore-verify --id <演练ID> --proof <business-proof.json> --tests-receipt <tests.json> --restore-config-dir <目录>

登记库使用 APP_CONFIG_DIR。恢复目标使用独立配置目录；APP_* 覆盖仍生效，最终 scope、物理身份、JWT 与 Redis namespace 必须隔离。
备份复制和数据库恢复由外部工具执行。inventory 与 verify 只读目标资源；begin、register 和 verify 的状态写入登记库。
target-inventory 覆盖未分配租户的明确目标及其迁移、ownership、登记表，不访问对象存储；跨目标一致性仍要求外部停止全部生产者。";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Help,
    TargetInventory {
        target: String,
        output: PathBuf,
    },
    Inventory {
        output: PathBuf,
        source_sha: String,
        quiesced_at: String,
    },
    Register {
        manifest: PathBuf,
        root: PathBuf,
    },
    Status,
    Begin {
        plan: PathBuf,
        config: PathBuf,
    },
    VerifyData {
        id: String,
        root: PathBuf,
        config: PathBuf,
    },
    VerifyRuntime {
        id: String,
        proof: PathBuf,
        tests_receipt: PathBuf,
        config: PathBuf,
    },
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let mut args = args.into_iter();
    let command = args.next().ok_or(USAGE)?;
    let mut flags = BTreeMap::new();
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .filter(|value| !value.starts_with("--") && !value.trim().is_empty())
            .ok_or(USAGE)?;
        if !flag.starts_with("--") || flags.insert(flag, value).is_some() {
            return Err("参数必须明确且不能重复".into());
        }
    }
    let mut take = |key: &str| {
        flags
            .remove(key)
            .ok_or_else(|| format!("缺少 {key}\n{USAGE}"))
    };
    let result = match command.as_str() {
        "--help" | "-h" => Command::Help,
        "target-inventory" => Command::TargetInventory {
            target: take("--target")?,
            output: take("--output")?.into(),
        },
        "backup-inventory" => Command::Inventory {
            output: take("--output")?.into(),
            source_sha: take("--source-sha")?,
            quiesced_at: take("--quiesced-at")?,
        },
        "backup-register" => Command::Register {
            manifest: take("--manifest")?.into(),
            root: take("--backup-root")?.into(),
        },
        "backup-status" => Command::Status,
        "restore-begin" => Command::Begin {
            plan: take("--plan")?.into(),
            config: take("--restore-config-dir")?.into(),
        },
        "restore-verify-data" => Command::VerifyData {
            id: take("--id")?,
            root: take("--backup-root")?.into(),
            config: take("--restore-config-dir")?.into(),
        },
        "restore-verify" => Command::VerifyRuntime {
            id: take("--id")?,
            proof: take("--proof")?.into(),
            tests_receipt: take("--tests-receipt")?.into(),
            config: take("--restore-config-dir")?.into(),
        },
        _ => return Err(USAGE.into()),
    };
    if !flags.is_empty() {
        return Err("存在不适用于当前命令的参数".into());
    }
    Ok(result)
}

impl Command {
    pub fn restore_config(&self) -> Option<&PathBuf> {
        match self {
            Self::Begin { config, .. }
            | Self::VerifyData { config, .. }
            | Self::VerifyRuntime { config, .. } => Some(config),
            _ => None,
        }
    }

    pub fn backup_root(&self) -> PathBuf {
        match self {
            Self::Register { root, .. } | Self::VerifyData { root, .. } => root.clone(),
            _ => PathBuf::new(),
        }
    }
}
