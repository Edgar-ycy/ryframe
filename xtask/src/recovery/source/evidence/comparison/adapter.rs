//! 固定 B0 工具补丁；只读分支不创建索引或 Git 对象。
use super::{common, document, source};
use crate::Result;
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

pub(super) const BACKEND: &str = "815c5eafb09d4b493319d255fb7ab88ddd02c8b6";
pub(super) const FRONTEND: &str = "0087ea2ecf62530d042b9e52f5c950fb34c66d78";
pub(super) const COMMIT: &str = "c05114bcdf5c369cd74087db6317ce3c8f89bee8";
const BASE_TREE: &str = "aeb1824ee52ea12d53d0cf87c327c7be3d1e12b4";
const TREE: &str = "2ff15e7f34da1749c7eb27a1a025b9d3811b87a7";
const PATCH: &str = "xtask/assets/baseline-adapters/stable-readiness-b0-v1.patch";
const SHA256: &str = "28ad29f21ca9fd58356d059700bcc871172466e0a62286c5ccdbf68b1ed3fba3";
pub(super) const CONTRACT: &str = "legacy-stable-readiness-b0-v1";

pub(super) fn git(root: &Path, args: &[&str], index: Option<&Path>) -> Result<String> {
    let mut command = Command::new("git");
    // 只允许显式根和本次索引，父进程的 Git 环境不得重定向对象或写入位置。
    for (name, _) in std::env::vars_os() {
        if name
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("GIT_")
        {
            command.env_remove(name);
        }
    }
    command
        .current_dir(root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .args(args);
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }
    let output = command.output()?;
    if !output.status.success() {
        return Err("比较来源 Git 核验失败；保留现场核对固定提交、补丁和权限".into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

pub(super) fn evidence(root: &Path, reconstruct: bool) -> Result<Value> {
    let root = super::repository(root)?;
    require_original_base(&root)?;
    let patch = crate::recovery::clone::native::digest::safe_file(&root, PATCH)?;
    let binding = common::binding(&patch)?;
    if git(&root, &["rev-parse", &format!("{BACKEND}^{{tree}}")], None)? != BASE_TREE
        || binding["sha256"] != SHA256
    {
        return Err("B0 基线树或工具适配补丁摘要不匹配".into());
    }
    if reconstruct && rebuild(&root, &patch)? != TREE {
        return Err("B0 工具补丁重建树不匹配".into());
    }
    if common::binding(&patch)? != binding {
        return Err("B0 工具补丁在核验期间变化".into());
    }
    require_original_base(&root)?;
    Ok(expected(binding["bytes"].clone()))
}

fn require_original_base(root: &Path) -> Result<()> {
    let reference = format!("refs/replace/{BACKEND}");
    if !git(
        root,
        &["for-each-ref", "--format=%(refname)", &reference],
        None,
    )?
    .is_empty()
    {
        return Err("B0 基线存在 Git 替换引用；保留现场，核对原始固定对象后再继续".into());
    }
    Ok(())
}

pub(super) fn expected(bytes: Value) -> Value {
    json!({"contract":CONTRACT,"base_backend_sha":BACKEND,"base_frontend_sha":FRONTEND,
        "reference_adapter_sha":COMMIT,"adapter_tree":TREE,"reconstructed_tree":TREE,
        "adapter_paths":["xtask/src/cli.rs"],
        "patch":{"path":PATCH,"bytes":bytes,"sha256":SHA256}})
}

fn rebuild(root: &Path, patch: &Path) -> Result<String> {
    let target = root.join("target");
    if crate::recovery::clone::native::digest::linked_path(&target)?
        .into_iter()
        .any(|v| v)
    {
        return Err("B0 索引临时目录不能是链接或重解析点".into());
    }
    fs::create_dir_all(&target)?;
    let index = target.join(format!("b0-adapter-{}.index", uuid::Uuid::new_v4()));
    let lock = index.with_extension("index.lock");
    if index.try_exists()? || lock.try_exists()? {
        return Err("B0 索引临时路径已存在，禁止覆盖".into());
    }
    let result = (|| {
        git(root, &["read-tree", BACKEND], Some(&index))?;
        git(
            root,
            &[
                "apply",
                "--cached",
                "--whitespace=nowarn",
                patch.to_str().ok_or("B0 补丁路径必须为 UTF-8")?,
            ],
            Some(&index),
        )?;
        git(root, &["write-tree"], Some(&index))
    })();
    // 路径属于本次随机索引；失败证据和已写入的 Git 对象保持不变。
    for path in [&index, &lock] {
        match fs::remove_file(path) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => {
                return Err(format!("B0 临时索引清理失败：{error}；原结果：{result:?}").into());
            }
        }
    }
    result
}

pub(super) fn build_source(args: &Value) -> Result<Value> {
    document::exact(
        args,
        &[
            "backend",
            "source_backend",
            "expected_head",
            "adapter_contract",
            "product_backend",
            "reconstruct",
        ],
        "B0 构建来源参数",
    )?;
    if args["expected_head"] != COMMIT || args["adapter_contract"] != CONTRACT {
        return Err("B0 构建来源与登记适配提交不同".into());
    }
    let execution = super::path(&args["source_backend"])?;
    let product = super::path(&args["product_backend"])?;
    let inventory = super::arms::inventory(&execution, Some(COMMIT))?;
    let product_inventory = super::arms::inventory(&product, Some(BACKEND))?;
    evidence(
        &super::path(&args["backend"])?,
        super::boolean(&args["reconstruct"])?,
    )?;
    if source::backend_domains(&source::validate_inventory(&inventory)?)?["product"]
        != source::backend_domains(&source::validate_inventory(&product_inventory)?)?["product"]
    {
        return Err("B0 工具适配改变了 API 或 Worker 产品输入".into());
    }
    Ok(json!({"product":product,"inventory":product_inventory,"execution_inventory":inventory}))
}
