use std::{
    env, fs,
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::{
    Result, doctor,
    process::{ChildGroup, spawn_pnpm_with_env, stop_child},
    watch::{SourceWatcher, WatchEvent},
    workspace::root_dir,
};

const HEALTH_TIMEOUT: Duration = Duration::from_secs(30);
const WATCH_DEBOUNCE: Duration = Duration::from_millis(350);
const LOOP_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug, Clone)]
struct Binaries {
    api: PathBuf,
    worker: PathBuf,
}

struct Services {
    api: Child,
    worker: Child,
    binaries: Binaries,
}

struct RunningProcesses {
    services: Services,
    vite: Child,
}

impl Drop for RunningProcesses {
    fn drop(&mut self) {
        let _ = stop_all(&mut self.services, &mut self.vite);
        let _ = cleanup_binaries(&root_dir(), &self.services.binaries);
    }
}

enum BuildResult {
    Ready(Binaries),
    Failed,
    Cancelled,
}

pub(crate) fn run(frontend_dir: &Path) -> Result<()> {
    doctor::run(frontend_dir)?;
    let root = root_dir();
    let ports = DevPorts::from_environment()?;
    ensure_initial_ports_available(ports)?;
    let worker_ids = WorkerIds::from_environment()?;
    let group = ChildGroup::new()?;
    let watcher = SourceWatcher::new(&root)?;
    let shutdown_requested = Arc::new(AtomicBool::new(false));
    let _shutdown_listener = spawn_shutdown_listener(Arc::clone(&shutdown_requested));

    let proxy_target = format!("http://127.0.0.1:{}", ports.api);
    let mut vite = spawn_pnpm_with_env(
        &group,
        frontend_dir,
        &["dev"],
        &[("VITE_APP_PROXY_TARGET", proxy_target.as_str())],
    )?;
    println!("Vite 已启动；正在构建 API、Worker 与迁移工具。按 Ctrl+C 可统一停止。");
    let initial = match build_candidate(&group, &root, &shutdown_requested)? {
        BuildResult::Ready(candidate) => candidate,
        BuildResult::Failed => {
            stop_child(&mut vite)?;
            return Err("初始后端构建失败，尚无 last-known-good 可继续运行".into());
        }
        BuildResult::Cancelled => {
            stop_child(&mut vite)?;
            return Ok(());
        }
    };
    // 首次启动没有 last-known-good，必须由正常模式完成依赖 provision；只读 probe 仅用于后续热切换。
    let services = match start_services(&group, &root, initial, ports, worker_ids) {
        Ok(services) => services,
        Err(error) => {
            let _ = stop_child(&mut vite);
            return Err(error);
        }
    };
    let mut running = RunningProcesses { services, vite };
    println!(
        "开发服务已就绪：API http://127.0.0.1:{}/readyz，Worker http://127.0.0.1:{}/readyz。",
        ports.api, ports.worker
    );

    loop {
        if shutdown_requested.load(Ordering::Acquire) {
            println!("收到中断信号，正在停止 API、Worker 与 Vite。");
            stop_all(&mut running.services, &mut running.vite)?;
            return Ok(());
        }
        ensure_running("API", &mut running.services.api)?;
        ensure_running("Worker", &mut running.services.worker)?;
        ensure_running("Vite", &mut running.vite)?;

        let Some(event) = watcher.recv_timeout(LOOP_INTERVAL)? else {
            continue;
        };
        let event = match event {
            WatchEvent::Failed(error) => return Err(error.into()),
            WatchEvent::BackendChanged(path) => watcher.drain_changes(path, WATCH_DEBOUNCE)?,
        };
        let changed_path = match event {
            WatchEvent::BackendChanged(path) => path,
            WatchEvent::Failed(error) => return Err(error.into()),
        };
        println!("检测到后端变更 {changed_path}，后台编译新候选；当前服务继续运行。");
        match build_candidate(&group, &root, &shutdown_requested)? {
            BuildResult::Failed => {
                println!("候选构建或迁移校验失败，继续使用 last-known-good。");
            }
            BuildResult::Cancelled => {
                stop_all(&mut running.services, &mut running.vite)?;
                return Ok(());
            }
            BuildResult::Ready(candidate) => {
                if let Err(error) = probe_candidate(&group, &root, &candidate, worker_ids) {
                    println!(
                        "候选 API/Worker 未通过隔离健康检查，继续使用 last-known-good：{error}"
                    );
                    let _ = cleanup_binaries(&root, &candidate);
                    continue;
                }
                if promote_services(
                    &group,
                    &root,
                    &mut running.services,
                    candidate,
                    ports,
                    worker_ids,
                )? {
                    println!("新候选健康检查通过，API 与 Worker 已切换。");
                }
            }
        }
    }
}

