//! 前端 v3 收据与配置来源的精确公开结构。
use super::{fail, source};
use crate::Result;
use serde_json::{Value, json};
pub(super) const TOOLS: [&str; 2] = ["config/tools.json", ".local-tests/frontend-tools.json"];
pub(super) const IDENTITY: &str = ".local-tests/build/source.json";

pub(super) fn exact(value: &Value, names: &[&str]) -> Result<()> {
    let object = value.as_object().ok_or_else(|| fail("字段必须为对象"))?;
    if object.len() != names.len() || names.iter().any(|name| !object.contains_key(*name)) {
        return Err(fail("字段缺失或包含未知字段"));
    }
    Ok(())
}
pub(super) fn text(value: &Value) -> Result<&str> {
    value.as_str().ok_or_else(|| fail("字段必须是字符串"))
}
pub(super) fn hex(value: &Value, len: usize) -> bool {
    value.as_str().is_some_and(|text| {
        text.len() == len
            && text
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
pub(super) fn number(value: &Value, max: u64) -> Option<u64> {
    if let Some(value) = value.as_u64() {
        return (value <= max).then_some(value);
    }
    let value = value.as_f64()?;
    (value.is_finite() && value >= 0.0 && value <= max as f64 && value.fract() == 0.0)
        .then_some(value as u64)
}
pub(super) fn relative(text: &str, colon: bool) -> bool {
    !text.is_empty()
        && !text.starts_with('/')
        && !text.contains(['\\', '\0', '\r', '\n'])
        && (!colon || !text.contains(':'))
        && text.split('/').all(|part| !matches!(part, "" | "." | ".."))
}
fn path_value(value: &Value) -> bool {
    value.as_str().is_some_and(|text| {
        relative(text, true)
            || (std::path::Path::new(text).is_absolute()
                && !text.contains(['\\', '\0', '\r', '\n'])
                && text
                    .split('/')
                    .skip(1)
                    .all(|part| !matches!(part, "." | ".."))
                && !text.contains("//")
                && (text == "/" || !text.ends_with('/')))
    })
}
fn descriptors(value: &Value) -> Result<Vec<&str>> {
    let files = value
        .as_array()
        .ok_or_else(|| fail("来源必须是有序文件数组"))?;
    let mut names = Vec::new();
    for file in files {
        exact(file, &["path", "sha256"])?;
        if !path_value(&file["path"]) || !hex(&file["sha256"], 64) {
            return Err(fail("配置来源路径或 SHA256 无效"));
        }
        let name = text(&file["path"])?;
        if names.contains(&name) {
            return Err(fail("配置来源重复"));
        }
        names.push(name);
    }
    Ok(names)
}
pub(crate) fn context(value: &Value) -> Result<()> {
    exact(
        value,
        &[
            "command",
            "mode",
            "target",
            "toolchain",
            "configuration",
            "identity",
        ],
    )?;
    let tool = &value["toolchain"];
    exact(tool, &["node", "pnpm", "vite"])?;
    exact(&tool["pnpm"], &["pinned", "observed"])?;
    if value["command"] != json!(["vite", "build"])
        || value["mode"] != "production"
        || value["target"] != "vite-default"
        || text(&tool["node"])?.is_empty()
        || text(&tool["vite"])?.is_empty()
        || text(&tool["pnpm"]["pinned"])?.is_empty()
        || tool["pnpm"]["observed"] != tool["pnpm"]["pinned"]
    {
        return Err(fail("构建命令、模式或工具链无效"));
    }
    let config = &value["configuration"];
    exact(config, &["environment", "directory", "files", "tools"])?;
    if !matches!(
        config["environment"].as_str(),
        Some("dev" | "test" | "prod")
    ) || config["directory"] != "." && !path_value(&config["directory"])
    {
        return Err(fail("APP_ENV 或规范配置目录无效"));
    }
    let prefix = if config["directory"] == "." {
        String::new()
    } else {
        format!("{}/", text(&config["directory"])?.trim_end_matches('/'))
    };
    let selected = format!("{prefix}app.{}.toml", text(&config["environment"])?);
    let common = format!("{prefix}app.toml");
    let files = descriptors(&config["files"])?;
    if files != vec![selected.as_str()] && files != vec![common.as_str(), selected.as_str()] {
        return Err(fail("配置必须按公共、所选环境文件顺序绑定"));
    }
    let tools = descriptors(&config["tools"])?;
    if tools
        != TOOLS
            .into_iter()
            .filter(|file| tools.contains(file))
            .collect::<Vec<_>>()
    {
        return Err(fail("工具来源或优先顺序无效"));
    }
    identity(&value["identity"])
}
fn identity(value: &Value) -> Result<()> {
    exact(value, &["frontend_commit", "source_file"])?;
    if value["frontend_commit"] != "development" && !hex(&value["frontend_commit"], 40) {
        return Err(fail("frontend_commit 必须是 development 或完整小写 SHA"));
    }
    if value["source_file"].is_null() {
        if value["frontend_commit"] != "development" {
            return Err(fail("正式身份缺少受管 source.json"));
        }
    } else if descriptors(&json!([value["source_file"]]))? != vec![IDENTITY] {
        return Err(fail("身份来源不是受管 source.json"));
    }
    Ok(())
}
pub(crate) fn receipt(value: &Value) -> Result<()> {
    exact(
        value,
        &["format_version", "kind", "sources", "build", "files"],
    )?;
    if number(&value["format_version"], 3) != Some(3) || value["kind"] != "restore-frontend-build" {
        return Err(fail("必须使用完整 v3 前端构建收据"));
    }
    source::protocol::request(
        &json!({"action":"validate-domains","repository":"frontend","value":value["sources"]}),
    )?;
    context(&value["build"])?;
    artifact_records(&value["files"])
}
pub(super) fn artifact_records(value: &Value) -> Result<()> {
    let files = value.as_array().ok_or_else(|| fail("产物必须是有序数组"))?;
    let mut previous = "";
    for file in files {
        exact(file, &["path", "bytes", "sha256"])?;
        let name = text(&file["path"])?;
        if !relative(name, false)
            || name <= previous
            || number(&file["bytes"], 9_007_199_254_740_991).is_none()
            || !hex(&file["sha256"], 64)
        {
            return Err(fail("产物路径、排序、长度或摘要无效"));
        }
        previous = name;
    }
    Ok(())
}

pub(super) fn normalized_files(value: &Value) -> Result<Value> {
    artifact_records(value)?;
    let mut files = value.clone();
    for file in files.as_array_mut().ok_or_else(|| fail("产物必须为数组"))? {
        file["bytes"] = json!(
            number(&file["bytes"], 9_007_199_254_740_991).ok_or_else(|| fail("产物长度无效"))?
        );
    }
    Ok(files)
}
