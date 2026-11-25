use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tree_sitter::Parser;

use crate::Result;

use super::model::{FileRecord, Kind};

pub(crate) const SOURCE: &str = include_str!("../../../../architecture/language-policy.toml");
pub(crate) const EMBEDDED_LUA_PATH: &str =
    "xtask/src/recovery/clone/native/target_verify/fresh/initialize/marker.rs";

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Policy {
    pub(crate) format_version: u32,
    pub(crate) id: String,
    pub(crate) measurement: String,
    pub(crate) inventory: String,
    pub(crate) denominator: String,
    pub(crate) embedded_lua_rule: String,
    pub(crate) unknown_text: String,
    pub(crate) case_rule: String,
    pub(crate) legacy_reference: String,
    pub(crate) template_suffixes: Vec<String>,
    pub(crate) template_rule: String,
    pub(crate) generated_components: Vec<String>,
    pub(crate) generated_name_fragments: Vec<String>,
    pub(crate) generated_name_prefixes: Vec<String>,
    pub(crate) generated_paths: Vec<String>,
    pub(crate) generated_line_prefixes: Vec<String>,
    pub(crate) generated_rule: String,
    pub(crate) test_components: Vec<String>,
    pub(crate) test_name_prefixes: Vec<String>,
    pub(crate) test_name_fragments: Vec<String>,
    pub(crate) test_rule: String,
    pub(crate) code_group_rule: String,
    pub(crate) document_extensions: Vec<String>,
    pub(crate) document_names: Vec<String>,
    pub(crate) config_extensions: Vec<String>,
    pub(crate) config_names: Vec<String>,
    pub(crate) config_name_prefixes: Vec<String>,
    pub(crate) resource_extensions: Vec<String>,
    pub(crate) source_extensions: BTreeMap<String, Vec<String>>,
    pub(crate) source_names: BTreeMap<String, Vec<String>>,
    pub(crate) source_name_prefixes: BTreeMap<String, Vec<String>>,
}

pub(crate) fn load() -> Result<Policy> {
    let policy: Policy = toml::from_str(SOURCE)?;
    if policy.format_version != 1 || policy.id != "ryframe-language-bytes-v1" {
        return Err("语言统计口径版本无效".into());
    }
    let mut extensions = BTreeSet::new();
    for extension in policy
        .source_extensions
        .values()
        .flatten()
        .chain(&policy.document_extensions)
        .chain(&policy.config_extensions)
        .chain(&policy.resource_extensions)
        .chain(&policy.template_suffixes)
    {
        if extension.is_empty()
            || *extension != extension.to_ascii_lowercase()
            || !extensions.insert(extension)
        {
            return Err(format!("语言统计扩展名规则重复或无效：{extension}").into());
        }
    }
    Ok(policy)
}

