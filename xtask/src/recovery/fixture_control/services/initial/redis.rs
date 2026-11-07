//! Redis 首代复用现有原生握手、身份、期限和失败关闭，不重新实现监督器。
use super::*;
use crate::recovery::clone::native::{
    cache::{
        self,
        model::{quoted, render_configuration},
    },
    target_verify::storage::redis_layout,
};
use crate::recovery::fixture_control::services::lifecycle::{Runtime, readonly::NativeRuntime};
use std::io::Write;

const PREFIX: &[(&str, &str)] = &[("storage-target", "initial")];

pub(super) fn run(inputs: &Inputs) -> Result<Value> {
    prefix(inputs, PREFIX)?;
    let output = inputs.run.join("redis");
    check(
        !output.try_exists()?,
        "Redis 首代输出已经存在，禁止覆盖或重放",
    )?;
    let _guard = ProcessGuard::acquire(&inputs.run)?;
    let lock = RunLock::claim(&inputs.run)?;
    let result = execute(inputs, &lock, &output);
    let released = lock.release();
    let result = result?;
    released?;
    Ok(result)
}

fn execute(inputs: &Inputs, lock: &RunLock, output: &Path) -> Result<Value> {
    let saved = prefix(inputs, PREFIX)?;
    storage_running(inputs, &saved)?;
    inputs.guard()?;
    let settings = inputs.settings()?;
    let request = request(
        &inputs.backend,
        &inputs.review,
        &inputs.bootstrap,
        &settings,
    )?;
    common::ensure_new_directory(&inputs.backend, output, "Redis 首代输出")?;
    document::write_new(&output.join("request.json"), &request)?;
    let manifest = common::binding(&inputs.run.join("manifest.json"))?;
    let mut sources = inputs.sources.clone();
    sources["manifest"] = manifest.clone();
    sources["request"] = common::binding(&output.join("request.json"))?;
    let (number, attempt) = state::begin(&inputs.run, "cache-target", "initial", &sources)?;
    state::bind_controller(&inputs.run, number, &lock.owner, &attempt)?;
    let controller = common::binding(&state::controller_path(&inputs.run, number))?;
    let request_sha256 = digest::plan_hash(&request)?;
    let result =
        cache::host_start::start(&inputs.backend, &request, &inputs.private, output, || {
            lock.verify()?;
            inputs.guard()?;
            common::bound_file(&inputs.backend, &manifest, "Redis 固定服务清单")?;
            common::bound_file(&inputs.backend, &sources["request"], "Redis 固定请求")?;
            storage_running(inputs, &saved)
        })
        .map(|runtime| {
            json!({"service":"redis","runtime":runtime,
        "request_sha256":request_sha256,"controller":controller,"remote_writes":0})
        });
    let result = finish(inputs, number, result, "NativeCacheStartFailed")?;
    Ok(
        json!({"status":"redis_started","storage":result["runtime"]["redis"],"run":inputs.run,
        "services_started":true,"remote_writes":0,"request_sha256":digest::plan_hash(&request)?}),
    )
}

pub(super) fn storage_running(inputs: &Inputs, state: &Value) -> Result<()> {
    let storage = common::read_bound(
        &inputs.backend,
        &state["attempts"][0]["result"],
        "已完成 RustFS 首代",
    )?;
    let request = common::read_bound(
        &inputs.backend,
        &state["attempts"][0]["sources"]["request"],
        "原 RustFS 请求",
    )?;
    let mut runtime = NativeRuntime::new(&inputs.backend);
    let tree = runtime.call(
        "tree-read",
        json!({"output":inputs.run.join("rustfs"),"scope_id":request["scope_id"]}),
    )?;
    check(
        common::binding(&inputs.run.join("rustfs/rustfs-tree.json"))? == storage["tree"]
            && tree["process"] == storage["identity"],
        "Redis 前驱不是原成功 RustFS 树",
    )?;
    for role in ["supervisor", "monitor", "process"] {
        crate::recovery::fixture_control::services::lifecycle::readonly::process::require(
            &tree[role],
        )?;
    }
    check(
        runtime.call(
            "storage-running",
            json!({"identity":tree["process"],"request":request}),
        )? == true,
        "Redis 首代启动前或等待期间 RustFS 不再就绪",
    )
}

