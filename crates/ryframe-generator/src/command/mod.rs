mod execution;
mod options;
mod plan;
mod staging;

#[cfg(feature = "schema-import")]
mod import_execution;

pub use plan::{GenerationBuilder, GenerationOperation, GenerationPlan};

const USAGE: &str = "用法：\n  cargo generate -- new-business <模块名或业务包名> [--plan|--write]\n  cargo generate -- resource [--package <crate>] [--models <Model1,Model2>] [--plan|--write] [--sync-frontend|--no-sync-frontend]\n  cargo generate --features schema-import -- import [--connection <name>] [--database <control|tenant>] [--package <crate>] [--tables <table1,table2>] [--plan|--write] [--sync-frontend|--no-sync-frontend]\n  cargo generate -- run [--plan|--write] [--sync-frontend|--no-sync-frontend]\n\n默认预览，仅 --write 更新源码；默认值读取 config/tools.toml 与 .local-tests/tools.toml。run 根据 generator.tables 选择导入或模型生成；导入需要 schema-import feature。";

/// 执行命令行与 Rust Builder 共用的生成流程。
pub fn run_from_args(args: Vec<String>) -> Result<(), String> {
    let Some(command) = args.first().map(String::as_str) else {
        println!("{USAGE}");
        return Ok(());
    };
    if matches!(command, "--help" | "-h" | "help") {
        println!("{USAGE}");
        return Ok(());
    }
    let (operation, rest) = match command {
        "new-business" => {
            let [module, rest @ ..] = &args[1..] else {
                return Err(USAGE.into());
            };
            (GenerationOperation::NewBusiness(module.clone()), rest)
        }
        "resource" => (GenerationOperation::Resource, &args[1..]),
        "import" => (GenerationOperation::Import, &args[1..]),
        "run" => (GenerationOperation::Configured, &args[1..]),
        _ => return Err(format!("未知生成操作：{command}\n{USAGE}")),
    };
    let arguments = options::Arguments::parse_for(&operation, rest)?;
    let plan = arguments
        .builder(options::workspace_root()?, operation)?
        .plan()?;
    if arguments.flag("--plan") {
        println!("{}", plan.describe());
        return Ok(());
    }
    plan.execute()
}