pub(crate) fn classify(policy: &Policy, path: &str, content: &[u8]) -> Result<FileRecord> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let mut target = name.to_ascii_lowercase();
    let mut template = false;
    while let Some((stem, extension)) = target.rsplit_once('.') {
        if !policy
            .template_suffixes
            .iter()
            .any(|value| value == extension)
        {
            break;
        }
        template = true;
        target = stem.to_owned();
    }
    let extension = target
        .rsplit_once('.')
        .map(|(_, extension)| extension)
        .unwrap_or("");
    let language = source_language(policy, &target, extension);
    let (kind, language, fallback) = if let Some(language) = language {
        (Kind::Code, Some(language), false)
    } else if member(&policy.document_extensions, extension)
        || member(&policy.document_names, &target)
    {
        (Kind::Document, None, false)
    } else if member(&policy.config_extensions, extension)
        || member(&policy.config_names, &target)
        || policy
            .config_name_prefixes
            .iter()
            .any(|prefix| target.starts_with(prefix))
    {
        (Kind::Config, None, false)
    } else if member(&policy.resource_extensions, extension) {
        (Kind::Resource, None, false)
    } else if template {
        (Kind::Code, Some("Template".into()), true)
    } else if content.contains(&0) || std::str::from_utf8(content).is_err() {
        (Kind::Resource, None, true)
    } else {
        (Kind::Code, Some("Other".into()), true)
    };
    if kind == Kind::Code && (content.contains(&0) || std::str::from_utf8(content).is_err()) {
        return Err(format!("代码文件不是无 NUL 的 UTF-8，不能改按资源缩小分母：{path}").into());
    }
    let embedded_lua_bytes = if path == EMBEDDED_LUA_PATH {
        if language.as_deref() != Some("Rust") {
            return Err(format!("Lua 宿主源码必须归类为 Rust：{path}").into());
        }
        Some(embedded_lua_bytes(content)?)
    } else {
        None
    };
    let header = String::from_utf8_lossy(&content[..content.len().min(2048)]);
    Ok(FileRecord {
        path: path.into(),
        bytes: u64::try_from(content.len())?,
        sha256: digest(content),
        kind,
        language,
        embedded_lua_bytes,
        vendor: path.starts_with("vendor/"),
        template,
        fallback,
        generated: path
            .split('/')
            .any(|part| member(&policy.generated_components, part))
            || member(&policy.generated_paths, path)
            || policy
                .generated_name_fragments
                .iter()
                .any(|part| name.contains(part))
            || policy
                .generated_name_prefixes
                .iter()
                .any(|prefix| name.starts_with(prefix))
            || header.lines().any(|line| {
                policy
                    .generated_line_prefixes
                    .iter()
                    .any(|prefix| line.trim_start().starts_with(prefix))
            }),
        test: path
            .split('/')
            .any(|part| member(&policy.test_components, part))
            || policy
                .test_name_prefixes
                .iter()
                .any(|prefix| name.starts_with(prefix))
            || policy
                .test_name_fragments
                .iter()
                .any(|part| name.contains(part)),
    })
}

fn embedded_lua_bytes(content: &[u8]) -> Result<u64> {
    let error = |reason| format!("内联 Lua 计量失败 {EMBEDDED_LUA_PATH}：{reason}");
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_rust::LANGUAGE.into())?;
    let tree = parser
        .parse(content, None)
        .ok_or_else(|| error("无法解析宿主源码"))?;
    let root = tree.root_node();
    if root.has_error() {
        return Err(error("宿主源码含语法错误").into());
    }
    let mut cursor = root.walk();
    let mut scripts = root.named_children(&mut cursor).filter(|node| {
        node.kind() == "const_item"
            && node
                .child_by_field_name("name")
                .is_some_and(|name| &content[name.byte_range()] == b"SCRIPT")
    });
    let script = scripts
        .next()
        .ok_or_else(|| error("缺少顶层 SCRIPT 常量"))?;
    if scripts.next().is_some() {
        return Err(error("顶层 SCRIPT 常量重复").into());
    }
    if !script
        .child_by_field_name("type")
        .is_some_and(|kind| &content[kind.byte_range()] == b"&str")
    {
        return Err(error("SCRIPT 常量的类型必须为 &str").into());
    }
    let payload = script
        .child_by_field_name("value")
        .filter(|node| node.kind() == "raw_string_literal" && content[node.start_byte()] == b'r')
        .and_then(|node| node.named_child(0))
        .filter(|node| node.kind() == "string_content" && node.start_byte() < node.end_byte())
        .ok_or_else(|| error("SCRIPT 必须是非空 Rust 原始字符串"))?;
    Ok(u64::try_from(payload.end_byte() - payload.start_byte())?)
}

fn source_language(policy: &Policy, name: &str, extension: &str) -> Option<String> {
    policy
        .source_extensions
        .iter()
        .find(|(_, extensions)| member(extensions, extension))
        .or_else(|| {
            policy
                .source_names
                .iter()
                .find(|(_, names)| member(names, name))
        })
        .or_else(|| {
            policy
                .source_name_prefixes
                .iter()
                .find(|(_, prefixes)| prefixes.iter().any(|prefix| name.starts_with(prefix)))
        })
        .map(|(language, _)| language.clone())
}

fn member(values: &[String], value: &str) -> bool {
    values.iter().any(|candidate| candidate == value)
}

pub(crate) fn digest(bytes: &[u8]) -> String {
    format_digest(Sha256::digest(bytes))
}

pub(crate) fn format_digest(bytes: impl IntoIterator<Item = u8>) -> String {
    use std::fmt::Write;
    bytes
        .into_iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            write!(hex, "{byte:02x}").expect("字符串写入不会失败");
            hex
        })
}
