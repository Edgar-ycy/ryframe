//! 核对历史 Linux 工具绑定与原启动参数。
use super::super::{digest, json as doc};
use crate::{Result, frontend_origin::manifest};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct NativeTool {
    pub distribution: String,
    pub user: String,
    pub uid: u32,
    pub program: PathBuf,
    pub linux_program: String,
    pub source_root: PathBuf,
    pub linux_source_root: String,
    pub source_commit: String,
    pub sha256: String,
    pub source_sha256: String,
    pub sources: Value,
}

impl NativeTool {
    pub(super) fn validate(&self) -> Result<()> {
        super::model::linux_path(self.program.to_str().ok_or("工具 Windows 路径不是UTF-8")?)?;
        super::model::linux_path(
            self.source_root
                .to_str()
                .ok_or("来源 Windows 路径不是UTF-8")?,
        )?;
        let safe_name = |s: &str| {
            !s.is_empty()
                && s.len() <= 128
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        };
        let linux = |s: &str| {
            s.starts_with('/')
                && s.len() <= 4096
                && !s.contains(['\\', '\0', '\r', '\n'])
                && !s.split('/').skip(1).any(|p| matches!(p, "" | "." | ".."))
        };
        if !safe_name(&self.distribution)
            || !safe_name(&self.user)
            || !linux(&self.linux_program)
            || !linux(&self.linux_source_root)
            || !manifest::hex_digest(&self.source_commit, 40)
            || !manifest::hex_digest(&self.sha256, 64)
            || !manifest::hex_digest(&self.source_sha256, 64)
            || manifest::sha256(&self.sources)? != self.source_sha256
        {
            return Err("缓存原生工具绑定无效".into());
        }
        Ok(())
    }

    pub(super) fn historical_arguments(
        &self,
        request: &Value,
        output: &Path,
    ) -> Result<Vec<String>> {
        // 重建 v2 冻结启动意图的参数，供历史收据比对。
        let path = output.join("launch-request.json");
        Ok(vec![
            doc::text(&request["wsl"]["path"], "WSL 工具")?.into(),
            "--distribution".into(),
            self.distribution.clone(),
            "--user".into(),
            self.user.clone(),
            "--exec".into(),
            self.linux_program.clone(),
            "__linux-cache".into(),
            "serve".into(),
            "--request".into(),
            super::model::linux_path(path.to_str().ok_or("缓存路径不是UTF-8")?)?,
            "--sha256".into(),
            digest::file(&path)?.sha256,
        ])
    }
}

pub(super) fn from_request(request: &Value) -> Result<NativeTool> {
    let tool: NativeTool = serde_json::from_value(request["native_tool"].clone())?;
    tool.validate()?;
    if request["distribution"] != tool.distribution {
        return Err("Redis 与原生工具发行版不同".into());
    }
    Ok(tool)
}

pub(crate) fn review_value(value: &Value) -> Result<()> {
    let tool: NativeTool = serde_json::from_value(value.clone())?;
    tool.validate()
}

pub(crate) fn successor(original: &Value, tools: &Value) -> Result<Value> {
    let before = from_request(original)?;
    let after: NativeTool = serde_json::from_value(tools["native_tool"].clone())?;
    after.validate()?;
    let stable = |tool: &NativeTool| {
        json!([
            tool.distribution,
            tool.user,
            tool.uid,
            tool.program,
            tool.linux_program,
            tool.source_root,
            tool.linux_source_root
        ])
    };
    if stable(&before) != stable(&after)
        || tools["redis_server"]["distribution"] != original["distribution"]
        || tools["redis_server"]["resolved_path"] != original["executable"]
        || tools["redis_server"]["sha256"] != original["sha256"]
        || tools["wsl"]["path"].as_str().map(str::to_ascii_lowercase)
            != original["wsl"]["path"]
                .as_str()
                .map(str::to_ascii_lowercase)
    {
        return Err("缓存工具后继不得改变原资源、Linux入口、UID或双端路径".into());
    }
    let mut result = original.clone();
    result["wsl"] = tools["wsl"].clone();
    result["native_tool"] = serde_json::to_value(after)?;
    super::model::shape(&result)?;
    Ok(result)
}

pub(crate) fn tools_hash(process: &Value) -> Result<String> {
    if process["format_version"] == 3 {
        super::windows_tool::validate(&process["tool"])?;
        digest::plan_hash(&process["tool"])
    } else if process["format_version"] == 2 {
        digest::plan_hash(&json!({"wsl":process["wsl"],"native_tool":process["native_tool"]}))
    } else {
        digest::plan_hash(&json!({"wsl":process["wsl"],"python":process["python"]}))
    }
}
