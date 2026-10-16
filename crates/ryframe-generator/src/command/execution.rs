use super::{GenerationOperation, GenerationPlan};
use crate::business::{BusinessBootstrapOptions, bootstrap_business_package};

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
                super::staging::execute(self, None)?;
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
