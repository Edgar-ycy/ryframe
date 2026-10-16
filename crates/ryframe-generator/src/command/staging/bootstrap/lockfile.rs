use std::{collections::BTreeSet, fs, path::Path, process::Command};

type Identity = (String, String, Option<String>, Option<String>);

pub(super) fn resolve(root: &Path, package: &str) -> Result<(), String> {
    let path = root.join("Cargo.lock");
    let before = read(&path)?;
    let manifest = read(&root.join("Cargo.toml"))?;
    let version = manifest
        .get("workspace")
        .and_then(|value| value.get("package"))
        .and_then(|value| value.get("version"))
        .and_then(toml::Value::as_str)
        .ok_or("Workspace 缺少 package.version；本批未安装")?;
    let output = Command::new("cargo")
        .args(["metadata", "--offline", "--format-version", "1"])
        .current_dir(root)
        .output()
        .map_err(|error| {
            format!("候选锁文件解析无法启动：{error}；确认 Cargo 已安装；本批未安装")
        })?;
    if !output.status.success() {
        return Err(format!(
            "候选依赖离线解析失败（{}）：{}；先安装原项目 Cargo.lock 锁定的依赖并检查业务清单及 patch，再重新预览；骨架、注册和锁文件均未安装",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    validate(&before, &read(&path)?, package, version)
}

fn read(path: &Path) -> Result<toml::Value, String> {
    let bytes = fs::read_to_string(path).map_err(|error| {
        format!(
            "无法读取 {}：{error}；先准备原项目锁文件；本批未安装",
            path.display()
        )
    })?;
    toml::from_str(&bytes)
        .map_err(|error| format!("{} 格式错误：{error}；本批未安装", path.display()))
}

fn identities(lock: &toml::Value) -> Result<BTreeSet<Identity>, String> {
    let packages = lock
        .get("package")
        .and_then(toml::Value::as_array)
        .ok_or("Cargo.lock 缺少 package 数组；本批未安装")?;
    let mut result = BTreeSet::new();
    for package in packages {
        let value = |name| {
            package
                .get(name)
                .and_then(toml::Value::as_str)
                .map(str::to_owned)
        };
        let identity = (
            value("name").ok_or("锁文件包名缺失")?,
            value("version").ok_or("锁文件版本缺失")?,
            value("source"),
            value("checksum"),
        );
        if !result.insert(identity) {
            return Err("Cargo.lock 包身份重复；本批未安装".into());
        }
    }
    Ok(result)
}

fn validate(
    before: &toml::Value,
    after: &toml::Value,
    package: &str,
    version: &str,
) -> Result<(), String> {
    let mut expected = identities(before)?;
    let business = (package.into(), version.into(), None, None);
    if !expected.insert(business) {
        return Err(format!(
            "锁文件已存在业务包 {package} {version}；核对原注册，不能重复创建；本批未安装"
        ));
    }
    let actual = identities(after)?;
    if expected != actual {
        let added = actual
            .difference(&expected)
            .map(|item| format!("{} {}", item.0, item.1))
            .collect::<Vec<_>>();
        let removed = expected
            .difference(&actual)
            .map(|item| format!("{} {}", item.0, item.1))
            .collect::<Vec<_>>();
        return Err(format!(
            "候选 Cargo.lock 超出新增业务包范围：额外身份={added:?}，缺失或改动身份={removed:?}；禁止附带升级、换来源或删除锁定依赖。核对 Workspace 清单和 patch 后重新预览；本批未安装"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_only_adds_the_exact_local_business_package() {
        let before: toml::Value = toml::from_str("[[package]]\nname='serde'\nversion='1.0.228'\nsource='registry+example'\nchecksum='original'").unwrap();
        let after: toml::Value = toml::from_str(&format!(
            "{}\n[[package]]\nname='orders-business'\nversion='0.13.1'",
            toml::to_string(&before).unwrap()
        ))
        .unwrap();
        validate(&before, &after, "orders-business", "0.13.1").unwrap();
        for (key, value) in [
            ("version", "1.0.229"),
            ("source", "registry+other"),
            ("checksum", "changed"),
        ] {
            let mut invalid = after.clone();
            invalid["package"].as_array_mut().unwrap()[0][key] = value.into();
            assert!(
                validate(&before, &invalid, "orders-business", "0.13.1")
                    .unwrap_err()
                    .contains("超出新增业务包范围")
            );
        }
        assert!(validate(&before, &before, "orders-business", "0.13.1").is_err());
        assert!(validate(&after, &after, "orders-business", "0.13.1").is_err());
    }
}
