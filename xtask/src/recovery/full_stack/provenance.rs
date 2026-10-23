//! 全栈构建的当前来源核验，收据格式与既有运行核验保持一致。
use crate::{
    Result,
    recovery::{
        clone::native::{json as documents, source},
        fixture_control::{
            file_binding, git_root, path_text_canonical, portable_path, read_json, text_field,
        },
    },
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

const MODEL: &str = "tools/python/fixtures/order-business/src/resources/mod.rs";
const SIDES: &[&str] = &["backend", "frontend"];

pub(super) fn current(root: &Path, directory: &Path) -> Result<Value> {
    if !directory.join("source-pair.json").is_file() {
        return Ok(Value::Null);
    }
    let root = portable_path(&fs::canonicalize(root)?);
    let fixture = root.parent().ok_or("后端缺少父目录")?.join("fixture.json");
    if fixture.is_file() {
        return business(&root, directory, &fixture);
    }
    let observed = snapshot(&root)?;
    let pair = pair(directory, &observed["head"], None, None)?;
    if pair["sources"]["backend"] != observed || snapshot(&root)? != observed {
        return Err("core 后端来源与当前源码不匹配或核验期间发生变化".into());
    }
    Ok(
        json!({"format_version":1,"fixture":"core","backend_root":root,
        "source_pair":pair,"original":{"backend":observed}}),
    )
}

fn business(root: &Path, directory: &Path, path: &Path) -> Result<Value> {
    let fixture = read_json(path, "业务 fixture", 4 * 1024 * 1024)?;
    documents::exact(
        &fixture,
        &[
            "format_version",
            "fixture",
            "status",
            "fixture_sha256",
            "sources",
            "paths",
            "generated",
        ],
        "业务 fixture",
    )?;
    if fixture["format_version"].as_u64() != Some(1)
        || fixture["fixture"] != "business"
        || fixture["status"] != "ready"
    {
        return Err("业务 fixture 尚未完成或格式不受支持".into());
    }
    for field in ["sources", "paths", "generated"] {
        documents::exact(&fixture[field], SIDES, "业务 fixture 双仓字段")?;
    }
    let parent = root.parent().ok_or("业务后端缺少父目录")?;
    let mut roots = serde_json::Map::new();
    for side in SIDES {
        let path = PathBuf::from(text_field(&fixture["paths"], side, "业务工作树")?);
        path_text_canonical(&path, "业务工作树")?;
        if !path.is_dir()
            || path.parent() != Some(parent)
            || git_root(&path, "业务工作树")? != path
            || (*side == "backend" && path != root)
        {
            return Err("业务 fixture 与规范 Git 工作树根目录不匹配".into());
        }
        validate_snapshot(&fixture["sources"][side], false)?;
        validate_snapshot(&fixture["generated"][side], false)?;
        if fixture["sources"][side]["head"] != fixture["generated"][side]["head"] {
            return Err("业务 fixture 原始 SHA 与生成工作树 HEAD 不匹配".into());
        }
        roots.insert((*side).into(), json!(path));
    }
    if roots["backend"] == roots["frontend"] {
        return Err("业务 fixture 必须绑定两个独立工作树".into());
    }
    let binding = file_binding(path, "业务 fixture")?;
    let pair = pair(
        directory,
        &fixture["sources"]["backend"]["head"],
        Some(&fixture["sources"]["frontend"]["head"]),
        Some((&binding, &fixture["sources"])),
    )?;
    if pair["sources"] != fixture["sources"] {
        return Err("业务 fixture 原始来源与源码组合不匹配".into());
    }
    let model = file_binding(&root.join(MODEL), "业务模型")?;
    if model["sha256"] != fixture["fixture_sha256"] {
        return Err("业务 fixture 定义与登记摘要不匹配".into());
    }
    for _ in 0..2 {
        for side in SIDES {
            if snapshot(Path::new(roots[*side].as_str().ok_or("工作树路径无效")?))?
                != fixture["generated"][side]
            {
                return Err("业务 fixture 生成工作树与登记快照不匹配".into());
            }
        }
    }
    Ok(
        json!({"format_version":1,"fixture":"business","fixture_receipt":binding,
        "fixture_definition":{"model":model},"roots":roots,"source_pair":pair,
        "original":fixture["sources"],"generated":fixture["generated"]}),
    )
}

fn pair(
    directory: &Path,
    backend: &Value,
    frontend: Option<&Value>,
    fixture: Option<(&Value, &Value)>,
) -> Result<Value> {
    let path = portable_path(&fs::canonicalize(directory)?).join("source-pair.json");
    let mut value = read_json(&path, "全栈源码组合", 4 * 1024 * 1024)?;
    if value["kind"] == "reference-fixture-source-pair" {
        documents::exact(
            &value,
            &["format_version", "kind", "fixture_receipt", "sources"],
            "参考源码组合",
        )?;
        let (binding, sources) = fixture.ok_or("参考源码组合缺少已核验的业务 fixture")?;
        if &value["fixture_receipt"] != binding || &value["sources"] != sources {
            return Err("参考源码组合未绑定当前 fixture 收据和来源".into());
        }
    } else {
        documents::exact(
            &value,
            &[
                "format_version",
                "backend_sha",
                "frontend_sha",
                "run_id",
                "attempt",
                "sources",
            ],
            "全栈源码组合",
        )?;
        for (key, expected) in [("backend_sha", Some(backend)), ("frontend_sha", frontend)] {
            if !hexadecimal(&value[key], 40) || expected.is_some_and(|v| &value[key] != v) {
                return Err("全栈源码组合提交与实际工作树不匹配".into());
            }
        }
        for (environment, field) in [
            ("GITHUB_RUN_ID", "run_id"),
            ("GITHUB_RUN_ATTEMPT", "attempt"),
        ] {
            let recorded = value[field]
                .as_u64()
                .filter(|v| *v > 0)
                .ok_or("源码组合运行身份无效")?;
            if let Some(actual) = std::env::var_os(environment)
                && actual
                    .to_str()
                    .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
                    .and_then(|v| v.parse::<u64>().ok())
                    != Some(recorded)
            {
                return Err("全栈源码组合不属于当前 CI attempt".into());
            }
        }
    }
    if value["format_version"].as_u64() != Some(1) {
        return Err("全栈源码组合版本无效".into());
    }
    documents::exact(&value["sources"], SIDES, "源码组合来源")?;
    for side in SIDES {
        validate_snapshot(&value["sources"][side], true)?;
        let expected = if *side == "backend" {
            Some(backend)
        } else {
            frontend
        };
        if expected.is_some_and(|head| &value["sources"][side]["head"] != head)
            || (value.get("backend_sha").is_some()
                && value["sources"][side]["head"] != value[format!("{side}_sha")])
        {
            return Err("源码组合原始快照与目标提交不匹配".into());
        }
    }
    value["receipt"] = file_binding(&path, "源码组合")?;
    Ok(value)
}

fn validate_snapshot(value: &Value, clean: bool) -> Result<()> {
    documents::exact(value, &["head", "patch_sha256", "files"], "来源快照")?;
    if !hexadecimal(&value["head"], 40) || !hexadecimal(&value["patch_sha256"], 64) {
        return Err("来源快照提交或摘要无效".into());
    }
    let mut previous = "";
    for file in documents::array(&value["files"], "未跟踪来源")? {
        documents::exact(file, &["path", "sha256"], "未跟踪来源")?;
        let path = documents::text(&file["path"], "未跟踪来源路径")?;
        // 与来源清单一致：只接受排序且不含重复项的规范相对路径。
        if path <= previous
            || path.contains(['\\', '\0', ':'])
            || path.starts_with('/')
            || path.split('/').any(|part| matches!(part, "" | "." | ".."))
            || !hexadecimal(&file["sha256"], 64)
        {
            return Err("未跟踪来源路径、顺序或摘要无效".into());
        }
        previous = path;
    }
    if clean
        && (value["patch_sha256"] != crate::devex::runtime_native::artifacts::sha(&[])
            || value["files"] != json!([]))
    {
        return Err("源码组合必须登记目标提交的干净源码".into());
    }
    Ok(())
}

fn hexadecimal(value: &Value, length: usize) -> bool {
    value.as_str().is_some_and(|s| {
        s.len() == length
            && s.bytes()
                .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
    })
}

pub(super) fn snapshot(root: &Path) -> Result<Value> {
    if git_root(root, "来源工作树")? != root {
        return Err("来源必须为实际 Git 根目录".into());
    }
    let value = source::capture(root)?.source.snapshot;
    Ok(json!({"head":value.head,"patch_sha256":value.patch_sha256,
        "files":value.files.into_iter().map(|f|json!({"path":f.path,"sha256":f.sha256})).collect::<Vec<_>>()}))
}

pub(super) fn build(
    root: &Path,
    directory: &Path,
    binaries: &Value,
    expected: &Value,
) -> Result<Value> {
    let roles = [
        "ryframe",
        "ryframe-worker",
        "ryframe-reset",
        "ryframe-migrate",
    ];
    documents::exact(binaries, &roles, "全栈二进制清单")?;
    let source = current(root, directory)?;
    if &source != expected {
        return Err("构建期间源码或 fixture 来源发生变化".into());
    }
    let mut artifacts = serde_json::Map::new();
    for role in roles {
        let path = Path::new(documents::text(&binaries[role], "构建产物路径")?);
        artifacts.insert(role.into(), file_binding(path, "构建产物")?);
    }
    if current(root, directory)? != source {
        return Err("采集构建产物期间源码或 fixture 来源发生变化".into());
    }
    Ok(json!({"format_version":1,"kind":"full-stack-build",
        "backend_root":portable_path(&fs::canonicalize(root)?),"source":source,"artifacts":artifacts}))
}
