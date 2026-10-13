use ryframe_config::ToolSettings;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

pub(super) struct Arguments {
    values: BTreeMap<String, String>,
    flags: BTreeSet<String>,
}

impl Arguments {
    pub(super) fn parse(args: &[String]) -> Result<Self, String> {
        Self::parse_values(
            args,
            &["--package", "--model"],
            &["--write", "--plan", "--sync-frontend"],
        )
    }

    pub(super) fn parse_new(args: &[String]) -> Result<Self, String> {
        Self::parse_values(args, &[], &["--write", "--plan"])
    }

    pub(super) fn parse_import(args: &[String]) -> Result<Self, String> {
        Self::parse_values(
            args,
            &["--connection", "--database", "--package", "--table"],
            &["--write", "--plan"],
        )
    }

    fn parse_values(args: &[String], names: &[&str], switches: &[&str]) -> Result<Self, String> {
        let mut parsed = Self {
            values: BTreeMap::new(),
            flags: BTreeSet::new(),
        };
        let mut index = 0;
        while let Some(option) = args.get(index) {
            if switches.contains(&option.as_str()) {
                if !parsed.flags.insert(option.clone()) {
                    return Err(format!("选项不能重复：{option}"));
                }
                index += 1;
            } else if names.contains(&option.as_str()) {
                let value = args
                    .get(index + 1)
                    .filter(|value| !value.starts_with('-') && !value.trim().is_empty())
                    .ok_or_else(|| format!("{option} 缺少取值；请提供非空值"))?;
                if parsed
                    .values
                    .insert(option.clone(), value.clone())
                    .is_some()
                {
                    return Err(format!("选项不能重复：{option}"));
                }
                index += 2;
            } else {
                return Err(format!("未知参数：{option}\n{}", super::USAGE));
            }
        }
        if parsed.flag("--plan") && parsed.flag("--write") {
            return Err("--plan 不能与 --write 同时使用".into());
        }
        Ok(parsed)
    }

    pub(super) fn value(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }
    #[cfg(feature = "schema-import")]
    pub(super) fn required(&self, name: &str) -> Result<&str, String> {
        self.value(name)
            .ok_or_else(|| format!("缺少 {name}\n{}", super::USAGE))
    }
    pub(super) fn flag(&self, name: &str) -> bool {
        self.flags.contains(name)
    }
}

pub(super) fn load_tools() -> Result<(PathBuf, ToolSettings), String> {
    let current = std::env::current_dir().map_err(|error| format!("无法读取当前目录：{error}"))?;
    let root = current
        .ancestors()
        .find(|path| {
            std::fs::read_to_string(path.join("Cargo.toml"))
                .ok()
                .and_then(|text| text.parse::<toml::Table>().ok())
                .is_some_and(|table| table.contains_key("workspace"))
        })
        .ok_or("无法定位后端 Workspace；请在后端仓库或业务 crate 中执行")?
        .to_path_buf();
    let settings = ToolSettings::load(&root).map_err(|error| error.to_string())?;
    Ok((root, settings))
}

pub(super) fn print_plan(operation: &str, parsed: &Arguments) -> Result<(), String> {
    let (_, settings) = load_tools()?;
    let package = parsed
        .value("--package")
        .or(settings.generator.package.as_deref())
        .ok_or("缺少 --package 或 generator.package")?;
    println!("生成计划：操作={operation}，业务包={package}，写入=false");
    if operation == "import" {
        let table = parsed.value("--table").ok_or("缺少 --table")?;
        let scope = parsed
            .value("--database")
            .or(settings.generator.storage.as_deref())
            .ok_or("缺少 --database 或 generator.storage")?;
        if !matches!(scope, "control" | "tenant") {
            return Err("--database 只支持 control 或 tenant".into());
        }
        let source = parsed
            .value("--connection")
            .or(settings.generator.source.as_deref())
            .unwrap_or("local");
        println!(
            "只读结构来源={source}，表={table}，模型目标={scope}；定位业务包 → 读取表结构 → 校验模型 → 预览文件"
        );
    } else {
        println!("定位业务包 → 编译资源描述符 → 校验资源 → 渲染 CRUD 与迁移 → 预览受管文件");
        if parsed.flag("--sync-frontend") || settings.generator.sync_frontend {
            println!("显式写入时：导出本次候选 OpenAPI → 原子同步前端契约及派生文件");
        }
    }
    println!("计划未执行任务，不访问数据库、不编译描述符、不创建缓存或报告。");
    Ok(())
}
