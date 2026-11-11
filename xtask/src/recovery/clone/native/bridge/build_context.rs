//! 构建上下文只记录工具链和允许工具环境的摘要，不记录环境值。
use std::{collections::BTreeMap, env, path::Path};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{Result, recovery::clone::native::digest};

#[path = "build_context/environment.rs"]
mod environment;
pub(crate) use environment::verify_registered;

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/bridge/build_context.rs"]
mod tests;

const BUILD_ENV_NAMES: &[&str] = &[
    "AR",
    "AWS_LC_SYS",
    "BINDGEN_EXTRA_CLANG_ARGS",
    "CARGO_BUILD_JOBS",
    "CARGO_BUILD_TARGET",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_HOME",
    "CARGO_INCREMENTAL",
    "CARGO_MAKEFLAGS",
    "CARGO_NET_OFFLINE",
    "CARGO_TARGET_DIR",
    "CC",
    "CC_ENABLE_DEBUG_OUTPUT",
    "CC_FORCE_DISABLE",
    "CFLAGS",
    "CLANG",
    "CLANG_PATH",
    "CMAKE",
    "CMAKE_GENERATOR",
    "CMAKE_TOOLCHAIN_FILE",
    "CRATE_CC_NO_DEFAULTS",
    "CXX",
    "CXXFLAGS",
    "CXXSTDLIB",
    "HOST",
    "INCLUDE",
    "LDFLAGS",
    "LIB",
    "LIBCLANG_PATH",
    "LIBPATH",
    "MAKE",
    "NASM",
    "NINJA",
    "OPENSSL",
    "PATH",
    "PERL",
    "PKG_CONFIG",
    "RANLIB",
    "RUSTC",
    "RUSTC_BOOTSTRAP",
    "RUSTC_LINKER",
    "RUSTC_WORKSPACE_WRAPPER",
    "RUSTC_WRAPPER",
    "RUSTDOCFLAGS",
    "RUSTFLAGS",
    "RUSTUP_HOME",
    "RUSTUP_TOOLCHAIN",
    "SDKROOT",
    "SOURCE_DATE_EPOCH",
    "TARGET",
    "UCRTVERSION",
    "UNIVERSALCRTSDKDIR",
    "VCINSTALLDIR",
    "VCPKGRS_DYNAMIC",
    "VCPKGRS_TRIPLET",
    "VCTOOLSINSTALLDIR",
    "WINDOWSSDKDIR",
    "WINDOWSSDKVERSION",
];

const BUILD_ENV_PREFIXES: &[&str] = &[
    "AR_",
    "AWS_LC_",
    "AWS_LC_SYS_",
    "BINDGEN_",
    "CARGO_PROFILE_",
    "CARGO_TARGET_",
    "CC_",
    "CFLAGS_",
    "CMAKE_",
    "CXX_",
    "CXXFLAGS_",
    "LDFLAGS_",
    "OPENSSL_",
    "PKG_CONFIG_",
    "RANLIB_",
    "RUSTFLAGS_",
    "VCPKG_",
];

pub(crate) fn current(root: &Path) -> Result<Value> {
    let mut environment = BTreeMap::new();
    for (name, value) in env::vars_os() {
        let name = name.to_str().ok_or("构建工具环境名称不是有效Unicode")?;
        if !selected(name) {
            continue;
        }
        let value = value.to_str().ok_or("所选构建工具环境值不是有效Unicode")?;
        if environment
            .insert(name.to_owned(), value.to_owned())
            .is_some()
        {
            return Err("构建工具环境包含重复名称".into());
        }
    }
    super::cargo_environment::apply(root, &mut environment)?;
    let rustc = version(root, "rustc", "-vV")?;
    let cargo = version(root, "cargo", "-V")?;
    from_inputs(&cargo, &rustc, &environment, super::build_jobs(root)?)
}

