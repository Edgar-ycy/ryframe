use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use super::{GenerationBuilder, GenerationOperation};

pub(super) struct Arguments {
    values: BTreeMap<String, String>,
    flags: BTreeSet<String>,
}

impl Arguments {
    pub(super) fn parse_for(
        operation: &GenerationOperation,
        args: &[String],
    ) -> Result<Self, String> {
        let (names, switches): (&[&str], &[&str]) = match operation {
            GenerationOperation::NewBusiness(_) => (&[], &["--write", "--plan"]),
            GenerationOperation::Resource => (
                &["--package", "--model", "--models"],
                &["--write", "--plan", "--sync-frontend"],
            ),
            GenerationOperation::Import => (
                &[
                    "--connection",
                    "--database",
                    "--package",
                    "--table",
                    "--tables",
                    "--strip-table-prefix",
                ],
                &["--write", "--plan"],
            ),
            GenerationOperation::Configured => (&[], &["--write", "--plan"]),
        };
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
        for (single, multiple) in [("--table", "--tables"), ("--model", "--models")] {
            if parsed.values.contains_key(single) && parsed.values.contains_key(multiple) {
                return Err(format!(
                    "{single} 与 {multiple} 不能同时使用；请选择一种输入"
                ));
            }
        }
        Ok(parsed)
    }

    pub(super) fn builder(
        &self,
        root: PathBuf,
        operation: GenerationOperation,
    ) -> Result<GenerationBuilder, String> {
        let mut builder = GenerationBuilder::new(root)
            .operation(operation)
            .write(self.flag("--write"));
        for (name, value) in &self.values {
            builder = match name.as_str() {
                "--package" => builder.package(value),
                "--connection" => builder.source(value),
                "--database" => builder.database(value),
                "--strip-table-prefix" => builder.strip_table_prefix(value),
                "--table" => builder.include_tables([value]),
                "--tables" => builder.include_tables(list(value)?),
                "--model" => builder.include_models([value]),
                "--models" => builder.include_models(list(value)?),
                _ => unreachable!("解析时已验证全部选项"),
            };
        }
        if self.flag("--sync-frontend") {
            builder = builder.sync_frontend(true);
        }
        Ok(builder)
    }

    pub(super) fn flag(&self, name: &str) -> bool {
        self.flags.contains(name)
    }
}

fn list(value: &str) -> Result<Vec<String>, String> {
    let values = value.split(',').map(str::trim).collect::<Vec<_>>();
    if values.iter().any(|value| value.is_empty()) {
        return Err("批量选择中包含空名称；使用 table1,table2 或 Model1,Model2".into());
    }
    Ok(values.into_iter().map(str::to_owned).collect())
}

pub(super) fn workspace_root() -> Result<PathBuf, String> {
    let current = std::env::current_dir().map_err(|error| format!("无法读取当前目录：{error}"))?;
    current
        .ancestors()
        .find(|path| {
            std::fs::read_to_string(path.join("Cargo.toml"))
                .ok()
                .and_then(|text| text.parse::<toml::Table>().ok())
                .is_some_and(|table| table.contains_key("workspace"))
        })
        .map(std::path::Path::to_path_buf)
        .ok_or_else(|| "无法定位后端 Workspace；请在后端仓库或业务 crate 中执行".into())
}