fn build_candidate(group: &ChildGroup, root: &Path, shutdown: &AtomicBool) -> Result<BuildResult> {
    let started = Instant::now();
    let mut build = Command::new("cargo");
    build
        .args([
            "build",
            "--locked",
            "-p",
            "ryframe",
            "--bin",
            "ryframe",
            "--bin",
            "ryframe-worker",
            "--bin",
            "ryframe-migrate",
        ])
        .current_dir(root)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let mut child = group.spawn(&mut build)?;
    let Some(status) = wait_command(&mut child, shutdown)? else {
        return Ok(BuildResult::Cancelled);
    };
    if !status.success() {
        println!("候选编译失败（{:.1}s）。", started.elapsed().as_secs_f64());
        return Ok(BuildResult::Failed);
    }

    let target = target_debug_dir(root);
    let migration_binary = target.join(binary_name("ryframe-migrate"));
    let mut verify = Command::new(&migration_binary);
    verify
        .args(["control", "verify"])
        .env("APP_ENV", "dev")
        .env("APP_DATABASE_MIGRATION_MODE", "verify")
        .current_dir(root)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let mut child = group.spawn(&mut verify).map_err(|error| {
        format!(
            "无法启动迁移校验工具 {}：{error}",
            migration_binary.display()
        )
    })?;
    let Some(status) = wait_command(&mut child, shutdown)? else {
        return Ok(BuildResult::Cancelled);
    };
    if !status.success() {
        println!("候选迁移 verify 失败；数据库未被修改。");
        return Ok(BuildResult::Failed);
    }

    let generation =
        root.join("target/xtask/dev")
            .join(format!("{}-{}", std::process::id(), nonce()?));
    fs::create_dir_all(&generation)?;
    let copied = (|| {
        let api = copy_binary(&target.join(binary_name("ryframe")), &generation)?;
        let worker = copy_binary(&target.join(binary_name("ryframe-worker")), &generation)?;
        Ok::<_, Box<dyn std::error::Error>>(Binaries { api, worker })
    })();
    let binaries = match copied {
        Ok(binaries) => binaries,
        Err(error) => {
            let _ = remove_generation_dir(root, &generation);
            return Err(error);
        }
    };
    println!(
        "候选构建与迁移校验完成（{:.1}s）。",
        started.elapsed().as_secs_f64()
    );
    Ok(BuildResult::Ready(binaries))
}

fn copy_binary(source: &Path, generation: &Path) -> Result<PathBuf> {
    if !source.is_file() {
        return Err(format!("Cargo 构建未生成预期二进制：{}", source.display()).into());
    }
    let name = source
        .file_name()
        .ok_or_else(|| format!("二进制路径缺少文件名：{}", source.display()))?;
    let target = generation.join(name);
    let staged = generation.join(format!(".{}.copying", name.to_string_lossy()));
    fs::copy(source, &staged)?;
    fs::rename(&staged, &target)?;
    Ok(target)
}

