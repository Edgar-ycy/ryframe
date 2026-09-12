use std::{
    env, fs,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    process::{Output, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    Result,
    check::{TaskExecutor, TaskPlan},
    cli::{DeploymentOptions, DeploymentPhase},
    process::{child_command, run_owned},
    workspace::root_dir,
};

const NGINX_IMAGE: &str =
    "nginx@sha256:65645c7bb6a0661892a8b03b89d0743208a18dd2f3f17a54ef4b76fb8e2f2a10";
const PROMETHEUS_IMAGE: &str =
    "prom/prometheus@sha256:8672a850efe2f9874702406c8318704edb363587f8c2ca88586b4c8fdb5cea24";
pub(crate) const DEPLOYMENT_PATHS: &[&str] = &[
    ".github/workflows",
    ".cargo",
    ".dockerignore",
    "Cargo.lock",
    "Cargo.toml",
    "build.rs",
    "rust-toolchain.toml",
    "architecture",
    "catalog",
    "config",
    "crates",
    "deploy",
    "docs/operations.md",
    "locales",
    "migrations",
    "openapi",
    "scripts/check_deployment_assets.py",
    "scripts/fixtures/deploy.env",
    "vendor",
    "xtask",
];

const SOURCE_TASKS: &[TaskExecutor] = &[
    TaskExecutor::CiDeploymentChanges,
    TaskExecutor::CiDeploymentStatic,
    TaskExecutor::CiDeploymentCompose,
    TaskExecutor::CiDeploymentNginx,
    TaskExecutor::CiDeploymentPrometheus,
];
const IMAGE_TASKS: &[TaskExecutor] = &[
    TaskExecutor::CiDeploymentChanges,
    TaskExecutor::CiDeploymentImage,
];
const SKIPPED_TASKS: &[TaskExecutor] = &[TaskExecutor::CiDeploymentChanges];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeploymentCommand {
    pub(crate) program: &'static str,
    pub(crate) arguments: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CheckoutObservation {
    head: String,
    required: bool,
}

pub(super) fn plan(options: &DeploymentOptions) -> Result<TaskPlan> {
    plan_at(options, &root_dir())
}

pub(super) fn run(options: &DeploymentOptions, plan: &TaskPlan) -> Result<()> {
    run_at(options, plan, &root_dir())
}

pub(crate) fn plan_at(options: &DeploymentOptions, root: &Path) -> Result<TaskPlan> {
    let observation = observe_checkout(root, options)?;
    plan_for_required(options.phase, observation.required)
}

pub(crate) fn plan_for_required(phase: DeploymentPhase, required: bool) -> Result<TaskPlan> {
    let tasks = match (phase, required) {
        (_, false) => SKIPPED_TASKS,
        (DeploymentPhase::Source, true) => SOURCE_TASKS,
        (DeploymentPhase::Image, true) => IMAGE_TASKS,
    };
    TaskPlan::sequence(tasks)
}

#[allow(dead_code)]
pub(crate) fn deployment_required_at(root: &Path, base: &str, head: &str) -> Result<bool> {
    observe_checkout_values(root, base, head).map(|observation| observation.required)
}

pub(crate) fn run_at(options: &DeploymentOptions, plan: &TaskPlan, root: &Path) -> Result<()> {
    let original = observe_checkout(root, options)?;
    let expected = plan_for_required(options.phase, original.required)?;
    if plan != &expected {
        return Err("Security deployment 计划与当前来源及登记的原子任务不一致".into());
    }
    if options.phase == DeploymentPhase::Source {
        validate_required_output(root, options)?;
    }

    for task in &plan.tasks {
        println!("开始 CI 原子任务：{}", task.id);
        if task.executor == TaskExecutor::CiDeploymentChanges {
            continue;
        }
        require_same_observation(root, options, &original)?;
        run_task(task.executor, root, options)?;
        require_same_observation(root, options, &original)?;
    }
    require_same_observation(root, options, &original)?;
    if options.phase == DeploymentPhase::Source {
        write_required_output(root, options, original.required)?;
    }
    Ok(())
}

fn require_same_observation(
    root: &Path,
    options: &DeploymentOptions,
    original: &CheckoutObservation,
) -> Result<()> {
    let current = observe_checkout(root, options)?;
    if &current != original {
        return Err("部署门禁执行期间 Git 来源或部署变更判定发生变化".into());
    }
    Ok(())
}

fn observe_checkout(root: &Path, options: &DeploymentOptions) -> Result<CheckoutObservation> {
    let observation = observe_checkout_values(root, &options.base, &options.head)?;
    if options.phase == DeploymentPhase::Image
        && options.expected_commit.as_deref() != Some(observation.head.as_str())
    {
        return Err("镜像期望提交与实际检出 HEAD 不一致".into());
    }
    Ok(observation)
}

fn observe_checkout_values(
    root: &Path,
    base: &str,
    requested_head: &str,
) -> Result<CheckoutObservation> {
    let head = clean_checkout_head(root)?;
    if valid_git_sha(requested_head) && requested_head != head {
        return Err(format!(
            "部署门禁请求的 head 与实际检出不一致：期望 {requested_head}，实际 {head}"
        )
        .into());
    }
    let required = if !valid_git_sha(base) || !valid_git_sha(requested_head) {
        true
    } else {
        deployment_differs(root, base, requested_head)?
    };
    Ok(CheckoutObservation { head, required })
}

fn clean_checkout_head(root: &Path) -> Result<String> {
    let status = checked_git_output(root, &["status", "--porcelain=v1", "--untracked-files=all"])?;
    if !status.stdout.is_empty() {
        return Err("部署门禁要求 Git 跟踪文件保持干净".into());
    }
    let output = checked_git_output(root, &["rev-parse", "--verify", "HEAD"])?;
    let head = String::from_utf8(output.stdout)
        .map_err(|_| "Git HEAD 输出不是 UTF-8")?
        .trim()
        .to_owned();
    if !valid_git_sha(&head) {
        return Err("实际 Git HEAD 不是非零的 40 位小写十六进制 SHA".into());
    }
    Ok(head)
}

fn deployment_differs(root: &Path, base: &str, head: &str) -> Result<bool> {
    let mut arguments = vec![
        "diff",
        "--quiet",
        "--no-ext-diff",
        "--no-textconv",
        base,
        head,
        "--",
    ];
    arguments.extend_from_slice(DEPLOYMENT_PATHS);
    let output = git_output(root, &arguments)?;
    match output.status.code() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        code => Err(format!(
            "部署资产 Git 差异检查失败（退出码 {}）：{}",
            code.map_or_else(|| "signal".to_owned(), |value| value.to_string()),
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into()),
    }
}

fn checked_git_output(root: &Path, arguments: &[&str]) -> Result<Output> {
    let output = git_output(root, arguments)?;
    if output.status.success() {
        return Ok(output);
    }
    Err(format!(
        "Git 命令失败：git {}：{}",
        arguments.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    )
    .into())
}

fn git_output(root: &Path, arguments: &[&str]) -> Result<Output> {
    let mut command = child_command("git");
    command
        .args(arguments)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
        .output()
        .map_err(|error| format!("无法执行 Git 命令：{error}").into())
}

fn valid_git_sha(value: &str) -> bool {
    value.len() == 40
        && !value.bytes().all(|byte| byte == b'0')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn run_task(executor: TaskExecutor, root: &Path, options: &DeploymentOptions) -> Result<()> {
    if executor == TaskExecutor::CiDeploymentNginx {
        return run_nginx(root, options);
    }
    for command in deployment_commands(executor, root, options, None)? {
        run_owned(root, command.program, &command.arguments)?;
    }
    Ok(())
}

pub(crate) fn deployment_commands(
    executor: TaskExecutor,
    root: &Path,
    options: &DeploymentOptions,
    certificate_dir: Option<&Path>,
) -> Result<Vec<DeploymentCommand>> {
    let root = absolute_utf8(root)?;
    let command = |program, arguments| DeploymentCommand { program, arguments };
    match executor {
        TaskExecutor::CiDeploymentChanges => Ok(Vec::new()),
        TaskExecutor::CiDeploymentStatic => Ok(vec![command(
            "python",
            vec!["scripts/check_deployment_assets.py".to_owned()],
        )]),
        TaskExecutor::CiDeploymentCompose => Ok(vec![command(
            "docker",
            vec![
                "compose".to_owned(),
                "--project-directory".to_owned(),
                root.clone(),
                "--env-file".to_owned(),
                format!("{root}/scripts/fixtures/deploy.env"),
                "--file".to_owned(),
                format!("{root}/deploy/compose.prod.yml"),
                "config".to_owned(),
                "--quiet".to_owned(),
            ],
        )]),
        TaskExecutor::CiDeploymentNginx => {
            nginx_commands(&root, certificate_dir.ok_or("Nginx 校验缺少受控证书目录")?)
        }
        TaskExecutor::CiDeploymentPrometheus => Ok(prometheus_commands(&root)),
        TaskExecutor::CiDeploymentImage => Ok(vec![command(
            "python",
            vec![
                "scripts/check_deployment_assets.py".to_owned(),
                "--image".to_owned(),
                options.image.clone().ok_or("镜像阶段缺少 --image")?,
                "--expected-commit".to_owned(),
                options
                    .expected_commit
                    .clone()
                    .ok_or("镜像阶段缺少 --expected-commit")?,
            ],
        )]),
        _ => Err("非 Security deployment 节点不能由部署执行器运行".into()),
    }
}

fn nginx_commands(root: &str, certificate_dir: &Path) -> Result<Vec<DeploymentCommand>> {
    let certificate = absolute_utf8(certificate_dir)?;
    Ok(vec![
        DeploymentCommand {
            program: "openssl",
            arguments: vec![
                "req".to_owned(),
                "-x509".to_owned(),
                "-newkey".to_owned(),
                "rsa:2048".to_owned(),
                "-nodes".to_owned(),
                "-days".to_owned(),
                "1".to_owned(),
                "-subj".to_owned(),
                "/CN=example.com".to_owned(),
                "-keyout".to_owned(),
                format!("{certificate}/privkey.pem"),
                "-out".to_owned(),
                format!("{certificate}/fullchain.pem"),
            ],
        },
        DeploymentCommand {
            program: "docker",
            arguments: vec![
                "run".to_owned(),
                "--rm".to_owned(),
                "--entrypoint".to_owned(),
                "nginx".to_owned(),
                "--volume".to_owned(),
                format!("{root}/deploy/nginx/ryframe.conf:/etc/nginx/conf.d/default.conf:ro"),
                "--volume".to_owned(),
                format!("{certificate}:/etc/letsencrypt/live/example.com:ro"),
                NGINX_IMAGE.to_owned(),
                "-t".to_owned(),
            ],
        },
    ])
}

fn prometheus_commands(root: &str) -> Vec<DeploymentCommand> {
    [
        ("check", "rules", "/rules/ryframe-alerts.yml"),
        ("test", "rules", "/rules/ryframe-alerts.test.yml"),
    ]
    .into_iter()
    .map(|(operation, kind, path)| DeploymentCommand {
        program: "docker",
        arguments: vec![
            "run".to_owned(),
            "--rm".to_owned(),
            "--entrypoint".to_owned(),
            "/bin/promtool".to_owned(),
            "--volume".to_owned(),
            format!("{root}/deploy/prometheus:/rules:ro"),
            "--workdir".to_owned(),
            "/rules".to_owned(),
            PROMETHEUS_IMAGE.to_owned(),
            operation.to_owned(),
            kind.to_owned(),
            path.to_owned(),
        ],
    })
    .collect()
}

fn run_nginx(root: &Path, options: &DeploymentOptions) -> Result<()> {
    let runner_temp = verified_runner_temp()?;
    let certificate_dir = create_certificate_dir(&runner_temp)?;
    let result = deployment_commands(
        TaskExecutor::CiDeploymentNginx,
        root,
        options,
        Some(&certificate_dir),
    )
    .and_then(|commands| {
        for command in commands {
            run_owned(root, command.program, &command.arguments)?;
        }
        Ok(())
    });
    let cleanup = remove_certificate_dir(&runner_temp, &certificate_dir);
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Err(primary), Err(cleanup)) => {
            Err(format!("{primary}；清理临时证书目录又失败：{cleanup}").into())
        }
    }
}

fn verified_runner_temp() -> Result<PathBuf> {
    let configured = env::var_os("RUNNER_TEMP").ok_or("Nginx 校验缺少 RUNNER_TEMP")?;
    let path = PathBuf::from(configured);
    if !path.is_absolute() {
        return Err("RUNNER_TEMP 必须是绝对路径".into());
    }
    let metadata = fs::symlink_metadata(&path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("RUNNER_TEMP 必须是非链接目录".into());
    }
    Ok(fs::canonicalize(path)?)
}

fn create_certificate_dir(runner_temp: &Path) -> Result<PathBuf> {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    for sequence in 0..16 {
        let directory = runner_temp.join(format!(
            "ryframe-nginx-cert-{}-{nonce}-{sequence}",
            std::process::id()
        ));
        match fs::create_dir(&directory) {
            Ok(()) => return Ok(directory),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err("无法创建唯一的 Nginx 临时证书目录".into())
}

fn remove_certificate_dir(runner_temp: &Path, directory: &Path) -> Result<()> {
    let safe_name = directory
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("ryframe-nginx-cert-"));
    if directory.parent() != Some(runner_temp) || !safe_name {
        return Err("拒绝清理未经登记的 Nginx 临时证书目录".into());
    }
    for name in ["privkey.pem", "fullchain.pem"] {
        let path = directory.join(name);
        if path.exists() {
            fs::remove_file(path)?;
        }
    }
    fs::remove_dir(directory)?;
    Ok(())
}

fn write_required_output(root: &Path, options: &DeploymentOptions, required: bool) -> Result<()> {
    let output = validate_required_output(root, options)?;
    let mut file = OpenOptions::new().append(true).open(&output)?;
    writeln!(file, "required={required}")?;
    Ok(())
}

fn validate_required_output(root: &Path, options: &DeploymentOptions) -> Result<PathBuf> {
    let configured = options
        .github_output
        .as_ref()
        .ok_or("source 阶段缺少 --github-output")?;
    let metadata = fs::symlink_metadata(configured)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("--github-output 必须是已存在的非链接普通文件".into());
    }
    let output = fs::canonicalize(configured)?;
    let repository = fs::canonicalize(root)?;
    if output.starts_with(&repository) {
        return Err("--github-output 不能位于后端 Git 工作树内".into());
    }
    Ok(output)
}

fn absolute_utf8(path: &Path) -> Result<String> {
    let absolute = std::path::absolute(path)?;
    absolute
        .to_str()
        .map(|value| value.replace('\\', "/"))
        .ok_or_else(|| "部署门禁路径必须能表示为 UTF-8".into())
}
