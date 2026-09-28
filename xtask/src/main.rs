use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    if let Err(error) = run(env::args_os().skip(1).collect()) {
        eprintln!("xtask: {error}");
        std::process::exit(1);
    }
}

fn run(args: Vec<OsString>) -> Result<(), String> {
    let root = workspace_root()?;
    let mut args = args.into_iter();
    let command = args.next().unwrap_or_else(|| OsString::from("help"));

    match command.to_string_lossy().as_ref() {
        "help" | "--help" | "-h" => print_help(),
        "check" => run_check(&root, args.collect()),
        "build" => run_build(&root, args.collect()),
        "dev" => run_backend(&root, "ryframe", args.collect()),
        "data" => run_data(&root, args.collect()),
        "generate" => Err("资源生成入口已从 xtask 移除，请使用对应的后端生成命令".into()),
        other => Err(format!(
            "未知命令 `{other}`，使用 `cargo xtask help` 查看帮助"
        )),
    }
}

fn workspace_root() -> Result<PathBuf, String> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "无法定位后端工作区".into())
}

fn run_check(root: &Path, args: Vec<OsString>) -> Result<(), String> {
    let plan = args.iter().any(|arg| arg == "--plan");
    for arg in &args {
        if arg != "--plan" && arg != "--full" {
            return Err(format!("check 不支持参数 `{}`", arg.to_string_lossy()));
        }
    }
    if plan {
        println!("cargo check --workspace --all-targets --locked");
        return Ok(());
    }
    run_cargo(root, ["check", "--workspace", "--all-targets", "--locked"])
}

fn run_build(root: &Path, args: Vec<OsString>) -> Result<(), String> {
    let mut profile = String::from("dev");
    let mut plan = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.to_string_lossy().as_ref() {
            "--plan" => plan = true,
            "--profile" => {
                profile = args
                    .next()
                    .ok_or_else(|| "--profile 缺少值".to_string())?
                    .into_string()
                    .map_err(|_| "--profile 不是有效文本".to_string())?;
                if profile != "dev" && profile != "release" {
                    return Err("--profile 只支持 dev 或 release".into());
                }
            }
            other => return Err(format!("build 不支持参数 `{other}`")),
        }
    }

    if plan {
        println!(
            "cargo build --workspace --locked{}",
            if profile == "release" {
                " --release"
            } else {
                ""
            }
        );
        return Ok(());
    }

    if profile == "release" {
        run_cargo(root, ["build", "--workspace", "--locked", "--release"])
    } else {
        run_cargo(root, ["build", "--workspace", "--locked"])
    }
}

fn run_backend(root: &Path, package: &str, args: Vec<OsString>) -> Result<(), String> {
    let mut command = vec![
        OsString::from("run"),
        OsString::from("--locked"),
        OsString::from("-p"),
        OsString::from(package),
        OsString::from("--bin"),
        OsString::from(package),
    ];
    if !args.is_empty() {
        command.push(OsString::from("--"));
        command.extend(args);
    }
    run_cargo_os(root, command)
}

fn run_data(root: &Path, args: Vec<OsString>) -> Result<(), String> {
    let mut args = args.into_iter();
    let operation = args
        .next()
        .ok_or_else(|| "data 缺少操作名，目前支持 migrate".to_string())?;
    if operation != "migrate" {
        return Err(format!("data 不支持操作 `{}`", operation.to_string_lossy()));
    }

    let mut command = vec![
        OsString::from("run"),
        OsString::from("--locked"),
        OsString::from("-p"),
        OsString::from("ryframe"),
        OsString::from("--bin"),
        OsString::from("ryframe-migrate"),
        OsString::from("--"),
    ];
    command.extend(args);
    run_cargo_os(root, command)
}

fn run_cargo<const N: usize>(root: &Path, args: [&str; N]) -> Result<(), String> {
    run_cargo_os(root, args.into_iter().map(OsString::from).collect())
}

fn run_cargo_os(root: &Path, args: Vec<OsString>) -> Result<(), String> {
    let status = Command::new("cargo")
        .args(args)
        .current_dir(root)
        .status()
        .map_err(|error| format!("启动 cargo 失败: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("cargo 退出状态为 {status}"))
    }
}

fn print_help() -> Result<(), String> {
    println!(
        "RyFrame 后端任务入口\n\n  cargo xtask dev [参数]\n  cargo xtask check [--full|--plan]\n  cargo xtask build [--profile dev|release] [--plan]\n  cargo xtask data migrate [参数]\n"
    );
    Ok(())
}
