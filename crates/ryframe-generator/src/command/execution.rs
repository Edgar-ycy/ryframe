use std::{path::Path, process::Command};

use super::{GenerationOperation, GenerationPlan};
use crate::business::{
    BusinessBootstrapOptions, BusinessGenerateOptions, bootstrap_business_package,
    generate_business_package,
};

impl GenerationPlan {
    /// 按已解析计划执行；默认预览，只有 Builder 或 CLI 显式 write 才修改目标。
    pub fn execute(&self) -> Result<(), String> {
        self.validate()?;
        match &self.operation {
            GenerationOperation::NewBusiness(module) => {
                let report = bootstrap_business_package(BusinessBootstrapOptions {
                    workspace_root: &self.workspace_root,
                    module,
                    write: self.write,
                })
                .map_err(|error| error.to_string())?;
                print_changes("create", &report.created);
                print_changes("update", &report.updated);
            }
            GenerationOperation::Resource => {
                let report = generate_business_package(BusinessGenerateOptions {
                    current_dir: &self.workspace_root,
                    package: &self.package,
                    models: &self.models,
                    write: self.write,
                })
                .map_err(|error| error.to_string())?;
                print_changes("create", &report.created);
                print_changes("update", &report.updated);
                print_changes("remove", &report.removed);
                println!("未变化 {}。", report.unchanged.len());
                if self.sync_frontend && self.write {
                    sync_frontend(&self.workspace_root, &self.settings)?;
                }
            }
            GenerationOperation::Import => self.execute_import()?,
            GenerationOperation::Configured => unreachable!("构建计划已解析配置操作"),
        }
        println!(
            "{}完成。",
            if self.write {
                "生成写入"
            } else {
                "生成预览"
            }
        );
        Ok(())
    }

    #[cfg(feature = "schema-import")]
    fn execute_import(&self) -> Result<(), String> {
        tokio::runtime::Runtime::new()
            .map_err(|error| format!("无法创建导入运行时：{error}"))?
            .block_on(super::import_execution::execute(self))
    }

    #[cfg(not(feature = "schema-import"))]
    fn execute_import(&self) -> Result<(), String> {
        Err("数据库导入需要 schema-import：运行 cargo generate --features schema-import -- import 或 run；Rust 示例使用 --features schema-import --example generate".into())
    }
}

fn print_changes(kind: &str, paths: &[String]) {
    for path in paths {
        println!("{kind} {path}");
    }
}

fn sync_frontend(workspace: &Path, settings: &ryframe_config::ToolSettings) -> Result<(), String> {
    let frontend = settings.frontend_dir(workspace);
    if !frontend.join("package.json").is_file() {
        return Err(format!(
            "前端目录无效：{}；请配置 workspace.frontend_dir；后端生成已完成",
            frontend.display()
        ));
    }
    let status = Command::new("cargo")
        .args(["xtask", "generate", "api", "--write", "--frontend-dir"])
        .arg(frontend)
        .current_dir(workspace)
        .status()
        .map_err(|error| {
            format!("无法启动候选契约同步：{error}；请确认 Cargo 与前端工具链可用；后端生成已完成")
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "候选契约同步失败：{status}；后端生成已完成，修复上述错误后重新执行同步"
        ))
    }
}
