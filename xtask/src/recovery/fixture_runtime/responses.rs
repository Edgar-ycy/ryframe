use super::model::*;
use crate::{Result, recovery::clone::native::json as strict};
use percent_encoding::{AsciiSet, CONTROLS, percent_decode_str, utf8_percent_encode};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
const ENCODE: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');
pub(super) fn target(value: &str) -> Result<String> {
    check(
        value.starts_with('/') && !value.contains('\\'),
        "静态响应路径必须是规范化同源路径",
    )?;
    let decoded = percent_decode_str(value)
        .decode_utf8()
        .map_err(|_| "静态响应不是UTF-8")?;
    check(
        utf8_percent_encode(&decoded, ENCODE).to_string() == value && !decoded.contains(['?', '#']),
        "静态响应包含查询、片段或非规范化转义",
    )?;
    let parts = decoded[1..].split('/').collect::<Vec<_>>();
    check(
        !parts
            .iter()
            .enumerate()
            .any(|(i, p)| *p == "." || *p == ".." || (p.is_empty() && i + 1 != parts.len())),
        "静态响应路径包含空段或目录跳转",
    )?;
    Ok(
        if decoded == "/" || Path::new(&decoded[1..]).extension().is_none() {
            "index.html".into()
        } else {
            decoded[1..].into()
        },
    )
}
pub(super) fn preview(path: &Path, binding: &Value, manifest: &Value) -> Result<Value> {
    let (v, guard) = document(path)?;
    strict::exact(
        &v,
        &[
            "format_version",
            "kind",
            "status",
            "run_id",
            "scope_id",
            "limits",
            "total_entries",
            "total_bytes",
            "entries",
        ],
        "preview静态响应收据",
    )?;
    let entries = v["entries"].as_array().ok_or("静态响应明细无效")?;
    check(
        v["format_version"].as_u64() == Some(1)
            && v["kind"] == "business-preview-static-responses"
            && v["status"] == "complete"
            && v["run_id"] == binding["run_id"]
            && v["scope_id"] == binding["scope_id"]
            && v["limits"] == json!({"entries":MAX_FILES,"bytes":MAX_BYTES})
            && !entries.is_empty()
            && entries.len() <= MAX_FILES as usize
            && v["total_entries"].as_u64() == Some(entries.len() as u64),
        "preview静态响应身份、状态或上限无效",
    )?;
    strict::exact(
        manifest,
        &[
            "format_version",
            "kind",
            "root",
            "limits",
            "total_files",
            "total_bytes",
            "files",
        ],
        "前端生产清单",
    )?;
    check(
        manifest["format_version"].as_u64() == Some(1)
            && manifest["kind"] == "bounded-artifact-manifest",
        "生产清单版本无效",
    )?;
    let (total, targets) = verify_entries(entries, manifest)?;
    check(
        v["total_bytes"].as_u64() == Some(total) && total <= MAX_BYTES,
        "静态响应字节汇总无效",
    )?;
    check(
        targets.contains("index.html") && targets.iter().any(|t| t.ends_with(".js")),
        "preview没有实际加载入口HTML与JavaScript",
    )?;
    unchanged(&guard)?;
    Ok(json!({"receipt":v,"descriptor":guard["descriptor"]}))
}

fn verify_entries(entries: &[Value], manifest: &Value) -> Result<(u64, BTreeSet<String>)> {
    let mut files = BTreeMap::new();
    for file in manifest["files"].as_array().ok_or("生产清单缺少文件")? {
        strict::exact(file, &["path", "bytes", "sha256"], "生产文件")?;
        check(!text(&file["path"])?.is_empty(), "生产清单路径不能为空")?;
        check(
            files.insert(text(&file["path"])?, file).is_none(),
            "生产清单路径重复",
        )?;
    }
    let mut total = 0_u64;
    let mut targets = BTreeSet::new();
    for (i, e) in entries.iter().enumerate() {
        strict::exact(
            e,
            &[
                "sequence",
                "method",
                "path",
                "destination",
                "status",
                "bytes",
                "sha256",
                "representation",
            ],
            "preview静态响应",
        )?;
        let target = target(text(&e["path"])?)?;
        let expected = files.get(target.as_str()).ok_or("静态响应未登记生产文件")?;
        let sha = text(&e["sha256"])?;
        check(
            sha.len() == 64
                && sha
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "静态响应SHA-256无效",
        )?;
        check(
            e["sequence"].as_u64() == Some(i as u64 + 1)
                && e["method"] == "GET"
                && [
                    "audio",
                    "document",
                    "embed",
                    "font",
                    "frame",
                    "iframe",
                    "image",
                    "manifest",
                    "object",
                    "script",
                    "sharedworker",
                    "style",
                    "track",
                    "video",
                    "worker",
                ]
                .contains(&text(&e["destination"])?)
                && e["status"].as_u64() == Some(200)
                && e["representation"] == "identity"
                && e["bytes"] == expected["bytes"]
                && e["sha256"] == expected["sha256"],
            "preview静态响应与绑定生产产物不一致",
        )?;
        total = total
            .checked_add(e["bytes"].as_u64().ok_or("响应字节无效")?)
            .ok_or("响应字节溢出")?;
        targets.insert(target);
    }
    Ok((total, targets))
}
