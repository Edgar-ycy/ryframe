use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

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
#[path = "execution/window.rs"]
mod window;
pub(in crate::recovery::fixture_control) use window::Window;

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
    let before = root_receipt_observed(root, expected, side, initial);
    let after = root_receipt_observed(root, expected, side, initial);
    match before {
        Ok(before) => {
            let after = after.and_then(|after| {
                window::check(after == before, "读取独立执行源码期间内容或索引漂移")
            });
            after?;
            Ok(before)
        }
        Err(error) => Err(window::failure(error, after.map(|_| ()))),
    }
}

fn root_receipt_observed(
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
    let window = local_window(coordinator, descriptor)?;
    window.verify()?;
    Ok(window.publication().clone())
}

/// 运行来源只持有 P 时，清单必须在 P 同级固定位置，仍执行同一完整来源核验。
pub(in crate::recovery) fn verify_for_root(root: &Path, descriptor: &Value) -> Result<Value> {
    let window = root_window(root, descriptor)?;
    let selected = require_root(root, window.publication());
    let after = window.verify();
    window::finish([selected, after])?;
    Ok(window.publication().clone())
}

fn local_window(coordinator: &Path, descriptor: &Value) -> Result<Window> {
    exact(descriptor, &["path", "bytes", "sha256"], "执行源码清单绑定")?;
    let path = Path::new(descriptor["path"].as_str().ok_or("执行源码绑定路径缺失")?);
    validate_local_test_path(path, coordinator, LocalTestPathKind::ExistingFile)?;
    Window::bound(path, Some(descriptor))
}

fn root_window(root: &Path, descriptor: &Value) -> Result<Window> {
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
    Window::bound(path, Some(descriptor))
}

fn require_root(root: &Path, publication: &Value) -> Result<()> {
    window::check(
        publication["published"]["backend"]["path"]
            == super::super::path_text_canonical(root, "执行后端")?,
        "执行来源未绑定当前运行后端",
    )
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

/// 发布输入只从完整来源证明派生；不扩大普通收据的本地路径边界。
pub(in crate::recovery) fn target_protected_inputs(
    coordinator: &Path,
    declared: &Value,
) -> Result<Vec<PathBuf>> {
    let (_, verified) = target_binding(coordinator, declared)?;
    let mut paths = Vec::new();
    for descriptor in [&verified["execution_source"], &verified["fixture"]] {
        let path = PathBuf::from(
            descriptor["path"]
                .as_str()
                .ok_or("发布执行辅助输入路径缺失")?,
        );
        validate_absolute_path(&path, LocalTestPathKind::ExistingFile)?;
        if crate::recovery::clone::native::resources::common::binding(&path)? != *descriptor {
            return Err("发布执行辅助输入在完整核验后变化".into());
        }
        paths.push(path);
    }
    Ok(paths)
}

pub(in crate::recovery) const PAIR_KIND: &str = "reference-fixture-execution-source-pair";

/// 仅从显式发布收据签发执行组合，不要求生成后的提交等于原始提交。
pub(in crate::recovery) fn pair(
    coordinator: &Path,
    root: &Path,
    descriptor: &Value,
) -> Result<Value> {
    let window = local_window(coordinator, descriptor)?;
    let selected = (|| {
        window::check(
            window.publication()["published"]["backend"]["path"]
                == super::super::path_text_canonical(root, "执行后端")?,
            "执行来源与构建后端不同",
        )
    })();
    let after = window.verify();
    window::finish([selected, after])?;
    Ok(
        json!({"format_version":1,"kind":PAIR_KIND,"execution_source":descriptor,
        "sources":window.publication()["original"]["source_pair"]["sources"]}),
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
    let window = root_window(root, &pair["execution_source"])?;
    let selected = require_root(root, window.publication());
    let sources = window::check(
        pair["sources"] == window.publication()["original"]["source_pair"]["sources"],
        "执行来源组合的原始来源不匹配",
    );
    let after = window.verify();
    window::finish([selected, sources, after])?;
    Ok(window.publication().clone())
}
