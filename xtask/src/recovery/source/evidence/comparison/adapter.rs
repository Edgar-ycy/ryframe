//! 固定 B0 执行适配的 Git 和补丁证明；只读分支不创建索引或 Git 对象。
use super::document;
use crate::Result;
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

use crate::recovery::b0;

pub(super) fn git(
    root: &Path,
    args: &[&str],
    index: Option<&Path>,
    input: Option<&[u8]>,
) -> Result<String> {
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
    command.stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let written = if let Some(bytes) = input {
        child
            .stdin
            .take()
            .ok_or("B0 补丁标准输入不可用")?
            .write_all(bytes)
    } else {
        Ok(())
    };
    let output = child.wait_with_output()?;
    written?;
    if !output.status.success() {
        return Err("比较来源 Git 核验失败；保留现场核对固定提交、补丁和权限".into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

pub(super) fn evidence(root: &Path, reconstruct: bool) -> Result<Value> {
    let root = super::repository(root)?;
    require_original_objects(&root)?;
    let evidence = b0::evidence()?;
    require_tree(&root, b0::BACKEND, b0::BASE_TREE)?;
    if reconstruct && rebuild(&root, b0::PATCH_BYTES)? != b0::TREE {
        return Err("B0 工具补丁重建树不匹配".into());
    }
    require_original_objects(&root)?;
    require_tree(&root, b0::BACKEND, b0::BASE_TREE)?;
    Ok(evidence)
}

fn require_original_objects(root: &Path) -> Result<()> {
    for commit in [b0::BACKEND, b0::COMMIT] {
        let reference = format!("refs/replace/{commit}");
        if !git(
            root,
            &["for-each-ref", "--format=%(refname)", &reference],
            None,
            None,
        )?
        .is_empty()
        {
            return Err("B0 基线或执行适配存在 Git 替换引用；保留现场核对固定对象".into());
        }
    }
    Ok(())
}

fn require_tree(root: &Path, commit: &str, expected: &str) -> Result<()> {
    if git(
        root,
        &["rev-parse", &format!("{commit}^{{tree}}")],
        None,
        None,
    )? != expected
    {
        return Err("B0 精确提交的实际树与登记适配不同".into());
    }
    Ok(())
}

fn rebuild(root: &Path, patch: &[u8]) -> Result<String> {
    b0::validate_patch(patch)?;
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
        git(root, &["read-tree", b0::BACKEND], Some(&index), None)?;
        git(
            root,
            &["apply", "--cached", "--whitespace=nowarn", "-"],
            Some(&index),
            Some(patch),
        )?;
        git(root, &["write-tree"], Some(&index), None)
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
    if args["expected_head"] != b0::COMMIT || args["adapter_contract"] != b0::CONTRACT {
        return Err("B0 构建来源与登记适配提交不同".into());
    }
    let execution = super::path(&args["source_backend"])?;
    let product = super::path(&args["product_backend"])?;
    registered(
        &super::path(&args["backend"])?,
        &execution,
        &product,
        super::boolean(&args["reconstruct"])?,
    )
}

pub(super) fn registered(
    coordinator: &Path,
    execution: &Path,
    product: &Path,
    reconstruct: bool,
) -> Result<Value> {
    let coordinator = super::repository(coordinator)?;
    let execution = super::repository(execution)?;
    let product = super::repository(product)?;
    for root in [&coordinator, &execution, &product] {
        require_original_objects(root)?;
    }
    require_tree(&product, b0::BACKEND, b0::BASE_TREE)?;
    require_tree(&execution, b0::COMMIT, b0::TREE)?;
    let inventory = super::arms::inventory(&execution, Some(b0::COMMIT))?;
    let product_inventory = super::arms::inventory(&product, Some(b0::BACKEND))?;
    evidence(&product, reconstruct)?;
    for root in [&coordinator, &execution, &product] {
        require_original_objects(root)?;
    }
    require_tree(&product, b0::BACKEND, b0::BASE_TREE)?;
    require_tree(&execution, b0::COMMIT, b0::TREE)?;
    if super::arms::inventory(&execution, Some(b0::COMMIT))? != inventory
        || super::arms::inventory(&product, Some(b0::BACKEND))? != product_inventory
    {
        return Err("B0 原产品或精确适配执行源码在核验期间变化".into());
    }
    Ok(json!({"product":product,"inventory":product_inventory,"execution_inventory":inventory}))
}
