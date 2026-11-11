//! 在原控制锁内登记恢复目标的三端零进程状态。
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use serde_json::{Number, Value, json};

use super::{
    bind::{
        control::runtime_target,
        ownership::{Owner, control_root},
        validate_output,
    },
    dataset_authority::{configuration_registration, reads::Window},
    verify::same_path,
};
use crate::{
    Result,
    cli::RuntimeRegistrationOptions,
    local_test_path::{LocalTestPathKind, validate_absolute_path, validate_local_test_path},
    recovery::{
        clone::native::{resources::common, target_verify::authority::require_closed},
        fixture_control::{git_root, portable_path},
        inputs::{document, publish::publish_verified, target_plan},
    },
};

const ROLES: [&str; 3] = ["api", "worker", "frontend"];

#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    directory: PathBuf,
    configuration: configuration_registration::Source,
    receipt: Value,
}

pub(super) fn run(root: &Path, options: &RuntimeRegistrationOptions) -> Result<Value> {
    let backend = backend_root(root)?;
    register(&backend, &options.output, || {
        Snapshot::observe(&backend, &options.plan, &options.target_plan)
    })
}

pub(super) fn backend_root(root: &Path) -> Result<PathBuf> {
    let backend = git_root(root, "恢复运行登记后端")?;
    if backend != portable_path(&root.canonicalize()?) {
        return Err("恢复运行登记后端必须是实际 Git 工作树根".into());
    }
    Ok(backend)
}

pub(super) fn verify_not_started(
    backend: &Path,
    reference: &Path,
    target: &Path,
    registration_path: &Path,
    registration: &Value,
) -> Result<()> {
    let mut observed = Snapshot::observe(backend, reference, target)?;
    let environment = &registration["runtime_environment"];
    observed.receipt["runtime_environment"] = environment.clone();
    if observed.receipt != *registration {
        return Err("fresh target 运行目录或三端口不再保持登记的零进程状态".into());
    }
    configuration_registration::verify(
        backend,
        registration_path,
        environment,
        &observed.configuration,
    )
}

fn register(
    backend: &Path,
    selected_output: &Path,
    observe: impl Fn() -> Result<Snapshot>,
) -> Result<Value> {
    let window = Window::direct()?;
    let result = (|| {
        let before = observe()?;
        let output = validate_output(backend, selected_output)?;
        require_outside_runtime(&output, &before.directory)?;
        let configuration = configuration_registration::sidecar(&output)?;
        require_outside_runtime(&configuration, &before.directory)?;
        require_outside_runtime(&before.directory, &configuration)?;
        let control = control_root(backend)?;
        let digest = document::digest(&before.receipt["target_plan"]["sha256"], "目标计划摘要")?;
        Owner::with(&control, &format!("register:{digest}"), |owner| {
            let result = register_locked(backend, &output, &before, owner, &window, &observe);
            window.finish(result)
        })
    })();
    window.finish(result)
}

fn register_locked(
    backend: &Path,
    output: &Path,
    before: &Snapshot,
    owner: &Owner,
    window: &Window,
    observe: &impl Fn() -> Result<Snapshot>,
) -> Result<Value> {
    owner.check()?;
    let environment = before.configuration.publish(backend, output)?;
    let mut receipt = before.receipt.clone();
    receipt["runtime_environment"] = environment.clone();
    let mut published = false;
    publish_verified(output, &receipt, || {
        owner.check()?;
        let current = observe().map_err(|error| -> Box<dyn std::error::Error> {
            if published {
                format!("恢复运行登记发布后复核失败，保留登记文件：{error}").into()
            } else {
                error
            }
        })?;
        owner.check()?;
        if current != *before {
            return Ok(false);
        }
        configuration_registration::verify(backend, output, &environment, &current.configuration)?;
        if !published {
            if validate_output(backend, output)? != output {
                return Err("恢复运行登记输出路径在持锁期间发生变化".into());
            }
            published = true;
        }
        window.finish(Ok(true))
    })?;
    let current = document::read(output, backend, "恢复运行登记")?;
    if current.raw != document::canonical_bytes(&receipt)? {
        return Err("恢复运行登记发布后字节变化，保留现场".into());
    }
    owner.check()?;
    Ok(json!({
        "status":"runtime_registered_not_started",
        "registration":document::binding(&current),
    }))
}

