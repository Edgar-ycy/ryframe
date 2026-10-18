use std::{fs, path::Path, process::Command};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::{
    Result,
    recovery::{
        clone::native::digest,
        monitoring::contract::{Document, MONITORING_ALERTS, hex_text, read_document},
    },
};

pub(crate) fn coordinator_binding(backend: &Path) -> Result<Value> {
    crate::local_test_path::validate_absolute_path(
        backend,
        crate::local_test_path::LocalTestPathKind::ExistingDirectory,
    )?;
    let top = String::from_utf8(git(backend, &["rev-parse", "--show-toplevel"])?)?;
    if fs::canonicalize(top.trim())? != fs::canonicalize(backend)? {
        return Err("监控协调目录必须是 Git 仓库根目录".into());
    }
    let root_text = backend.to_string_lossy().into_owned();
    let head = git(backend, &["rev-parse", "HEAD"])?;
    let head = String::from_utf8(head)?.trim().to_owned();
    hex_text(Some(&json!(head)), 40, "监控协调提交")?;
    let status = git(backend, &["status", "--porcelain", "--untracked-files=all"])?;
    if !status.is_empty() {
        return Err("监控验收必须使用干净的协调后端工作树".into());
    }
    let mut hasher = Sha256::new();
    hasher.update(head.as_bytes());
    hasher.update([0]);
    hasher.update(status);
    Ok(json!({
        "root": root_text,
        "head": head,
        "inventory_sha256": hex::encode(hasher.finalize()),
        "executable": super::staging::descriptor(&std::env::current_exe()?)?,
    }))
}

pub(crate) fn bind_rules(backend: &Path) -> Result<(Value, Vec<Document>)> {
    let mut receipts = Map::new();
    let mut documents = Vec::new();
    for (name, relative) in crate::recovery::monitoring::staging::rules() {
        let path = backend.join(relative);
        let document = raw_document(&path, &format!("监控规则 {name}"))?;
        let tracked = git(backend, &["ls-files", "--error-unmatch", "--", relative])?;
        if String::from_utf8(tracked)?.trim() != relative {
            return Err(format!("监控规则 {name} 不是协调器 HEAD 跟踪的正式文件").into());
        }
        let committed = git(backend, &["show", &format!("HEAD:{relative}")])?;
        if committed != document.raw {
            return Err(format!("监控规则 {name} 与协调器 HEAD 跟踪内容不同").into());
        }
        validate_rule_text(name, &document.raw)?;
        receipts.insert(name.to_owned(), document.descriptor());
        documents.push(document);
    }
    Ok((Value::Object(receipts), documents))
}

fn raw_document(path: &Path, label: &str) -> Result<Document> {
    if let Ok(document) = read_document(path, label) {
        return Ok(document);
    }
    let raw = fs::read(path)?;
    let digest = digest::file(path)?;
    let value = json!({"text": std::str::from_utf8(&raw)?});
    Ok(Document {
        path: path.to_path_buf(),
        raw,
        value,
        sha256: digest.sha256,
    })
}

fn validate_rule_text(name: &str, raw: &[u8]) -> Result<()> {
    let text = std::str::from_utf8(raw).map_err(|_| format!("监控规则 {name} 不是 UTF-8"))?;
    if name == "alerts" {
        for alert in MONITORING_ALERTS {
            if text.matches(&format!("alert: {alert}")).count() != 1 {
                return Err("正式监控规则没有各定义一次固定的 8 个恢复告警".into());
            }
        }
    } else {
        for alert in MONITORING_ALERTS {
            if !text.contains(&format!("alertname: {alert}")) {
                return Err("正式监控规则测试没有精确覆盖固定的 8 个恢复告警".into());
            }
        }
        for boundary in ["23h", "23h30s", "24h30s"] {
            if !text.contains(&format!("eval_time: {boundary}")) {
                return Err("正式监控规则测试缺少 23/24 小时边界断言".into());
            }
        }
    }
    Ok(())
}

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git").args(args).current_dir(root).output()?;
    if !output.status.success() {
        return Err(format!("协调器 Git 命令失败：git {}", args.join(" ")).into());
    }
    Ok(output.stdout)
}
