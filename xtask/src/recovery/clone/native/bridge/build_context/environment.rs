//! 在已登记的实际构建环境中复核该侧工具链，不修改控制器进程环境。
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use serde_json::Value;

use super::{VersionFailure, from_inputs, selected, version_diagnostic, version_output};
use crate::{
    Result,
    local_test_path::{
        LocalTestPathKind, validate_absolute_path, validate_external_existing_file,
        validate_local_test_path,
    },
    process::command_output_result_with_timeout,
    recovery::{
        clone::native::digest,
        inputs::document::{self, Binding},
        monitoring::execution::ProtectedInputs,
        runtime::start::actor::system_environment,
    },
};

#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/clone/native/bridge/build_context_environment.rs"]
mod tests;

const ENVIRONMENT_BYTES: u64 = 128 * 1024;
const VERSION_BYTES: usize = 16 * 1024;
const VERSION_TIMEOUT: Duration = Duration::from_secs(15);
const FIELDS: &[&str] = &[
    "format_version",
    "kind",
    "execution_root",
    "execution_commit",
    "backend_build",
    "cargo",
    "path_rustc",
    "override_rustc",
    "environment",
];

struct Registered {
    values: BTreeMap<String, String>,
    cargo: Binding,
    rustc: Binding,
    override_rustc: Option<Binding>,
}

pub(crate) fn verify_registered(
    root: &Path,
    path: &Path,
    expected_head: &str,
    expected_build_binding: &Value,
    expected_context: &Value,
) -> Result<()> {
    validate_absolute_path(root, LocalTestPathKind::ExistingDirectory)?;
    validate_local_test_path(path, root, LocalTestPathKind::ExistingFile)?;
    check_size(path)?;
    let environment =
        ProtectedInputs::open([path.to_owned()]).map_err(|_| "无法持有登记的构建环境文件")?;
    let result = (|| {
        check_size(path)?;
        let document = document::read(path, root, "登记构建环境")
            .map_err(|_| "登记构建环境必须是无重复字段的严格 UTF-8 JSON")?;
        if document.raw.len() as u64 > ENVIRONMENT_BYTES {
            return Err("登记构建环境文件超过大小限制".into());
        }
        let registered =
            Registered::parse(&document.value, root, expected_head, expected_build_binding)?;
        let build = document::binding_shape(expected_build_binding, "原后端构建收据")?;
        validate_local_test_path(&build.path, root, LocalTestPathKind::ExistingFile)?;
        let mut paths = BTreeSet::from([build.path.clone()]);
        paths.extend(registered.tools().map(|tool| tool.path.clone()));
        let files = ProtectedInputs::open(paths).map_err(|_| "无法持有登记的构建收据及工具文件")?;
        files.finish(registered.verify(root, &build, expected_context))
    })();
    environment.finish(result)
}

impl Registered {
    fn parse(
        value: &Value,
        root: &Path,
        expected_head: &str,
        expected_build_binding: &Value,
    ) -> Result<Self> {
        document::exact(value, FIELDS, "登记构建环境")?;
        if value["format_version"].as_u64() != Some(1)
            || value["kind"].as_str() != Some("restore-backend-build-environment")
            || value["execution_root"].as_str().map(Path::new) != Some(root)
            || value["execution_commit"].as_str() != Some(expected_head)
            || &value["backend_build"] != expected_build_binding
        {
            return Err("登记构建环境与执行根、提交或原后端构建收据不符".into());
        }
        let values = environment_values(&value["environment"])?;
        let rustc_override = entry(&values, "RUSTC");
        let override_rustc = match (rustc_override, &value["override_rustc"]) {
            (None, Value::Null) => None,
            (Some(_), binding) => Some(tool_binding(binding, "RUSTC 覆盖编译器")?),
            (None, _) => return Err("无 RUSTC 环境值时不能登记覆盖编译器".into()),
        };
        Ok(Self {
            values,
            cargo: tool_binding(&value["cargo"], "Cargo")?,
            rustc: tool_binding(&value["path_rustc"], "PATH 编译器")?,
            override_rustc,
        })
    }

    fn tools(&self) -> impl Iterator<Item = &Binding> {
        [&self.cargo, &self.rustc]
            .into_iter()
            .chain(self.override_rustc.iter())
    }

    fn verify(&self, root: &Path, build: &Binding, expected: &Value) -> Result<()> {
        verify_file(build)?;
        self.verify_tool_selection(root)?;
        for tool in self.tools() {
            verify_file(tool)?;
        }
        let jobs = self.configured_jobs(root)?;
        let cargo = document::text(&expected["toolchain"]["cargo"], "原构建 Cargo 版本")?;
        let rustc = document::text(&expected["toolchain"]["rustc"], "原构建 rustc 版本")?;
        if from_inputs(cargo, rustc, &self.values, jobs)? != *expected {
            return Err("登记环境摘要、构建参数或目标与原后端构建收据不符".into());
        }
        let observed_cargo = probe(root, &self.cargo, &self.values, "cargo", "-V")?;
        let observed_rustc = probe(root, &self.rustc, &self.values, "rustc", "-vV")?;
        if let Some(tool) = &self.override_rustc
            && probe(root, tool, &self.values, "rustc", "-vV")? != rustc
        {
            return Err("登记的 RUSTC 覆盖编译器与原后端构建版本不符".into());
        }
        if from_inputs(&observed_cargo, &observed_rustc, &self.values, jobs)? != *expected {
            return Err("实际探测的工具链版本与原后端构建收据不符".into());
        }
        self.verify_tool_selection(root)?;
        if self.configured_jobs(root)? != jobs {
            return Err("构建环境核验期间配置的构建并发发生变化".into());
        }
        Ok(())
    }

