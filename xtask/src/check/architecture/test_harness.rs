use std::{collections::BTreeSet, path::Path};

use serde::Deserialize;
use toml::Table;

use super::super::{support::relative_posix, workspace::Packages};

#[path = "test_harness/attributes.rs"]
mod attributes;
#[path = "test_harness/graph.rs"]
mod graph;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Registration {
    target: String,
    source_entry: String,
    owner_test_entry: String,
}

pub(crate) fn validate_registrations(
    root: &Path,
    raw: &Table,
    packages: &Packages,
    disabled: &BTreeSet<String>,
    errors: &mut Vec<String>,
) -> BTreeSet<String> {
    let mut registered = BTreeSet::new();
    let Some(value) = raw.get("shared_bin_test_harnesses") else {
        return registered;
    };
    let registrations: Vec<Registration> = match value.clone().try_into() {
        Ok(value) => value,
        Err(error) => {
            errors.push(format!("共享测试入口登记无效：{error}"));
            return registered;
        }
    };
    for registration in registrations {
        if !registered.insert(registration.target.clone()) {
            errors.push(format!("共享测试入口重复登记：{}", registration.target));
            continue;
        }
        if disabled.contains(&registration.target) {
            errors.push(format!(
                "共享测试入口不得同时登记为空 bin：{}",
                registration.target
            ));
        }
        if let Err(error) = validate_registration(root, &registration, packages) {
            errors.push(error);
            continue;
        }
        errors.extend(validate_pair(
            root,
            &registration.source_entry,
            &registration.owner_test_entry,
        ));
    }
    registered
}

fn validate_registration(
    root: &Path,
    registration: &Registration,
    packages: &Packages,
) -> Result<(), String> {
    if registration.target != "xtask:xtask"
        || registration.source_entry != "xtask/src/main.rs"
        || registration.owner_test_entry != "xtask/tests/internal.rs"
    {
        return Err("共享测试入口仅允许显式批准的 xtask:xtask → xtask/tests/internal.rs".into());
    }
    let package = packages
        .get("xtask")
        .ok_or("共享测试入口引用不存在的 xtask crate")?;
    let source = graph::checked_path(root, &root.join(&registration.source_entry))?;
    let owner = graph::checked_path(root, &root.join(&registration.owner_test_entry))?;
    let bin = package
        .targets
        .iter()
        .find(|target| {
            target.name.as_deref() == Some("xtask") && target.kind.iter().any(|kind| kind == "bin")
        })
        .ok_or("共享测试入口引用不存在的 xtask bin")?;
    if bin.test != Some(false) || graph::checked_path(root, &bin.src_path)? != source {
        return Err("共享 xtask bin 必须设置 test = false 且 source_entry 对应实际源码".into());
    }
    let test = package
        .targets
        .iter()
        .find(|target| {
            target.name.as_deref() == Some("internal")
                && target.kind.iter().any(|kind| kind == "test")
        })
        .ok_or("共享测试入口缺少 internal 测试目标")?;
    if test.test != Some(true) || graph::checked_path(root, &test.src_path)? != owner {
        return Err("共享 owner_test_entry 必须对应启用的 internal 测试目标".into());
    }
    Ok(())
}

/// 逐个配置比较物理测试身份，不能用一个平台的可达性替代另一平台。
pub(crate) fn validate_pair(root: &Path, source: &str, owner: &str) -> Vec<String> {
    let mut errors = Vec::new();
    let mut scanner = match graph::Scanner::new(root) {
        Ok(scanner) => scanner,
        Err(error) => return vec![error],
    };
    for platform in ["windows", "linux"] {
        for resource in [false, true] {
            let variant = attributes::Variant { platform, resource };
            let result = scanner
                .collect(&root.join(source), variant)
                .and_then(|source_tests| {
                    let owner_tests = scanner.collect(&root.join(owner), variant)?;
                    let source_count = source_tests.values().filter(|case| !case.opaque).count();
                    let owner_count = owner_tests.values().filter(|case| !case.opaque).count();
                    let opaque_count = source_tests.values().filter(|case| case.opaque).count();
                    let missing = source_tests.keys().filter(|key| !owner_tests.contains_key(*key)).count();
                    println!(
                        "共享测试可达性 [{platform} resource={resource}] source_tests={source_count} owner_tests={owner_count} source_opaque={opaque_count} missing={missing}"
                    );
                    for (identity, case) in source_tests {
                        if !owner_tests.contains_key(&identity) {
                            let reason = if case.opaque {
                                "无法证明的测试生成构造"
                            } else {
                                "共享测试入口遗漏"
                            };
                            errors.push(format!(
                                "{reason} [{} resource={resource}] {}:{} {}，请装配至 {owner}",
                                platform,
                                relative_posix(root, &identity.file),
                                case.line,
                                case.name,
                            ));
                        }
                    }
                    Ok(())
                });
            if let Err(error) = result {
                errors.push(format!(
                    "共享测试入口检查失败 [{platform} resource={resource}]：{error}"
                ));
            }
        }
    }
    errors
}