pub(in crate::recovery) fn from_inputs(
    cargo: &str,
    rustc: &str,
    environment: &BTreeMap<String, String>,
    jobs: u8,
) -> Result<Value> {
    let cargo = cargo.trim();
    let rustc = rustc.trim();
    if cargo.is_empty() || rustc.is_empty() {
        return Err("构建工具链版本输出为空".into());
    }
    let host = rustc.lines().find_map(|line| line.strip_prefix("host: "));
    let requested = environment
        .get("CARGO_BUILD_TARGET")
        .map_or("", |value| value.trim());
    let target = if requested.is_empty() {
        host.unwrap_or("")
    } else {
        requested
    };
    if target.is_empty() {
        return Err("无法确定Cargo构建目标".into());
    }
    let entries = environment.iter().filter(|(name, _)| selected(name))
        .map(|(name, value)| json!({"name":name,"sha256":hex::encode(Sha256::digest(value.as_bytes()))}))
        .collect::<Vec<_>>();
    let variables = entries
        .iter()
        .map(|entry| entry["name"].clone())
        .collect::<Vec<_>>();
    Ok(
        json!({"commands":{"api":super::build_command("api",jobs)?,"worker":super::build_command("worker",jobs)?},
        "profile":"dev","target":target,"jobs":jobs.to_string(),"toolchain":{"cargo":cargo,"rustc":rustc},
        "environment":{"variables":variables,"sha256":digest::plan_hash(&json!(entries))?}}),
    )
}

fn selected(name: &str) -> bool {
    let candidate = if cfg!(windows) {
        name.to_ascii_uppercase()
    } else {
        name.to_owned()
    };
    BUILD_ENV_NAMES.contains(&candidate.as_str())
        || BUILD_ENV_PREFIXES
            .iter()
            .any(|prefix| candidate.starts_with(prefix))
}

fn version(root: &Path, executable: &str, argument: &str) -> Result<String> {
    let mut command = crate::process::child_command(executable);
    command.current_dir(root).arg(argument);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    // 只公开结构化失败原因，不把原始输出、环境或系统错误中的路径写入诊断。
    let output = command
        .output()
        .map_err(|error| version_diagnostic(executable, argument, VersionFailure::Spawn(&error)))?;
    version_output(executable, argument, output)
}

fn version_output(
    executable: &str,
    argument: &str,
    output: std::process::Output,
) -> Result<String> {
    if !output.status.success() {
        return Err(
            version_diagnostic(executable, argument, VersionFailure::Exit(output.status)).into(),
        );
    }
    let value = String::from_utf8(output.stdout)
        .map_err(|_| version_diagnostic(executable, argument, VersionFailure::InvalidUtf8))?;
    let value = value.trim();
    if value.is_empty() {
        return Err(version_diagnostic(executable, argument, VersionFailure::Empty).into());
    }
    Ok(value.to_owned())
}

// formatter 只接受结构化原因；原始 I/O 错误正文和进程输出不参与格式化。
enum VersionFailure<'a> {
    Spawn(&'a std::io::Error),
    Probe,
    Exit(std::process::ExitStatus),
    InvalidUtf8,
    Empty,
}

fn version_diagnostic(executable: &str, argument: &str, failure: VersionFailure<'_>) -> String {
    let command = match (executable, argument) {
        ("rustc", "-vV") => "rustc -vV",
        ("cargo", "-V") => "cargo -V",
        _ => "未登记的固定版本命令",
    };
    let reason = match failure {
        VersionFailure::Spawn(error) => format!(
            "无法启动；ErrorKind={:?}，OS错误码={:?}",
            error.kind(),
            error.raw_os_error()
        ),
        VersionFailure::Exit(status) => {
            format!("未成功退出；ExitStatus={status}，code={:?}", status.code())
        }
        VersionFailure::Probe => "未能在限定时间内完成版本探测或确认进程树回收".into(),
        VersionFailure::InvalidUtf8 => "命令成功退出，但版本标准输出不是有效 UTF-8".into(),
        VersionFailure::Empty => "命令成功退出，但版本标准输出为空或仅含空白".into(),
    };
    format!(
        "读取构建工具链版本命令 {command} 失败：{reason}。修复办法：检查工作目录、PATH、执行权限及安全软件拦截，确认对应工具属于 rust-toolchain.toml 固定工具链；运行 rustup show 核对，必要时重装固定工具链或修复对应 cargo/rustc 工具及其包装器。"
    )
}
