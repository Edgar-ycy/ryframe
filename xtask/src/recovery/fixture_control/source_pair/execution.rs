use std::{collections::BTreeSet, fs, path::Path};

use serde_json::{Value, json};

use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path, validate_local_test_path},
    recovery::clone::native::{json::exact, source},
};

use super::super::{file_binding, read_json, write_new};

#[path = "execution/assets.rs"]
mod assets;
#[path = "execution/git.rs"]
mod git;
#[path = "execution/input.rs"]
mod input;

#[cfg(test)]
#[path = "../../../../tests/unit/recovery/fixture_control/source_pair/execution.rs"]
pub(crate) mod tests;

/// 将已冻结的业务源码发布为独立仓库；失败保留部分输出，禁止覆盖重试。
pub(crate) fn publish(backend: &Path, output: &Path) -> Result<Value> {
    let before = input::load(backend)?;
    validate_local_test_path(output, backend, LocalTestPathKind::NewDirectory)?;
    let parent = output.parent().ok_or("执行源码输出缺少父目录")?;
    validate_absolute_path(parent, LocalTestPathKind::ExistingDirectory)?;
    if parent.join("fixture.json").exists() {
        return Err("执行源码不能放在含 fixture.json 的直接父目录；请选择独立输出目录".into());
    }
    fs::create_dir(output)?;
    let result = publish_roots(&before, output).and_then(|published| {
        if input::load(backend)?.receipt != before.receipt {
            return Err("发布期间原 fixture 或完整源码变化；保留输出，禁止重放".into());
        }
        let document = json!({
            "format_version": 1,
            "kind": "reference-fixture-execution-source",
            "original": before.receipt,
            "published": published,
        });
        let path = output.join("execution-source.json");
        write_new(&path, &document)?;
        verify(backend, &file_binding(&path, "执行源码清单")?)
    });
    result.map_err(|error| {
        format!(
            "执行源码发布失败，已保留现场 {}；核对原因后选择新输出目录：{error}",
            output.display()
        )
        .into()
    })
}

fn publish_roots(input: &input::Input, output: &Path) -> Result<Value> {
    let mut published = serde_json::Map::new();
    for (name, side) in &input.sides {
        let destination = output.join(name);
        git::publish(side, &destination)?;
        published.insert(
            (*name).into(),
            root_receipt(&destination, &side.files, name, true)?,
        );
    }
    Ok(Value::Object(published))
}

fn root_receipt(
    root: &Path,
    expected: &[source::FileRecord],
    side: &str,
    initial: bool,
) -> Result<Value> {
    validate_absolute_path(root, LocalTestPathKind::ExistingDirectory)?;
    // 先拒绝链接，再统一实际路径拼写，避免 Windows 分隔符导致同一根的收据不同。
    let canonical = super::super::portable_path(&fs::canonicalize(root)?);
    let root = canonical.as_path();
    if root
        .parent()
        .ok_or("执行源码根缺少父目录")?
        .join("fixture.json")
        .exists()
    {
        return Err("独立执行源码的父目录不能含 fixture.json".into());
    }
    let inventory = source::capture(root)?;
    if !inventory.source.snapshot.clean || inventory.files.as_slice() != expected {
        return Err("独立执行源码不干净或完整内容与冻结生成源码不一致".into());
    }
    let files = inventory.files.iter().map(|v| v.path.as_str()).collect();
    let managed: &[&str] = match (initial, side) {
        (true, _) => &[],
        (false, "backend") => &[".local-tests"],
        (false, "frontend") => &[".local-tests", "node_modules", "dist"],
        _ => return Err("执行源码侧无效".into()),
    };
    require_exact_files(root, root, &files, managed)?;
    let receipt = json!({
        "path": super::super::path_text_canonical(root, "执行源码根")?,
        "head": inventory.source.snapshot.head,
        "tree": git::text(root, &["rev-parse", "HEAD^{tree}"])? ,
        "inventory": source::inventory_value(&inventory),
        "fingerprints": source::fingerprints(&inventory.files)?,
    });
    if source::capture(root)? != inventory {
        return Err("读取独立执行源码期间内容或索引漂移".into());
    }
    Ok(receipt)
}