impl Snapshot {
    fn observe(backend: &Path, reference: &Path, target: &Path) -> Result<Self> {
        let reference_path = protocol_path(reference)?;
        let target_path = protocol_path(target)?;
        let reference = document::read(&reference_path, backend, "参考恢复计划")?;
        let target = document::read(&target_path, backend, "正式恢复目标计划")?;
        let (chain, verified, _) = target_plan::verify(backend, reference, &target.path)?;
        if verified != target.value {
            return Err("正式恢复目标计划核验结果与登记文件不同".into());
        }
        let (runtime, endpoints) = runtime_target(&chain.reference.value, &target.value)?;
        let (directory, runtime) = runtime_observation(backend, &runtime)?;
        let ports_idle = closed_roles(&endpoints, require_closed)?;
        chain.reference.assert_unchanged()?;
        target.assert_unchanged()?;
        Ok(Self {
            directory,
            configuration: configuration_registration::Source::from_target(&chain.arm.target)?,
            receipt: json!({
                "format_version":2, "kind":"restore-runtime-registration",
                "reference_plan":document::binding(&chain.reference),
                "target_plan":document::binding(&target),
                "observation":{"runtime":runtime,"ports_idle":ports_idle},
                "remote_writes":0,
            }),
        })
    }
}

pub(super) fn closed_roles(
    endpoints: &Value,
    require_closed: impl Fn(&str) -> Result<()>,
) -> Result<[&'static str; 3]> {
    for role in ROLES {
        require_closed(document::text(&endpoints[role], "恢复运行角色端点")?)?;
    }
    Ok(ROLES)
}

fn require_outside_runtime(output: &Path, directory: &Path) -> Result<()> {
    if output
        .ancestors()
        .any(|ancestor| same_path(ancestor, directory))
    {
        return Err("恢复运行登记不能写入需要证明为空的目标运行目录".into());
    }
    Ok(())
}

pub(super) fn runtime_observation(backend: &Path, runtime: &Path) -> Result<(PathBuf, Value)> {
    validate_local_test_path(runtime, backend, LocalTestPathKind::StateDirectory)?;
    let anchor = nearest_anchor(runtime)?;
    let exists = anchor == runtime;
    let before = directory_identity(anchor)?;
    let mut directory = portable_path(&anchor.canonicalize()?);
    if exists {
        if fs::read_dir(anchor)?.next().transpose()?.is_some() {
            return Err("fresh target 运行目录已出现 lifecycle、launch、进程树或未知文件".into());
        }
    } else {
        directory.push(runtime.strip_prefix(anchor)?);
    }
    validate_local_test_path(runtime, backend, LocalTestPathKind::StateDirectory)?;
    if nearest_anchor(runtime)? != anchor || directory_identity(anchor)? != before {
        return Err("fresh target 运行目录状态或身份在观察期间发生变化".into());
    }
    Ok((
        directory,
        json!({
            "exists":exists, "entries":[],
            "anchor":{"path":protocol_path(anchor)?, "device":before.0,
                "inode":before.1, "created_ns":time_number(before.2)?},
        }),
    ))
}

fn nearest_anchor(runtime: &Path) -> Result<&Path> {
    for path in runtime.ancestors() {
        match fs::symlink_metadata(path) {
            Ok(_) => return Ok(path),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Err("fresh target 运行目录没有可信现有父目录".into())
}

fn directory_identity(path: &Path) -> Result<(u64, u64, i128)> {
    validate_absolute_path(path, LocalTestPathKind::ExistingDirectory)?;
    let metadata = fs::symlink_metadata(path)?;
    let (device, inode) = common::device_inode_for(path, &metadata)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // 与 Windows Python stat 的目录创建时间采用同一 Unix 纳秒纪元。
        let created = i128::from(metadata.creation_time()) * 100 - 11_644_473_600_000_000_000;
        Ok((device, inode, created))
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let changed =
            i128::from(metadata.ctime()) * 1_000_000_000 + i128::from(metadata.ctime_nsec());
        Ok((device, inode, changed))
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = (device, inode);
        Err("当前平台无法核验恢复运行目录的时间身份".into())
    }
}

fn time_number(nanoseconds: i128) -> Result<Number> {
    if let Ok(value) = i64::try_from(nanoseconds) {
        return Ok(Number::from(value));
    }
    let value = u64::try_from(nanoseconds).map_err(|_| "恢复运行目录时间身份超出 JSON 整数范围")?;
    Ok(Number::from(value))
}

pub(super) fn protocol_path(path: &Path) -> Result<PathBuf> {
    let text = path.to_str().ok_or("恢复运行登记路径必须为 UTF-8")?;
    #[cfg(windows)]
    let text = text.replace('/', "\\");
    Ok(PathBuf::from(text))
}

#[cfg(test)]
#[path = "../../../tests/unit/recovery/runtime/registration.rs"]
mod tests;