    fn configured_jobs(&self, root: &Path) -> Result<u8> {
        let mut configured = self.values.clone();
        super::super::cargo_environment::apply(root, &mut configured)?;
        if configured != self.values {
            return Err("登记构建环境与当前工具文件的 Cargo 目标目录不符".into());
        }
        super::super::build_jobs(root)
    }

    fn verify_tool_selection(&self, root: &Path) -> Result<()> {
        let path = entry(&self.values, "PATH").ok_or("登记构建环境缺少 PATH")?;
        for (name, tool) in [("cargo", &self.cargo), ("rustc", &self.rustc)] {
            same_executable(&first_executable(root, path, name)?, &tool.path)?;
        }
        if let (Some(value), Some(tool)) = (entry(&self.values, "RUSTC"), &self.override_rustc) {
            let executable = Path::new(value);
            let selected = if executable.components().count() == 1 {
                first_executable(root, path, value)?
            } else {
                root.join(executable)
            };
            same_executable(&selected, &tool.path)?;
        }
        Ok(())
    }
}

fn check_size(path: &Path) -> Result<()> {
    let length = fs::symlink_metadata(path)?.len();
    if length == 0 || length > ENVIRONMENT_BYTES {
        return Err("登记构建环境文件为空或超过大小限制".into());
    }
    Ok(())
}

fn environment_values(value: &Value) -> Result<BTreeMap<String, String>> {
    let object = document::object(value, "登记构建工具环境变量")?;
    let mut seen = BTreeSet::new();
    let mut values = BTreeMap::new();
    for (name, value) in object {
        if !selected(name)
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            || !seen.insert(if cfg!(windows) {
                name.to_ascii_uppercase()
            } else {
                name.clone()
            })
        {
            return Err("登记构建环境含有不允许或重复的变量名称".into());
        }
        let value = document::text(value, "登记构建环境变量值")?;
        if value.contains('\0') {
            return Err("登记构建环境变量值含有 NUL".into());
        }
        values.insert(name.clone(), value.to_owned());
    }
    Ok(values)
}

fn entry<'a>(values: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    values.iter().find_map(|(key, value)| {
        let matches = if cfg!(windows) {
            key.eq_ignore_ascii_case(name)
        } else {
            key == name
        };
        matches.then_some(value.as_str())
    })
}

fn tool_binding(value: &Value, label: &str) -> Result<Binding> {
    let tool = document::binding_shape(value, label)?;
    validate_external_existing_file(&tool.path)
        .map_err(|_| "登记构建工具不是无链接的绝对普通文件")?;
    Ok(tool)
}

fn verify_file(binding: &Binding) -> Result<()> {
    let actual = digest::file(&binding.path).map_err(|_| "无法复核登记构建文件的内容")?;
    if actual.bytes != binding.bytes || actual.sha256 != binding.sha256 {
        return Err("实际构建收据或工具文件与登记摘要不符".into());
    }
    Ok(())
}

fn first_executable(root: &Path, path: &str, name: &str) -> Result<PathBuf> {
    let name = Path::new(name);
    if name.file_name().is_none() || name.components().count() != 1 {
        return Err("构建工具名称不是单个文件名".into());
    }
    let name = if cfg!(windows) && name.extension().is_none() {
        name.with_extension("exe")
    } else {
        name.to_owned()
    };
    for directory in env::split_paths(path) {
        let candidate = root.join(directory).join(&name);
        match fs::metadata(&candidate) {
            Ok(metadata) if is_executable(&metadata) => return Ok(candidate),
            Ok(_) => (),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) => {}
            Err(_) => return Err("无法核对登记 PATH 的工具位置".into()),
        }
    }
    Err("登记 PATH 未找到对应的构建工具".into())
}

fn is_executable(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        metadata.is_file()
    }
}

fn same_executable(selected: &Path, registered: &Path) -> Result<()> {
    for path in [selected, registered] {
        validate_external_existing_file(path)
            .map_err(|_| "构建工具路径不是无链接的绝对普通文件")?;
    }
    let canonical =
        |path: &Path| fs::canonicalize(path).map_err(|_| "无法复核构建工具的规范绝对路径");
    if canonical(selected)? != canonical(registered)? {
        return Err("实际登记 PATH 或 RUSTC 选择的工具与文件描述符不符".into());
    }
    Ok(())
}

fn probe(
    root: &Path,
    tool: &Binding,
    values: &BTreeMap<String, String>,
    name: &str,
    argument: &str,
) -> Result<String> {
    let mut command = Command::new(&tool.path);
    command
        .env_clear()
        .envs(system_environment().filter(|(key, _)| !selected(&key.to_string_lossy())))
        .envs(values)
        .current_dir(root)
        .arg(argument)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let output = command_output_result_with_timeout(command, VERSION_TIMEOUT)
        .map_err(|_| version_diagnostic(name, argument, VersionFailure::Probe))?;
    if output.stdout.len() > VERSION_BYTES {
        return Err("构建工具链版本输出超过大小限制".into());
    }
    version_output(name, argument, output)
}
