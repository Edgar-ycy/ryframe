//! 登录审计证据由原生重新读取与比较，适配层只负责查询和独占写文件。
use super::super::super::model as source_model;
use super::{documents, model, receipts};
use crate::{
    Result,
    recovery::clone::native::{digest, resources::common},
};
use serde_json::{Value, json};

pub(super) fn create(args: &Value) -> Result<Value> {
    documents::exact(
        args,
        &[
            "backend",
            "directory",
            "old_before",
            "old_after",
            "upper_id",
            "new_raw",
            "lineage",
            "subjects",
            "started_at",
            "completed_at",
        ],
        "来源登录审计构造参数",
    )?;
    let (_, directory) = receipts::directory(args)?;
    let verified = source_model::login_audit(
        &json!({"old_before":args["old_before"],"old_after":args["old_after"],
        "upper_id":args["upper_id"],"new_rows":model::login_rows(&json!({"raw":args["new_raw"]}))?,
        "lineage":args["lineage"],"subjects":args["subjects"],"started_at":args["started_at"],"completed_at":args["completed_at"]}),
    )?;
    Ok(
        json!({"format_version":1,"kind":"restore-source-login-audit","status":"verified",
        "before_rows":common::binding(&directory.join("audit/login-before.tsv"))?,
        "after_old_rows":common::binding(&directory.join("audit/login-after-old.tsv"))?,
        "after_new_rows":common::binding(&directory.join("audit/login-after-new.tsv"))?,
        "verified":verified,"completed_at":args["completed_at"]}),
    )
}

pub(super) fn stored(args: &Value) -> Result<Value> {
    documents::exact(
        args,
        &[
            "backend",
            "directory",
            "descriptor",
            "lineage",
            "subjects",
            "started_at",
        ],
        "来源已保存登录审计参数",
    )?;
    let (_, directory) = receipts::directory(args)?;
    let audit = receipts::login(
        &json!({"backend":args["backend"],"directory":args["directory"],"descriptor":args["descriptor"]}),
    )?;
    let mut bytes = Vec::new();
    for name in [
        "login-before.tsv",
        "login-after-old.tsv",
        "login-after-new.tsv",
    ] {
        let path = digest::safe_file(&directory.join("audit"), name)?;
        bytes.push(std::fs::read(path)?);
    }
    let added = std::str::from_utf8(&bytes[2])
        .map_err(|_| "来源登录审计新行不是 UTF-8")?
        .trim();
    let verified = source_model::login_audit(
        &json!({"old_before":hex::encode(&bytes[0]),"old_after":hex::encode(&bytes[1]),
        "upper_id":audit["verified"]["upper_id"].as_str().ok_or("来源登录上界不是文本")?.parse::<u64>().map_err(|_| "来源登录上界无效")?,
        "new_rows":model::login_rows(&json!({"raw":added}))?,"lineage":args["lineage"],"subjects":args["subjects"],
        "started_at":args["started_at"],"completed_at":audit["completed_at"]}),
    )?;
    if verified != audit["verified"] {
        return Err("来源登录审计派生结果发生变化".into());
    }
    receipts::login(
        &json!({"backend":args["backend"],"directory":args["directory"],"descriptor":args["descriptor"]}),
    )?;
    Ok(audit)
}

pub(super) fn recheck(args: &Value) -> Result<Value> {
    documents::exact(
        args,
        &["backend", "directory", "filename", "raw"],
        "来源登录日志复核参数",
    )?;
    let (_, directory) = receipts::directory(args)?;
    let filename = documents::text(&args["filename"], "来源登录日志文件")?;
    if !["login-after-old.tsv", "login-after-new.tsv"].contains(&filename) {
        return Err("来源登录日志复核文件不属于固定原审计".into());
    }
    let path = digest::safe_file(&directory.join("audit"), filename)?;
    let before = common::binding(&path)?;
    let raw = documents::text(&args["raw"], "来源登录日志查询响应")?;
    let actual = format!("{raw}{}", if raw.is_empty() { "" } else { "\n" });
    if std::fs::read(&path)? != actual.as_bytes() || common::binding(&path)? != before {
        return Err("来源验收恢复时登录日志出现原审计以外的未知写入".into());
    }
    Ok(Value::Null)
}
