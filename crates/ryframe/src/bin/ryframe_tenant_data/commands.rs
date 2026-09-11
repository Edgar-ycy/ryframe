use super::{
    args::{Command, InventoryTime},
    context::Context,
    output,
};

pub fn validate_outputs(command: &mut Command) -> AppResult<()> {
    match command {
        Command::Begin { output, .. } | Command::VerifyData { output, .. } => {
            *output = output::validate_new_output(output)?;
        }
        _ => {}
    }
    Ok(())
}
use chrono::{DateTime, Utc};
use ryframe_application::ports::backup::*;
use ryframe_config::AppConfig;
use ryframe_kernel::{AppError, AppResult};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    fs::OpenOptions,
    io::{Read, Write},
    path::Path,
};

pub async fn execute(config: &AppConfig, context: &Context, command: Command) -> AppResult<()> {
    match command {
        Command::TargetInventory { target, output } => {
            target_inventory(config, context.databases.as_ref(), &target, &output).await
        }
        Command::Inventory {
            output,
            source_sha,
            observation,
        } => inventory(config, context, &output, source_sha, observation).await,
        Command::Register { manifest, .. } => {
            let manifest: BackupManifest = read_json(&manifest)?;
            if manifest.scope_id != config.scope_id.as_str()
                || manifest.control_schema_fingerprint
                    != ryframe_db::migration::schema_fingerprint()
                || manifest.tenant_schema_fingerprint
                    != ryframe_tenant_db::migration::tenant_data_schema_fingerprint()
            {
                return Err(AppError::Validation(
                    "备份 scope 或 schema 与登记环境不一致".into(),
                ));
            }
            let required = context.repository.required_resources().await?;
            print_json(&context.service.register(manifest, &required).await?)
        }
        Command::Begin { plan, output, .. } => output::publish_result(
            &output,
            context.service.begin_restore(read_json(&plan)?).await,
            "恢复开始记录",
        ),
        Command::VerifyData { id, output, .. } => output::publish_result(
            &output,
            context.service.verify_data(&id).await,
            "恢复数据核验记录",
        ),
        Command::VerifyRuntime {
            id,
            proof,
            tests_receipt,
            runtime_receipt,
            target_plan,
            runner_root,
            ..
        } => {
            let proof = super::proof::verify(
                context.repository.as_ref(),
                &id,
                super::proof::EvidencePaths {
                    proof: &proof,
                    tests: &tests_receipt,
                    runtime: &runtime_receipt,
                    target: &target_plan,
                    runner: &runner_root,
                },
            )
            .await?;
            print_json(&context.service.finish_restore(&id, &proof).await?)
        }
        Command::Status => status(config, context.repository.as_ref()).await,
        Command::Help => Ok(()),
    }
}

pub async fn status(config: &AppConfig, repository: &dyn BackupRepository) -> AppResult<()> {
    let resources = repository.required_resources().await?;
    let now = repository.database_now().await?;
    print_json(
        &repository
            .health(config.scope_id.as_str(), &resources, now)
            .await?,
    )
}

#[derive(Serialize)]
struct Inventory {
    scope_id: String,
    source_sha: String,
    #[serde(flatten)]
    observation: InventoryTime<DateTime<Utc>>,
    captured_at: DateTime<Utc>,
    control_schema_fingerprint: String,
    tenant_schema_fingerprint: String,
    databases: Vec<DatabaseBackup>,
    objects: Vec<ObjectBackup>,
}

async fn inventory(
    config: &AppConfig,
    context: &Context,
    output: &Path,
    source_sha: String,
    observation: InventoryTime<String>,
) -> AppResult<()> {
    if source_sha.len() != 40
        || !source_sha
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(AppError::Validation("source-sha 必须是精确源码 SHA".into()));
    }
    let value = match &observation {
        InventoryTime::QuiescedAt(value) | InventoryTime::ObservedAt(value) => value,
    };
    let started_at = DateTime::parse_from_rfc3339(value)
        .map_err(|_| AppError::Validation("库存观察时间必须是 RFC3339".into()))?
        .with_timezone(&Utc);
    let captured_at = context.repository.database_now().await?;
    if started_at > captured_at {
        return Err(AppError::Validation(
            "库存观察开始时间不能在采集时间之后".into(),
        ));
    }
    let inventory = Inventory {
        scope_id: config.scope_id.as_str().into(),
        source_sha,
        observation: match observation {
            InventoryTime::QuiescedAt(_) => InventoryTime::QuiescedAt(started_at),
            InventoryTime::ObservedAt(_) => InventoryTime::ObservedAt(started_at),
        },
        captured_at,
        control_schema_fingerprint: ryframe_db::migration::schema_fingerprint(),
        tenant_schema_fingerprint: ryframe_tenant_db::migration::tenant_data_schema_fingerprint()
            .into(),
        databases: context.databases.snapshot().await?,
        objects: context.objects.snapshot().await?,
    };
    write_inventory(output, &inventory)?;
    println!(
        "已写入完整只读库存；observed_at 不证明停止或备份资格，正式备份须另行验证 quiesced_at 与全部生产者。"
    );
    Ok(())
}

#[derive(Serialize)]
struct TargetInventory {
    scope_id: String,
    control_schema_fingerprint: String,
    tenant_schema_fingerprint: String,
    target: DatabaseTargetInventory,
}

pub async fn target_inventory(
    config: &AppConfig,
    databases: &dyn BackupDatabaseVerifier,
    target: &str,
    output: &Path,
) -> AppResult<()> {
    let inventory = TargetInventory {
        scope_id: config.scope_id.as_str().into(),
        control_schema_fingerprint: ryframe_db::migration::schema_fingerprint(),
        tenant_schema_fingerprint: ryframe_tenant_db::migration::tenant_data_schema_fingerprint()
            .into(),
        target: databases.target_inventory(target).await?,
    };
    write_inventory(output, &inventory)?;
    println!("已写入显式目标的只读完整表清单；该清单不表示跨目标一致性、备份或恢复成功。");
    Ok(())
}

fn write_inventory(output: &Path, inventory: &impl Serialize) -> AppResult<()> {
    let bytes = serde_json::to_vec_pretty(inventory)
        .map_err(|_| AppError::Internal("备份清单序列化失败".into()))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .map_err(|_| AppError::Validation("清单输出文件必须是不存在的新文件".into()))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| AppError::Internal("备份清单写入失败".into()))?;
    Ok(())
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> AppResult<T> {
    const MAX_BYTES: u64 = 16 * 1024 * 1024;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(MAX_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|_| AppError::Validation("备份或演练清单无法读取".into()))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(AppError::Validation("备份或演练清单不能超过 16 MiB".into()));
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| AppError::Validation(format!("清单格式无效：{error}")))
}

fn print_json(value: &impl Serialize) -> AppResult<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(value)
            .map_err(|_| AppError::Internal("无法序列化维护结果".into()))?
    );
    Ok(())
}
