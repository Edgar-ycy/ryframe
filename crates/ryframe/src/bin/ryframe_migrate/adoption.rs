use std::{error::Error, fs, path::Path};

use ryframe_sdk::{BusinessAdoptionPlan, BusinessMigrationScope, RyFrameBusinessModule};
use sea_orm::DatabaseConnection;

#[path = "adoption/args.rs"]
mod args;
#[path = "adoption/workflow.rs"]
mod workflow;
use args::Action;
pub(super) use args::{Options, parse};

pub(super) async fn run(directory: &Path, options: Options) -> Result<(), Box<dyn Error>> {
    workflow::run(directory, options).await
}

async fn perform(
    options: &Options,
    saved: Option<&BusinessAdoptionPlan>,
    scope: BusinessMigrationScope,
    database: &DatabaseConnection,
    modules: &[RyFrameBusinessModule],
) -> Result<(), Box<dyn Error>> {
    let migrations = modules
        .iter()
        .flat_map(|module| module.migrations())
        .filter(|migration| migration.scope() == scope)
        .map(|migration| migration.as_ref())
        .collect::<Vec<_>>();
    match &options.action {
        Action::Preview { module, output } => {
            let plan = ryframe_sdk::preview_business_adoption(database, scope, module, &migrations)
                .await?;
            let bytes = serde_json::to_vec_pretty(&plan)?;
            if let Some(output) = output {
                let output = super::private_artifact::output(output)?;
                super::private_artifact::publish(&output, &bytes)?;
                println!(
                    "接管计划已保存：{}；数据库零写入。审阅后显式 --apply <原计划> --write",
                    output.display()
                );
            } else {
                println!("{}", String::from_utf8(bytes)?);
            }
        }
        Action::Apply(_) => {
            let plan = saved.ok_or("接管缺少已读取的原计划")?;
            let outcome = ryframe_sdk::apply_business_adoption(database, plan, &migrations).await?;
            println!("{}", serde_json::to_string_pretty(&outcome)?);
        }
        Action::Inspect(_) => {
            let plan = saved.ok_or("核验缺少已读取的原计划")?;
            let status =
                ryframe_sdk::inspect_business_adoption(database, plan, &migrations).await?;
            println!("{}", serde_json::to_string_pretty(&status)?);
        }
    }
    Ok(())
}

fn read_plan(path: &Path) -> Result<BusinessAdoptionPlan, Box<dyn Error>> {
    let path = super::private_artifact::input(path)?;
    if fs::metadata(&path)?.len() > 1_048_576 {
        return Err("接管计划超过 1 MiB，拒绝读取；核对是否选错文件，尚未写入".into());
    }
    serde_json::from_slice(&fs::read(&path)?).map_err(|error| {
        format!(
            "接管计划 {} 格式错误：{error}；重新选择原始计划，尚未连接数据库",
            path.display()
        )
        .into()
    })
}