fn require_exact_files(
    root: &Path,
    directory: &Path,
    files: &BTreeSet<&str>,
    managed: &[&str],
) -> Result<()> {
    for entry in crate::recovery::runtime::dataset_authority::reads::entries(directory)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        if directory == root && name == ".git" {
            validate_absolute_path(&path, LocalTestPathKind::ExistingDirectory)?;
            continue;
        }
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            validate_absolute_path(&path, LocalTestPathKind::ExistingDirectory)?;
            if directory == root
                && let Some(name) = name.to_str()
                && managed.contains(&name)
            {
                git::require_ignored(root, &format!("{name}/"))?;
                // 此处只分离 Git 源码与受管资产；资产内容和内部链接由各自 guard 负责。
                continue;
            }
            require_exact_files(root, &path, files, managed)?;
        } else {
            validate_absolute_path(&path, LocalTestPathKind::ExistingFile)?;
            let name = path
                .strip_prefix(root)?
                .to_str()
                .ok_or("源码路径必须为 UTF-8")?
                .replace('\\', "/");
            if !files.contains(name.as_str()) {
                return Err("独立源码根混入清单外文件、运行配置或构建产物".into());
            }
        }
    }
    Ok(())
}

/// 核验显式清单绑定并重读完整来源；调用方仍须匹配执行根与预期提交。
pub(crate) fn verify(coordinator: &Path, descriptor: &Value) -> Result<Value> {
    exact(descriptor, &["path", "bytes", "sha256"], "执行源码清单绑定")?;
    let path = Path::new(descriptor["path"].as_str().ok_or("执行源码绑定路径缺失")?);
    validate_local_test_path(path, coordinator, LocalTestPathKind::ExistingFile)?;
    verify_bound(descriptor, path)
}

/// 运行来源只持有 P 时，清单必须在 P 同级固定位置，仍执行同一完整来源核验。
pub(in crate::recovery) fn verify_for_root(root: &Path, descriptor: &Value) -> Result<Value> {
    exact(descriptor, &["path", "bytes", "sha256"], "执行源码清单绑定")?;
    validate_absolute_path(root, LocalTestPathKind::ExistingDirectory)?;
    let path = Path::new(descriptor["path"].as_str().ok_or("执行源码绑定路径缺失")?);
    validate_absolute_path(path, LocalTestPathKind::ExistingFile)?;
    let expected = root
        .parent()
        .ok_or("执行源码根缺少父目录")?
        .join("execution-source.json");
    if super::super::portable_path(&fs::canonicalize(path)?)
        != super::super::portable_path(&expected)
    {
        return Err("执行源码清单必须位于运行后端同级固定位置".into());
    }
    let publication = verify_bound(descriptor, path)?;
    if publication["published"]["backend"]["path"]
        != super::super::path_text_canonical(root, "执行后端")?
    {
        return Err("执行来源未绑定当前运行后端".into());
    }
    Ok(publication)
}

/// fresh 请求明确绑定发布源码，原 fixture 只从完整发布证明派生。
pub(in crate::recovery) fn target_binding(
    coordinator: &Path,
    declared: &Value,
) -> Result<(std::path::PathBuf, Value)> {
    exact(declared, &["path", "execution_source"], "发布执行工作树")?;
    let requested = Path::new(declared["path"].as_str().ok_or("发布执行根缺失")?);
    validate_absolute_path(coordinator, LocalTestPathKind::ExistingDirectory)?;
    validate_absolute_path(requested, LocalTestPathKind::ExistingDirectory)?;
    let root = super::super::portable_path(&fs::canonicalize(requested)?);
    let coordinator = super::super::portable_path(&fs::canonicalize(coordinator)?);
    if root != coordinator {
        validate_local_test_path(&root, &coordinator, LocalTestPathKind::ExistingDirectory)?;
    }
    let publication = verify_for_root(&root, &declared["execution_source"])?;
    let mut snapshot =
        publication["published"]["backend"]["inventory"]["source"]["snapshot"].clone();
    snapshot
        .as_object_mut()
        .ok_or("发布源码快照无效")?
        .remove("clean");
    exact(
        &snapshot,
        &["files", "head", "patch_sha256"],
        "发布源码快照",
    )?;
    let binding = json!({"kind":"published-business-fixture","path":root,
        "execution_source":declared["execution_source"],"fixture":publication["original"]["fixture_receipt"],"source":snapshot});
    Ok((root, binding))
}