fn wait_command(child: &mut Child, shutdown: &AtomicBool) -> Result<Option<ExitStatus>> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if shutdown.load(Ordering::Acquire) {
            stop_child(child)?;
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn probe_candidate(
    group: &ChildGroup,
    root: &Path,
    binaries: &Binaries,
    worker_ids: WorkerIds,
) -> Result<()> {
    let (api_port, worker_port) = available_ports()?;
    let mut command = api_command(
        root,
        &binaries.api,
        api_port,
        worker_port,
        worker_ids.probe_api,
        true,
    );
    let mut api = group.spawn(&mut command)?;
    let mut worker_command = worker_command(
        root,
        &binaries.worker,
        worker_port,
        worker_ids.probe_worker,
        true,
    );
    let mut worker = match group.spawn(&mut worker_command) {
        Ok(worker) => worker,
        Err(error) => {
            let api_stop = stop_child(&mut api);
            return combine_failures(Err(error), [("停止候选 API", api_stop)]);
        }
    };
    let health_deadline = Instant::now() + HEALTH_TIMEOUT;
    let result = wait_services_ready_until(
        health_deadline,
        "候选 API",
        "候选 Worker",
        || ensure_running("候选 API", &mut api),
        || ensure_running("候选 Worker", &mut worker),
        || http_readyz(api_port),
        || http_readyz(worker_port),
    );
    let api_stop = stop_child(&mut api);
    let worker_stop = stop_child(&mut worker);
    combine_failures(
        result,
        [("停止候选 API", api_stop), ("停止候选 Worker", worker_stop)],
    )
}

pub(crate) fn combine_failures<T, const N: usize>(
    result: Result<T>,
    stops: [(&str, Result<()>); N],
) -> Result<T> {
    let mut errors = Vec::new();
    let value = match result {
        Ok(value) => Some(value),
        Err(error) => {
            errors.push(error.to_string());
            None
        }
    };
    for (label, stop) in stops {
        if let Err(error) = stop {
            errors.push(format!("{label} 失败：{error}"));
        }
    }
    if errors.is_empty() {
        Ok(value.expect("没有错误时必须保留主操作结果"))
    } else {
        Err(errors.join("；").into())
    }
}

pub(crate) fn start_worker_after_api_ready<T>(
    wait_for_api: impl FnOnce() -> Result<()>,
    start_worker: impl FnOnce() -> Result<T>,
) -> Result<T> {
    wait_for_api()?;
    start_worker()
}

fn start_services(
    group: &ChildGroup,
    root: &Path,
    binaries: Binaries,
    ports: DevPorts,
    worker_ids: WorkerIds,
) -> Result<Services> {
    let mut api_command = api_command(
        root,
        &binaries.api,
        ports.api,
        ports.worker,
        worker_ids.api,
        false,
    );
    let mut api = match group.spawn(&mut api_command) {
        Ok(api) => api,
        Err(error) => {
            let cleanup = cleanup_binaries(root, &binaries);
            return combine_failures(Err(error), [("清理候选版本目录", cleanup)]);
        }
    };
    let health_deadline = Instant::now() + HEALTH_TIMEOUT;
    let mut worker_command = worker_command(
        root,
        &binaries.worker,
        ports.worker,
        worker_ids.worker,
        false,
    );
    let mut worker = match start_worker_after_api_ready(
        || wait_healthy_until(&mut api, ports.api, "API", health_deadline),
        || group.spawn(&mut worker_command),
    ) {
        Ok(worker) => worker,
        Err(error) => {
            let api_stop = stop_child(&mut api);
            let cleanup = cleanup_binaries(root, &binaries);
            return combine_failures(
                Err(error),
                [
                    ("停止未完成启动的 API", api_stop),
                    ("清理候选版本目录", cleanup),
                ],
            );
        }
    };
    if let Err(error) = wait_services_ready_until(
        health_deadline,
        "API",
        "Worker",
        || ensure_running("API", &mut api),
        || ensure_running("Worker", &mut worker),
        || http_readyz(ports.api),
        || http_readyz(ports.worker),
    ) {
        let api_stop = stop_child(&mut api);
        let worker_stop = stop_child(&mut worker);
        let cleanup = cleanup_binaries(root, &binaries);
        return combine_failures(
            Err(error),
            [
                ("停止未通过健康检查的 API", api_stop),
                ("停止未通过健康检查的 Worker", worker_stop),
                ("清理候选版本目录", cleanup),
            ],
        );
    }
    Ok(Services {
        api,
        worker,
        binaries,
    })
}

pub(crate) fn api_command(
    root: &Path,
    binary: &Path,
    api_port: u16,
    worker_port: u16,
    worker_id: u16,
    probe: bool,
) -> Command {
    let mut command = Command::new(binary);
    command
        .env("APP_ENV", "dev")
        .env("APP_DATABASE_MIGRATION_MODE", "verify")
        .env("APP_JOBS_MODE", "external")
        .env("APP_APP_PORT", api_port.to_string())
        .env("APP_JOBS_HEALTH_PORT", worker_port.to_string())
        .env("SNOWFLAKE_WORKER_ID", worker_id.to_string())
        .current_dir(root)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    if probe {
        command.arg("--probe");
        command
            .env("APP_APP_HOST", "127.0.0.1")
            .env("APP_TELEMETRY_ENABLED", "false")
            .env("APP_LOGGER_OUTPUT", "stdout");
    }
    command
}

pub(crate) fn worker_command(
    root: &Path,
    binary: &Path,
    port: u16,
    worker_id: u16,
    probe: bool,
) -> Command {
    let mut command = Command::new(binary);
    command
        .env("APP_ENV", "dev")
        .env("APP_DATABASE_MIGRATION_MODE", "verify")
        .env("APP_JOBS_MODE", "external")
        .env("APP_JOBS_HEALTH_PORT", port.to_string())
        .env("SNOWFLAKE_WORKER_ID", worker_id.to_string())
        .env("APP_JOBS_WORKER_ID", "ryframe-dev-worker")
        .current_dir(root)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    if probe {
        command.arg("--probe");
        command
            .env("APP_JOBS_WORKER_ID", "ryframe-dev-probe")
            .env("APP_JOBS_HEALTH_HOST", "127.0.0.1")
            .env("APP_TELEMETRY_ENABLED", "false")
            .env("APP_LOGGER_OUTPUT", "stdout");
    }
    command
}

fn promote_services(
    group: &ChildGroup,
    root: &Path,
    current: &mut Services,
    candidate: Binaries,
    ports: DevPorts,
    worker_ids: WorkerIds,
) -> Result<bool> {
    let previous = current.binaries.clone();
    let restore = previous.clone();
    switch_services_with_rollback(
        current,
        stop_services,
        || start_services(group, root, candidate, ports, worker_ids),
        || start_services(group, root, restore, ports, worker_ids),
        || {
            let _ = cleanup_binaries(root, &previous);
        },
    )
}

pub(crate) fn switch_services_with_rollback<T>(
    current: &mut T,
    stop_current: impl FnOnce(&mut T) -> Result<()>,
    start_candidate: impl FnOnce() -> Result<T>,
    restore_previous: impl FnOnce() -> Result<T>,
    on_promoted: impl FnOnce(),
) -> Result<bool> {
    stop_current(current)?;
    match start_candidate() {
        Ok(next) => {
            on_promoted();
            *current = next;
            Ok(true)
        }
        Err(candidate_error) => {
            println!("候选在正式端口启动失败，正在恢复 last-known-good：{candidate_error}");
            match restore_previous() {
                Ok(previous) => {
                    *current = previous;
                    Ok(false)
                }
                Err(restore_error) => Err(format!(
                    "候选在正式端口启动失败：{candidate_error}；last-known-good 恢复失败：{restore_error}"
                )
                .into()),
            }
        }
    }
}

fn wait_healthy_until(
    child: &mut Child,
    port: u16,
    label: &str,
    health_deadline: Instant,
) -> Result<()> {
    loop {
        ensure_running(label, child)?;
        if Instant::now() >= health_deadline {
            return Err(format!(
                "{label} 未在共享 {} 秒截止时间内通过 /readyz",
                HEALTH_TIMEOUT.as_secs()
            )
            .into());
        }
        let ready = http_readyz(port);
        if ready && Instant::now() <= health_deadline {
            ensure_running(label, child)?;
            return Ok(());
        }
        thread::sleep(LOOP_INTERVAL.min(health_deadline.saturating_duration_since(Instant::now())));
    }
}

pub(crate) fn wait_services_ready_until<EA, EW, RA, RW>(
    health_deadline: Instant,
    api_label: &str,
    worker_label: &str,
    mut ensure_api_running: EA,
    mut ensure_worker_running: EW,
    mut api_ready: RA,
    mut worker_ready: RW,
) -> Result<()>
where
    EA: FnMut() -> Result<()>,
    EW: FnMut() -> Result<()>,
    RA: FnMut() -> bool,
    RW: FnMut() -> bool,
{
    loop {
        ensure_api_running()?;
        ensure_worker_running()?;
        if Instant::now() >= health_deadline {
            return Err(format!(
                "{api_label} 与 {worker_label} 未在共享 {} 秒截止时间内同时通过 /readyz",
                HEALTH_TIMEOUT.as_secs()
            )
            .into());
        }
        let api_is_ready = api_ready();
        let worker_is_ready = worker_ready();
        if api_is_ready && worker_is_ready && Instant::now() <= health_deadline {
            ensure_api_running()?;
            ensure_worker_running()?;
            return Ok(());
        }
        thread::sleep(LOOP_INTERVAL.min(health_deadline.saturating_duration_since(Instant::now())));
    }
}

fn http_readyz(port: u16) -> bool {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(250)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
    if stream
        .write_all(b"GET /readyz HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut response = [0_u8; 64];
    let Ok(length) = stream.read(&mut response) else {
        return false;
    };
    response[..length].starts_with(b"HTTP/1.1 200")
        || response[..length].starts_with(b"HTTP/1.0 200")
}

pub(crate) fn available_ports() -> Result<(u16, u16)> {
    let api = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let worker = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    Ok((api.local_addr()?.port(), worker.local_addr()?.port()))
}

fn ensure_initial_ports_available(ports: DevPorts) -> Result<()> {
    for (label, port) in [("API", ports.api), ("Worker", ports.worker)] {
        TcpListener::bind((Ipv4Addr::LOCALHOST, port)).map_err(|error| {
            format!("{label} 开发端口 {port} 已被占用，请停止旧进程或调整环境变量：{error}")
        })?;
    }
    Ok(())
}

fn ensure_running(label: &str, child: &mut Child) -> Result<()> {
    if let Some(status) = child.try_wait()? {
        Err(format!("{label} 进程意外退出：{status}").into())
    } else {
        Ok(())
    }
}

fn stop_services(services: &mut Services) -> Result<()> {
    let api = stop_child(&mut services.api);
    let worker = stop_child(&mut services.worker);
    api?;
    worker
}

fn stop_all(services: &mut Services, vite: &mut Child) -> Result<()> {
    let backend = stop_services(services);
    let frontend = stop_child(vite);
    backend?;
    frontend
}

fn target_debug_dir(root: &Path) -> PathBuf {
    let target = env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"));
    if target.is_absolute() {
        target.join("debug")
    } else {
        root.join(target).join("debug")
    }
}

fn binary_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

fn cleanup_binaries(root: &Path, binaries: &Binaries) -> Result<()> {
    let generation = binaries
        .api
        .parent()
        .ok_or_else(|| format!("API 二进制缺少版本目录：{}", binaries.api.display()))?;
    if binaries.worker.parent() != Some(generation) {
        return Err("API 与 Worker 不属于同一版本目录，拒绝清理".into());
    }
    remove_generation_dir(root, generation)
}

fn remove_generation_dir(root: &Path, generation: &Path) -> Result<()> {
    let allowed_parent = root.join("target/xtask/dev");
    if generation.parent() != Some(allowed_parent.as_path()) || generation.file_name().is_none() {
        return Err(format!("版本目录越过清理白名单：{}", generation.display()).into());
    }
    match fs::remove_dir_all(generation) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("无法清理版本目录 {}：{error}", generation.display()).into()),
    }
}

