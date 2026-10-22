use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use ryframe_config::{AppConfig, Environment, ToolSettings};

use super::{
    config::{DevPorts, WorkerIds},
    model::{Binaries, ServiceLaunchMode},
    runtime_config::{RuntimeConfig, RuntimeProfile},
};
use crate::{Result, cli::DevOptions, watch::WatchInputRoots};

pub(super) fn workspace(options: &DevOptions) -> Result<PathBuf> {
    let start = options
        .workspace_root
        .clone()
        .map_or_else(std::env::current_dir, Ok)?;
    let start = start.canonicalize().map_err(|error| {
        format!(
            "解析 dev workspace {} 失败：{}；请指定已有工作区目录",
            start.display(),
            error.kind()
        )
    })?;
    for root in start.ancestors() {
        let manifest = root.join("Cargo.toml");
        match fs::read_to_string(&manifest) {
            Ok(text) => {
                let table: toml::Table =
                    toml::from_str(&text).map_err(|error: toml::de::Error| {
                        format!(
                            "工作区清单 {} TOML 无效，字节位置 {:?}；请修正清单",
                            manifest.display(),
                            error.span()
                        )
                    })?;
                if table.contains_key("workspace") {
                    return Ok(root.to_owned());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!("读取工作区清单 {} 失败：error.kind={:?}，raw_os_error={:?}；请检查清单路径和读取权限",manifest.display(),error.kind(),error.raw_os_error()).into());
            }
        }
        if options.workspace_root.is_some() {
            break;
        }
    }
    Err(format!(
        "dev 在 {} 未找到 Cargo 工作区；请使用 --workspace-root 指定后端根目录",
        start.display()
    )
    .into())
}

pub(super) fn selected(root: &Path, explicit: Option<&Path>) -> Result<(PathBuf, Vec<String>)> {
    let settings = ToolSettings::load(root).map_err(|error| error.message().to_owned())?;
    let path = explicit
        .or(settings.development.config_dir.as_deref())
        .unwrap_or(Path::new("config"));
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        root.join(path)
    };
    let targets = settings.development.verify_tenant_targets;
    if targets
        .iter()
        .any(|value| value.trim().is_empty() || value.contains(['\0', '\r', '\n']))
    {
        return Err(
            "development.verify_tenant_targets 含空目标或控制字符；请修正工具文件中的目标名称"
                .into(),
        );
    }
    Ok((path, targets))
}

pub(super) fn watch_roots(root: &Path, config: &RuntimeConfig, source: PathBuf) -> WatchInputRoots {
    let locales = &config.app().app.locales_dir;
    WatchInputRoots {
        config_dir: source,
        locales_dir: if locales.is_absolute() {
            locales.clone()
        } else {
            root.join(locales)
        },
    }
}

pub(super) fn write_roles(
    config: &RuntimeConfig,
    target: &Path,
    final_path: &Path,
    previous: Option<&Binaries>,
) -> Result<()> {
    let normal = DevPorts::from_config(config.app())?;
    let probe = DevPorts::isolated()?;
    let ids = WorkerIds::from_config(config.app(), previous)?;
    let locales = final_path.join("locales");
    config.snapshot_to(&target.join("source-config"))?;
    for (name, worker, probe_mode, id) in [
        ("api", false, false, ids.api),
        ("worker", true, false, ids.worker),
        ("probe-api", false, true, ids.probe_api),
        ("probe-worker", true, true, ids.probe_worker),
        ("migrate", false, false, ids.api),
    ] {
        let ports = if probe_mode { probe } else { normal };
        config.write_profile(
            &target.join(name),
            RuntimeProfile {
                api_port: ports.api,
                worker_port: ports.worker,
                snowflake_worker_id: id,
                worker,
                probe: probe_mode,
            },
            &locales,
        )?;
    }
    Ok(())
}

pub(super) fn role_config(runtime: &Path, mode: ServiceLaunchMode, worker: bool) -> PathBuf {
    runtime.join(match (mode.is_probe(), worker) {
        (false, false) => "api",
        (false, true) => "worker",
        (true, false) => "probe-api",
        (true, true) => "probe-worker",
    })
}

pub(super) fn load_role(directory: &Path) -> Result<AppConfig> {
    AppConfig::load(directory, Environment::Dev).map_err(|error| {
        format!(
            "读取已绑定角色配置 {} 失败：{}",
            directory.display(),
            error.message()
        )
        .into()
    })
}

pub(super) fn load_ports(runtime: &Path, mode: ServiceLaunchMode) -> Result<DevPorts> {
    let api = load_role(&role_config(runtime, mode, false))?;
    let worker = load_role(&role_config(runtime, mode, true))?;
    let ports = DevPorts {
        api: api.app.port,
        worker: worker.jobs.health_port,
    };
    if ports.api == 0 || ports.worker == 0 || ports.api == ports.worker {
        return Err("角色端口非零且必须互异；请恢复完整的可信角色快照".into());
    }
    Ok(ports)
}

pub(super) fn reserved_ids(previous: Option<&Binaries>) -> Result<BTreeSet<u16>> {
    let mut reserved = BTreeSet::new();
    if let Some(previous) = previous {
        // 测量会以 probe 角色运行 LKG，同时排除其编号，避免重复启动同一实例编号。
        for name in ["api", "worker", "probe-api", "probe-worker"] {
            let value = load_role(&previous.runtime_dir.join(name))?.snowflake_worker_id;
            reserved
                .insert(u16::try_from(value).map_err(|_| "LKG 角色实例编号无效；请恢复可信快照")?);
        }
    }
    Ok(reserved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_config_overrides_local_public_and_workspace_defaults() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            selected(root.path(), None).unwrap().0,
            root.path().join("config")
        );
        fs::create_dir(root.path().join("config")).unwrap();
        fs::create_dir(root.path().join(".local-tests")).unwrap();
        fs::write(
            root.path().join("config/tools.toml"),
            "[development]\nconfig_dir='public'\nverify_tenant_targets=['shared-control']\n",
        )
        .unwrap();
        assert_eq!(
            selected(root.path(), None).unwrap().0,
            root.path().join("public")
        );
        fs::write(
            root.path().join(".local-tests/tools.toml"),
            "[development]\nconfig_dir='本机 配置'\nverify_tenant_targets=['other']\n",
        )
        .unwrap();
        let (path, targets) = selected(root.path(), Some(Path::new("explicit"))).unwrap();
        assert_eq!(path, root.path().join("explicit"));
        assert_eq!(targets, ["other"]);
        assert_eq!(
            selected(root.path(), None).unwrap().0,
            root.path().join("本机 配置")
        );
    }

    #[test]
    fn explicit_workspace_requires_a_cargo_workspace_manifest() {
        let root = tempfile::tempdir().unwrap();
        let options = DevOptions {
            workspace_root: Some(root.path().to_owned()),
            ..Default::default()
        };
        assert!(
            workspace(&options)
                .unwrap_err()
                .to_string()
                .contains("--workspace-root")
        );
        fs::write(root.path().join("Cargo.toml"), "[workspace]\nmembers=[]\n").unwrap();
        assert_eq!(
            workspace(&options).unwrap(),
            root.path().canonicalize().unwrap()
        );
    }
}
