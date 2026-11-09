//! 在明确隔离的工作树中原生准备业务夹具，不启动产品服务。
#[path = "fixture_prepare/snapshot.rs"]
mod snapshot;
#[cfg(test)]
#[path = "../../tests/unit/recovery/fixture_prepare/tests.rs"]
mod tests;

use super::full_stack::{Port, context::Context, execution::Native};
use crate::{
    Result,
    cli::{FixturePrepareCommand, FixturePrepareOptions},
    local_test_path::{LocalTestPathKind, validate_absolute_path, validate_local_test_path},
    recovery::clone::native::json as documents,
};
use serde_json::{Value, json};
use std::{fs, path::Path};

pub(super) fn run(command: &FixturePrepareCommand, root: &Path, frontend: &Path) -> Result<()> {
    let FixturePrepareCommand::Run(options) = command else {
        println!(
            "cargo xtask check recovery fixture --output-dir <绝对新目录> [--expected-backend-sha <SHA> --expected-frontend-sha <SHA>] --write"
        );
        return Ok(());
    };
    validate(options, root, frontend)?;
    let context = Context {
        root: root.into(),
        output: options.output_dir.clone(),
        control: Value::Null,
        run: String::new(),
        attempt: String::new(),
    };
    let mut port = Native::new(&context)?;
    prepare(options, root, frontend, &mut port)
}

pub(crate) fn validate(
    options: &FixturePrepareOptions,
    root: &Path,
    frontend: &Path,
) -> Result<()> {
    validate_local_test_path(&options.output_dir, root, LocalTestPathKind::NewDirectory)?;
    for (path, marker) in [(root, "Cargo.toml"), (frontend, "package.json")] {
        validate_absolute_path(path, LocalTestPathKind::ExistingDirectory)?;
        if !path.join(marker).is_file() {
            return Err(format!("工作区缺少 {marker}").into());
        }
        let actual = std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["rev-parse", "--show-toplevel"])
            .output()?;
        if !actual.status.success()
            || fs::canonicalize(String::from_utf8(actual.stdout)?.trim())?
                != fs::canonicalize(path)?
        {
            return Err("业务夹具必须绑定实际 Git 工作树根目录".into());
        }
    }
    if root == frontend
        || frontend.starts_with(&options.output_dir)
        || root.starts_with(&options.output_dir)
    {
        return Err("前后端必须是独立工作树，输出不能包含源仓库".into());
    }
    let ignored = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("check-ignore")
        .arg(&options.output_dir)
        .status()?;
    if !ignored.success() {
        return Err("业务夹具输出必须被 Git 忽略".into());
    }
    Ok(())
}

fn prepare(
    options: &FixturePrepareOptions,
    root: &Path,
    frontend: &Path,
    port: &mut impl Port,
) -> Result<()> {
    let settings =
        ryframe_config::ToolSettings::load(root).map_err(|error| error.message().to_owned())?;
    let target = settings
        .generator_target_dir(root)
        .map_err(|error| error.message().to_owned())?;
    validate_absolute_path(&target, LocalTestPathKind::StateDirectory)?;
    let cache = settings
        .pnpm_cache_dir(root)
        .map_err(|error| error.message().to_owned())?;
    let sources = [snapshot::capture(root)?, snapshot::capture(frontend)?];
    if let Some(expected) = &options.expected_sources {
        snapshot::formal(&sources[0], &expected.backend_sha)?;
        snapshot::formal(&sources[1], &expected.frontend_sha)?;
    }
    if sources[0] != snapshot::capture(root)? || sources[1] != snapshot::capture(frontend)? {
        return Err("创建业务夹具前来源发生变化；未创建输出目录".into());
    }
    fs::create_dir(&options.output_dir)?;
    let backend = options.output_dir.join("backend");
    let generated_frontend = options.output_dir.join("frontend");
    let path = options.output_dir.join("fixture.json");
    let mut receipt = json!({"format_version":1,"fixture":"business","status":"preparing","fixture_sha256":null,
        "sources":{"backend":sources[0].receipt,"frontend":sources[1].receipt},"paths":{"backend":backend,"frontend":generated_frontend}});
    documents::write_new(&path, &receipt)?;
    snapshot::copy(root, &backend, &sources[0], port)?;
    snapshot::copy(frontend, &generated_frontend, &sources[1], port)?;
    fs::create_dir_all(backend.join(".local-tests"))?;
    fs::create_dir(backend.join(".local-tests/reference-fixture"))?;
    fs::write(
        backend.join(".local-tests/tools.toml"),
        format!(
            "[workspace]\nfrontend_dir = {}\n[generator]\ntarget_dir = {}\n{}",
            json!(generated_frontend),
            json!(target),
            cache
                .map(|path| format!("[verification]\npnpm_cache_dir = {}\n", json!(path)))
                .unwrap_or_default()
        ),
    )?;
    let log = options.output_dir.join("prepare.log");
    for (cwd, args) in commands(&backend, &generated_frontend, &target) {
        let program = &args[0];
        if args.get(1).map(String::as_str) == Some("__install-model") {
            install_model(root, &backend, &path, &mut receipt)?;
        } else {
            port.command_at(&cwd, program, &args[1..], Some(&log))?;
        }
    }
    finish(
        &backend,
        &generated_frontend,
        port,
        &log,
        &path,
        &mut receipt,
    )
}