fn nonce() -> Result<u128> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("系统时间早于 Unix epoch：{error}"))?
        .as_nanos())
}

#[derive(Debug, Clone, Copy)]
struct DevPorts {
    api: u16,
    worker: u16,
}

impl DevPorts {
    fn from_environment() -> Result<Self> {
        let ports = Self {
            api: environment_port("APP_APP_PORT", 8080)?,
            worker: environment_port("APP_JOBS_HEALTH_PORT", 9091)?,
        };
        if ports.api == ports.worker {
            return Err("APP_APP_PORT 与 APP_JOBS_HEALTH_PORT 不能使用同一端口".into());
        }
        Ok(ports)
    }
}

fn environment_port(name: &str, default: u16) -> Result<u16> {
    match env::var(name) {
        Ok(value) => value
            .parse::<u16>()
            .ok()
            .filter(|port| *port > 0)
            .ok_or_else(|| format!("{name} 必须是 1~65535 的端口").into()),
        Err(env::VarError::NotPresent) => Ok(default),
        Err(env::VarError::NotUnicode(_)) => Err(format!("{name} 不是有效 UTF-8").into()),
    }
}

#[derive(Debug, Clone, Copy)]
struct WorkerIds {
    api: u16,
    worker: u16,
    probe_api: u16,
    probe_worker: u16,
}

impl WorkerIds {
    fn from_environment() -> Result<Self> {
        let base = match env::var("SNOWFLAKE_WORKER_ID") {
            Ok(value) => value
                .parse::<u16>()
                .ok()
                .filter(|value| *value <= 1023)
                .ok_or("SNOWFLAKE_WORKER_ID 必须是 0~1023 的整数")?,
            Err(env::VarError::NotPresent) => 1,
            Err(env::VarError::NotUnicode(_)) => {
                return Err("SNOWFLAKE_WORKER_ID 不是有效 UTF-8".into());
            }
        };
        Ok(Self {
            api: base,
            worker: (base + 1) % 1024,
            probe_api: (base + 2) % 1024,
            probe_worker: (base + 3) % 1024,
        })
    }
}

fn spawn_shutdown_listener(shutdown_requested: Arc<AtomicBool>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                eprintln!("无法安装 Ctrl+C 监听器：{error}");
                return;
            }
        };
        if runtime.block_on(tokio::signal::ctrl_c()).is_ok() {
            shutdown_requested.store(true, Ordering::Release);
        }
    })
}
