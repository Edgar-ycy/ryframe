//! 普通恢复构建与来源代次共用的精确来源窗口；B0 使用唯一比较适配权威。
use crate::{
    Result,
    recovery::{
        clone::native::{digest, json as document, source},
        fixture_control,
    },
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(in crate::recovery) fn repository(root: &Path) -> Result<PathBuf> {
    crate::local_test_path::validate_absolute_path(
        root,
        crate::local_test_path::LocalTestPathKind::ExistingDirectory,
    )?;
    if digest::linked_path(root)?.into_iter().any(|linked| linked)
        || fixture_control::git_root(root, "恢复构建来源")? != root
    {
        return Err("恢复构建来源必须是无链接的规范Git根目录".into());
    }
    Ok(root.to_owned())
}

pub(in crate::recovery) fn registered(
    coordinator: &Path,
    execution: &Path,
    expected: &str,
    adapter: &Value,
    product: &Value,
) -> Result<Value> {
    if expected.len() != 40
        || !expected
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("构建来源提交必须是完整小写SHA".into());
    }
    let execution = repository(execution)?;
    let inventory = source::capture(&execution)?;
    if inventory.source.snapshot.head != expected || !inventory.source.snapshot.clean {
        return Err("恢复构建必须使用声明的精确干净后端源码".into());
    }
    let inventory = source::inventory_value(&inventory);
    if adapter.is_null() && product.is_null() {
        return Ok(json!({"execution":execution,"inventory":inventory,"product":null}));
    }
    let bound = crate::recovery::source::evidence::comparison::request(
        "comparison-build-source",
        &json!({
            "backend":coordinator,"source_backend":execution,"expected_head":expected,
            "adapter_contract":adapter,"product_backend":product,"reconstruct":false,
        }),
    )?;
    if bound["execution_inventory"] != inventory {
        return Err("核验B0期间执行源码发生变化".into());
    }
    Ok(json!({"execution":execution,"inventory":inventory,
        "product":{"path":bound["product"],"inventory":bound["inventory"]}}))
}

pub(super) fn verify(root: &Path, receipt: &Value, expected: &str, context: &Value) -> Result<()> {
    if receipt["kind"] != "restore-backend-build" {
        return Err("普通恢复读取只接受API/Worker构建收据".into());
    }
    super::bridge::verify_build(root, Path::new(""), receipt)?;
    let inventory = source::capture(&repository(root)?)?;
    if inventory.source.snapshot.head != expected
        || !inventory.source.snapshot.clean
        || receipt["sources"] != source::backend_domains(&inventory)?
        || receipt["build"] != *context
    {
        return Err("正式恢复必须使用精确干净SHA、当前完整来源和构建上下文".into());
    }
    Ok(())
}

pub(super) fn path(value: &Value) -> Result<&Path> {
    Ok(Path::new(document::text(value, "恢复来源路径")?))
}