fn verify_bound(descriptor: &Value, path: &Path) -> Result<Value> {
    if file_binding(path, "执行源码清单")? != *descriptor {
        return Err("执行源码清单绑定漂移；重新核对来源，禁止改写旧绑定".into());
    }
    let document = verify_file(path)?;
    if file_binding(path, "执行源码清单")? != *descriptor {
        return Err("执行源码清单在来源复核期间发生变化".into());
    }
    Ok(document)
}

pub(in crate::recovery) const PAIR_KIND: &str = "reference-fixture-execution-source-pair";

/// 仅从显式发布收据签发执行组合，不要求生成后的提交等于原始提交。
pub(in crate::recovery) fn pair(
    coordinator: &Path,
    root: &Path,
    descriptor: &Value,
) -> Result<Value> {
    let publication = verify(coordinator, descriptor)?;
    if publication["published"]["backend"]["path"]
        != super::super::path_text_canonical(root, "执行后端")?
    {
        return Err("执行来源与构建后端不同".into());
    }
    Ok(
        json!({"format_version":1,"kind":PAIR_KIND,"execution_source":descriptor,
        "sources":publication["original"]["source_pair"]["sources"]}),
    )
}

pub(in crate::recovery) fn verify_pair(root: &Path, pair: &Value) -> Result<Value> {
    exact(
        pair,
        &["format_version", "kind", "execution_source", "sources"],
        "执行来源组合",
    )?;
    if pair["format_version"] != 1 || pair["kind"] != PAIR_KIND {
        return Err("执行来源组合类型或版本无效".into());
    }
    let publication = verify_for_root(root, &pair["execution_source"])?;
    if pair["sources"] != publication["original"]["source_pair"]["sources"] {
        return Err("执行来源组合的原始来源不匹配".into());
    }
    Ok(publication)
}

fn verify_file(path: &Path) -> Result<Value> {
    if path.file_name().and_then(|v| v.to_str()) != Some("execution-source.json") {
        return Err("执行源码清单必须命名为 execution-source.json".into());
    }
    let document = read_json(path, "执行源码清单", 16 * 1024 * 1024)?;
    exact(
        &document,
        &["format_version", "kind", "original", "published"],
        "执行源码清单",
    )?;
    if document["format_version"] != 1 || document["kind"] != "reference-fixture-execution-source" {
        return Err("执行源码清单版本或类型无效".into());
    }
    let backend = document["original"]["paths"]["backend"]
        .as_str()
        .ok_or("原后端路径缺失")?;
    let before = input::load(Path::new(backend))?;
    if document["original"] != before.receipt {
        return Err("原 fixture 绑定或生成源码已漂移；禁止沿用执行来源".into());
    }
    exact(
        &document["published"],
        &["backend", "frontend"],
        "发布源码组合",
    )?;
    for (name, side) in &before.sides {
        let root = path.parent().ok_or("执行源码清单缺少父目录")?.join(name);
        let actual = root_receipt(&root, &side.files, name, false)?;
        let expected = &document["published"][*name];
        if expected != &actual {
            let changed = ["path", "head", "tree", "inventory", "fingerprints"]
                .into_iter()
                .filter(|field| expected[*field] != actual[*field])
                .collect::<Vec<_>>();
            let changed = if changed.is_empty() {
                "结构".into()
            } else {
                changed.join("、")
            };
            return Err(format!(
                "发布源码 HEAD、tree、完整清单或来源指纹发生变化；{name} 变化字段：{changed}"
            )
            .into());
        }
    }
    if input::load(Path::new(backend))?.receipt != before.receipt
        || read_json(path, "执行源码清单", 16 * 1024 * 1024)? != document
    {
        return Err("执行来源复核期间原输入或清单变化".into());
    }
    Ok(document)
}