fn install_model(root: &Path, backend: &Path, path: &Path, receipt: &mut Value) -> Result<()> {
    let target = backend.join("crates/order-business/src/resources/mod.rs");
    fs::copy(
        root.join("tools/python/fixtures/order-business/src/resources/mod.rs"),
        &target,
    )?;
    receipt["fixture_sha256"] = json!(crate::devex::runtime_native::artifacts::sha(&fs::read(
        target
    )?));
    documents::write_replace(path, receipt)?;
    Ok(())
}

fn finish(
    backend: &Path,
    generated_frontend: &Path,
    port: &mut impl Port,
    log: &Path,
    path: &Path,
    receipt: &mut Value,
) -> Result<()> {
    let settings =
        ryframe_config::ToolSettings::load(backend).map_err(|error| error.message().to_owned())?;
    let target = settings
        .generator_target_dir(backend)
        .map_err(|error| error.message().to_owned())?;
    let before = json!({"backend":snapshot::capture(backend)?.receipt,"frontend":snapshot::capture(generated_frontend)?.receipt});
    port.command_at(
        backend,
        "cargo",
        &cargo_arguments(
            &strings(&[
                "run",
                "--locked",
                "-p",
                "ryframe-generator",
                "--bin",
                "ryframe-generate",
                "--",
                "resource",
                "--package",
                "order-business",
            ]),
            &target,
        ),
        Some(log),
    )?;
    let after = json!({"backend":snapshot::capture(backend)?.receipt,"frontend":snapshot::capture(generated_frontend)?.receipt});
    if before != after {
        return Err("资源只读检查修改了工作树；保留未完成夹具".into());
    }
    port.command_at(
        backend,
        "cargo",
        &cargo_arguments(&strings(&["xtask", "--help"]), &target),
        Some(log),
    )?;
    receipt["status"] = json!("ready");
    receipt["generated"] = before;
    documents::write_replace(path, receipt)?;
    println!("{}", json!({"status":"ready","paths":receipt["paths"]}));
    Ok(())
}

pub(super) fn commands(
    backend: &Path,
    frontend: &Path,
    target: &Path,
) -> Vec<(std::path::PathBuf, Vec<String>)> {
    vec![
        (
            backend.into(),
            strings(&[
                "cargo",
                "generate",
                "--",
                "new-business",
                "order",
                "--write",
            ]),
        ),
        (backend.into(), strings(&["internal", "__install-model"])),
        (backend.into(), generator(&["--write"])),
        (
            frontend.into(),
            strings(&[
                "corepack",
                "pnpm",
                "install",
                "--offline",
                "--frozen-lockfile",
            ]),
        ),
        (backend.into(), generator(&["--write", "--sync-frontend"])),
        (
            backend.into(),
            strings(&["cargo", "xtask", "data", "migrate", "reconcile", "--write"]),
        ),
        (
            backend.into(),
            vec![
                "cargo".into(),
                "xtask".into(),
                "generate".into(),
                "api".into(),
                "--write".into(),
                "--frontend-dir".into(),
                frontend.to_string_lossy().into_owned(),
            ],
        ),
    ]
    .into_iter()
    .map(|(cwd, args)| {
        if args[0] == "cargo" {
            let mut command = vec!["cargo".into()];
            command.extend(cargo_arguments(&args[1..], target));
            (cwd, command)
        } else {
            (cwd, args)
        }
    })
    .collect()
}

fn cargo_arguments(args: &[String], target: &Path) -> Vec<String> {
    let mut args = match args[0].as_str() {
        "generate" => {
            let mut command = strings(&[
                "run",
                "--locked",
                "-p",
                "ryframe-generator",
                "--bin",
                "ryframe-generate",
                "--",
            ]);
            // 固定命令由 commands 构造：仅移除 alias 紧随其后的分隔符。
            assert_eq!(args[1], "--");
            command.extend_from_slice(&args[2..]);
            command
        }
        "xtask" => {
            let mut command = strings(&["run", "--locked", "-p", "xtask", "--"]);
            command.extend_from_slice(&args[1..]);
            command
        }
        "run" => args.to_vec(),
        _ => unreachable!("业务夹具只构造固定的 generate、xtask 与 run 命令"),
    };
    args.splice(
        1..1,
        ["--target-dir".into(), target.to_string_lossy().into_owned()],
    );
    args
}

fn generator(extra: &[&str]) -> Vec<String> {
    let mut args = strings(&[
        "cargo",
        "run",
        "--locked",
        "-p",
        "ryframe-generator",
        "--bin",
        "ryframe-generate",
        "--",
        "resource",
        "--package",
        "order-business",
    ]);
    args.extend(extra.iter().map(|s| s.to_string()));
    args
}
fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}
