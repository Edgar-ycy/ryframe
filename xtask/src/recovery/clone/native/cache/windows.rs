//! Windows Redis 监督控制仅消费已冻结的 v3 请求及本机收据。
use super::{
    super::{digest, json as document, resources::common},
    model, windows_tool,
};
use crate::{
    Result,
    recovery::{
        clone::native::lifecycle::identity,
        fixture_control::services::lifecycle::readonly::process,
        monitoring::execution::ProtectedInputs,
    },
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(super) mod failure;
mod observe;
mod start;
mod stop;
mod supervisor;

pub(super) use failure::accepted_failure;
pub(super) use observe::handoff::handoff_markers;
pub(in crate::recovery) use observe::incident::{redis_reboot_evidence, redis_reboot_incident};
pub(super) use observe::{
    external_termination, inspect, inspect_reboot, observe, predecessor_reboot,
    predecessor_request, reboot_exit, status,
};
pub(super) use start::run as start;
pub(super) use stop::run as stop;

const ENTRY: &str = "__windows-cache-supervisor";
const LAUNCH: &str = "launch-request.json";
const FIELDS: &[&str] = &[
    "format_version",
    "backend",
    "output",
    "owner",
    "request",
    "coordinator",
];

fn publish(path: &Path, value: &Value) -> Result<()> {
    let parent = path.parent().ok_or("缓存收据没有父目录")?;
    let pending = parent.join(format!(".{}.pending", uuid::Uuid::new_v4().simple()));
    document::write_new(&pending, value)?;
    let result = std::fs::hard_link(&pending, path);
    let cleanup = std::fs::remove_file(&pending);
    result?;
    cleanup?;
    Ok(())
}

struct BoundLaunch {
    backend: PathBuf,
    output: PathBuf,
    owner: Value,
    request: Value,
    binding: Value,
    coordinator: Value,
    reboot: bool,
}

impl BoundLaunch {
    fn create(backend: &Path, output: &Path, request: &Value) -> Result<Self> {
        common::local_directory(backend, output, "Windows Redis 启动输出")?;
        let path = output.join(LAUNCH);
        publish(
            &path,
            &json!({"format_version":3,"backend":backend,"output":output,
                "owner":identity::current()?,"request":request,
                "coordinator":common::binding(&std::env::current_exe()?)?}),
        )?;
        Self::load(&path, &digest::file(&path)?.sha256)
    }

    fn load(path: &Path, expected_sha: &str) -> Result<Self> {
        let launch = Self::load_frozen(path, expected_sha)?;
        launch.check()?;
        Ok(launch)
    }

    fn load_reboot(path: &Path, expected_sha: &str) -> Result<Self> {
        let mut launch = Self::load_frozen(path, expected_sha)?;
        launch.reboot = true;
        launch.check()?;
        Ok(launch)
    }

    fn load_frozen(path: &Path, expected_sha: &str) -> Result<Self> {
        let binding = common::binding(path)?;
        if binding["sha256"] != expected_sha {
            return Err("Windows Redis 冻结请求摘要不同".into());
        }
        let value = document::read(path)?;
        document::exact(&value, FIELDS, "Windows Redis 监督请求")?;
        let backend = bound_path(&value, "backend")?;
        let output = bound_path(&value, "output")?;
        if value["format_version"] != 3
            || value["request"]["format_version"] != 3
            || path != output.join(LAUNCH)
            || digest::linked_path(&backend)?.contains(&true)
            || std::fs::canonicalize(&backend)?
                != std::fs::canonicalize(crate::workspace::root_dir())?
        {
            return Err("Windows Redis 监督请求不属于当前工作区或目录".into());
        }
        let launch = Self {
            backend,
            output,
            owner: value["owner"].clone(),
            request: value["request"].clone(),
            binding,
            coordinator: value["coordinator"].clone(),
            reboot: false,
        };
        model::identity(&launch.owner, false)?;
        model::shape(&launch.request)?;
        Ok(launch)
    }

    fn check(&self) -> Result<()> {
        let path = common::bound_file(&self.backend, &self.binding, "Windows Redis 冻结请求")?;
        if path != self.output.join(LAUNCH) {
            return Err("Windows Redis 监督输出路径变化".into());
        }
        model::file_bound(&self.coordinator)?;
        model::validate(&self.backend, &self.request, true)?;
        if self.reboot {
            windows_tool::verify_frozen(&self.request["tool"])?;
        } else {
            windows_tool::verify_registered(&self.backend, &self.request["tool"])?;
        }
        let config = bound_path(&self.request["configuration"], "path")?;
        let expected = model::render_configuration(
            ".",
            u16::try_from(self.request["port"].as_u64().ok_or("Redis 端口无效")?)?,
            model::password(&self.request["password"])?,
        );
        if std::fs::read(config)? != expected.as_bytes() {
            return Err("Windows Redis 配置与登记的启动参数不符".into());
        }
        Ok(())
    }

    fn protect(&self) -> Result<ProtectedInputs> {
        let program = bound_path(&self.request["tool"], "path")?;
        let parent = program.parent().ok_or("Windows Redis 工具缺少父目录")?;
        let dlls = document::object(&self.request["tool"]["dlls"], "Windows Redis DLL")?;
        let mut paths = vec![
            bound_path(&self.binding, "path")?,
            self.output.join("request.json"),
            self.output.join("intent.json"),
            bound_path(&self.coordinator, "path")?,
            bound_path(&self.request["configuration"], "path")?,
        ];
        paths.extend(dlls.keys().map(|name| parent.join(name)));
        paths.push(program);
        let guard = ProtectedInputs::open(paths)?;
        guard.finish((|| {
            self.check()?;
            if document::read(&self.output.join("request.json"))? != self.request
                || document::read(&self.output.join("intent.json"))? != self.binding
            {
                return Err("Windows Redis 原请求或启动意图与冻结绑定不同".into());
            }
            Ok(())
        })())?;
        Ok(guard)
    }

    fn arguments(&self) -> Result<[&str; 3]> {
        Ok([
            ENTRY,
            document::text(&self.binding["path"], "Windows Redis 监督请求路径")?,
            document::text(&self.binding["sha256"], "Windows Redis 监督请求摘要")?,
        ])
    }

    fn check_supervisor(&self, supervisor: &Value) -> Result<()> {
        exact_arguments(
            supervisor,
            document::text(&self.coordinator["path"], "Windows Redis 监督器映像")?,
            &self.arguments()?,
        )
    }
}

fn bound_path(value: &Value, key: &str) -> Result<PathBuf> {
    Ok(PathBuf::from(document::text(
        &value[key],
        "Windows Redis 路径",
    )?))
}

fn exact_arguments(identity: &Value, executable: &str, expected: &[&str]) -> Result<()> {
    let path = document::text(&identity["executable"], "Windows Redis 内核映像")?;
    let actual = process::arguments(identity)?;
    if !same_path(path, executable)
        || !actual
            .first()
            .is_some_and(|program| same_path(program, executable))
        || !actual
            .iter()
            .skip(1)
            .map(String::as_str)
            .eq(expected.iter().copied())
    {
        return Err("Windows Redis 登记映像或完整参数变化".into());
    }
    Ok(())
}

fn same_path(left: &str, right: &str) -> bool {
    left.replace('\\', "/")
        .eq_ignore_ascii_case(&right.replace('\\', "/"))
}

pub(super) fn run_hidden_if_requested() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next()?.as_os_str() != ENTRY {
        return None;
    }
    let result = (|| {
        let path = PathBuf::from(args.next().ok_or("缺少 Windows Redis 监督请求")?);
        let sha = args.next().ok_or("缺少 Windows Redis 监督请求摘要")?;
        if args.next().is_some() {
            return Err("Windows Redis 监督入口参数无效".into());
        }
        let launch = BoundLaunch::load(&path, sha.to_str().ok_or("无效监督摘要")?)?;
        if launch.coordinator != common::binding(&std::env::current_exe()?)? {
            return Err("Windows Redis 监督器与原工具绑定不同".into());
        }
        supervisor::serve(&launch)
    })();
    Some(i32::from(result.is_err()))
}