fn request(backend: &Path, review: &Value, bootstrap: &Value, settings: &Value) -> Result<Value> {
    let service = &review["services"]["redis"];
    let directory = PathBuf::from(document::text(&service["directory"], "Redis 目录")?);
    empty_directory(backend, &directory, false)?;
    let endpoint = url::Url::parse(&format!(
        "redis://{}",
        document::text(&service["endpoint"], "Redis 端点")?
    ))?;
    let port = endpoint.port().ok_or("Redis 审阅端点缺少端口")?;
    let password = document::text(&settings["redis"]["password"], "Redis 明文文件认证")?;
    check(
        !password.is_empty() && !password.contains('\0'),
        "Redis 明文文件认证无效",
    )?;
    let tools = &review["tools"];
    let tool = &tools["redis_server"];
    if tool["platform"] == "windows" {
        cache::windows_tool::validate(tool)?;
        check(
            ["wsl", "native_tool", "redis_python"]
                .iter()
                .all(|key| tools.get(*key).is_none()),
            "Windows Redis 审阅不能混用 WSL 或 Linux Redis 工具绑定",
        )?;
        let scope = document::text(&bootstrap["plan"]["scope_id"], "夹具scope")?;
        common::ensure_directory(backend, &directory, "Redis 数据目录")?;
        let configuration = directory.join("redis.conf");
        write_configuration(&configuration, ".", port, password)?;
        return Ok(json!({
            "format_version":3,"scope_id":format!("services-{scope}"),"tool":tool,
            "configuration":common::binding(&configuration)?,"directory":directory_identity(&directory)?,
            "previous_identity":null,"previous_run_id":null,"port":port,"password":password,
            "timeout_seconds":60,
        }));
    }
    let distribution = tools["redis_server"]["distribution"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or("Redis 已审阅工具发行版必须为非空字符串")?;
    check(
        tools["native_tool"]["distribution"].as_str() == Some(distribution),
        "Redis 服务与原生工具发行版不同",
    )?;
    common::ensure_directory(backend, &directory, "Redis 数据目录")?;
    let configuration = directory.join("redis.conf");
    let linux = redis_layout::wsl_path(&directory)?;
    write_configuration(&configuration, &quoted(&linux), port, password)?;
    Ok(
        json!({"format_version":2,"scope_id":format!("services-{}",document::text(&bootstrap["plan"]["scope_id"], "夹具scope")?),
        "wsl":tools["wsl"],"distribution":distribution,"launcher":"/usr/bin/redis-server",
        "executable":tools["redis_server"]["resolved_path"],"sha256":tools["redis_server"]["sha256"],
        "configuration":common::binding(&configuration)?,"directory":directory_identity(&directory)?,
        "previous_identity":null,"previous_boot_id":null,"previous_run_id":null,"port":port,
        "password":password,"timeout_seconds":60,"native_tool":tools["native_tool"]}),
    )
}

fn write_configuration(
    path: &Path,
    directory_argument: &str,
    port: u16,
    password: &str,
) -> Result<()> {
    let text = render_configuration(directory_argument, port, password);
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
#[test]
fn windows_initial_request_binds_relative_configuration_and_rejects_mixed_tools_before_writing() {
    let backend = crate::workspace::root_dir();
    let local = backend.join(".local-tests");
    fs::create_dir_all(&local).unwrap();
    let temporary = tempfile::tempdir_in(local).unwrap();
    let directory = temporary.path().join("redis with spaces");
    let tool = json!({
        "platform":"windows","path":"D:/tools/redis-server.exe","sha256":"a".repeat(64),
        "version":"Redis server v=7.4.11 sha=00000000:0 malloc=libc bits=64 build=5bd2587910691cb8",
        "dlls":{"msys-2.0.dll":"b".repeat(64)},
    });
    let review = json!({
        "services":{"redis":{"directory":directory,"endpoint":"127.0.0.1:29770"}},
        "tools":{"redis_server":tool},
    });
    let bootstrap = json!({"plan":{"scope_id":"windows-redis-risk"}});
    let password = "unit-only-\"\\\n\r\u{7f}-密";
    let settings = json!({"redis":{"password":password}});
    let value = request(&backend, &review, &bootstrap, &settings).unwrap();
    let configuration = directory.join("redis.conf");
    assert_eq!(
        value,
        json!({
            "format_version":3,"scope_id":"services-windows-redis-risk","tool":tool,
            "configuration":common::binding(&configuration).unwrap(),
            "directory":directory_identity(&directory).unwrap(),"previous_identity":null,
            "previous_run_id":null,"port":29770,"password":password,"timeout_seconds":60,
        })
    );
    let contents = fs::read_to_string(&configuration).unwrap();
    assert_eq!(
        contents,
        concat!(
            "bind 127.0.0.1\nport 29770\ndir .\ndatabases 1\nprotected-mode yes\n",
            "save \"\"\nappendonly no\ndaemonize no\n",
            r#"requirepass "unit-only-\"\\\x0a\x0d\x7f-密""#,
            "\n",
        )
    );
    assert!(request(&backend, &review, &bootstrap, &settings).is_err());
    assert_eq!(fs::read_to_string(&configuration).unwrap(), contents);

    for key in ["wsl", "native_tool", "redis_python", "invalid-dll-digest"] {
        let mut changed = review.clone();
        let rejected = temporary.path().join(key);
        changed["services"]["redis"]["directory"] = json!(rejected);
        if key == "invalid-dll-digest" {
            changed["tools"]["redis_server"]["dlls"]["msys-2.0.dll"] = json!("invalid");
        } else {
            changed["tools"][key] = Value::Null;
        }
        let error = request(&backend, &changed, &bootstrap, &settings).unwrap_err();
        assert!(!error.to_string().contains("unit-only-"));
        assert!(!rejected.exists(), "{key}");
        assert!(!rejected.join("redis.conf").exists(), "{key}");
    }
}

#[cfg(all(test, windows))]
#[test]
fn request_uses_reviewed_tool_distribution_and_rejects_invalid_tools_before_directory_creation() {
    let backend = crate::workspace::root_dir();
    let temporary = tempfile::tempdir_in(backend.join(".local-tests")).unwrap();
    let directory = temporary.path().join("accepted-redis");
    let review = json!({"services":{"redis":{"directory":directory,"endpoint":"127.0.0.1:29770"}},
        "tools":{"redis_server":{"distribution":"Ubuntu-24.04","resolved_path":"/usr/bin/redis-server","sha256":"a".repeat(64)},
        "native_tool":{"distribution":"Ubuntu-24.04"},"wsl":{"path":"C:/Windows/System32/wsl.exe","sha256":"b".repeat(64)}}});
    let bootstrap = json!({"plan":{"scope_id":"redis-distribution-risk"}});
    let settings = json!({"redis":{"password":"unit-only-redis-password"}});
    let value = request(&backend, &review, &bootstrap, &settings).unwrap();
    assert_eq!(value["format_version"], 2);
    assert_eq!(value["distribution"], "Ubuntu-24.04");
    assert_eq!(value["port"], 29770);
    let configuration = directory.join("redis.conf");
    assert_eq!(
        value["configuration"],
        common::binding(&configuration).unwrap()
    );
    let contents = fs::read_to_string(&configuration).unwrap();
    assert!(contents.lines().any(|line| line == "port 29770"));
    let expected_directory = format!(
        "dir {}",
        quoted(&redis_layout::wsl_path(&directory).unwrap())
    );
    assert!(contents.lines().any(|line| line == expected_directory));
    assert!(
        contents
            .lines()
            .any(|line| line == "requirepass \"unit-only-redis-password\"")
    );
    for (index, (role, invalid)) in [
        ("native_tool", Some(json!("different-distribution"))),
        ("redis_server", None),
        ("redis_server", Some(json!(""))),
        ("redis_server", Some(json!(7))),
        ("native_tool", None),
        ("native_tool", Some(json!(""))),
        ("native_tool", Some(json!(7))),
    ]
    .into_iter()
    .enumerate()
    {
        let mut changed = review.clone();
        let selected = temporary.path().join(format!("rejected-redis-{index}"));
        changed["services"]["redis"]["directory"] = json!(selected);
        if let Some(value) = invalid {
            changed["tools"][role]["distribution"] = value;
        } else {
            changed["tools"][role]
                .as_object_mut()
                .unwrap()
                .remove("distribution");
        }
        let error = request(&backend, &changed, &bootstrap, &settings).unwrap_err();
        assert!(!error.to_string().contains("unit-only-redis-password"));
        assert!(!selected.exists());
        assert!(!selected.join("redis.conf").exists());
    }
}
