use super::{
    Checked,
    client::{self, Client, Invocation},
    files,
    model::Plan,
};
use crate::devex::runtime_native::process::Cancellation;
use serde_json::{Value, json};
use std::{
    path::Path,
    process::Stdio,
    time::{Duration, Instant},
};

pub(super) async fn execute(plan: &Plan<'_>, cancel: &Cancellation) -> Checked<Value> {
    execute_with(plan, cancel, |directory| {
        client::invocation(
            plan.binding,
            Client::Container(&plan.input.container),
            directory,
        )
    })
    .await
}

pub(super) async fn execute_with(
    plan: &Plan<'_>,
    cancel: &Cancellation,
    invocation: impl FnOnce(&Path) -> Checked<Invocation>,
) -> Checked<Value> {
    cancel.check().map_err(|_| "request_cancelled")?;
    let config = plan.input.output.join("config");
    let worker = plan.input.output.join("worker-config");
    files::private_directory(&config)?;
    files::private_directory(&worker)?;
    for directory in [&config, &worker] {
        files::write_new(
            &directory.join("app.toml"),
            plan.input.common_text.as_bytes(),
        )?;
    }
    let path = plan.input.output.join("database-targets.json");
    let mut receipt = json!({"format_version":1,"scope_id":plan.scope,
        "container_id":plan.input.container,"host":plan.binding["host"],
        "port":plan.binding["port"],"schemas":plan.schemas,"state":"planned"});
    files::write_new(
        &path,
        &serde_json::to_vec(&receipt).map_err(|_| "receipt_write")?,
    )?;
    let invocation = invocation(&config)?;
    let sql = plan
        .schemas
        .iter()
        .map(|name| {
            format!("CREATE DATABASE `{name}` CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci;\n")
        })
        .collect::<String>();
    // 写前证据先持久化，崩溃或强制取消也不能留下可重放的 planned 状态。
    receipt["state"] = json!("creation-unknown");
    files::replace_receipt(&path, &receipt)?;
    run(
        invocation,
        sql.as_bytes(),
        &config,
        cancel,
        Duration::from_secs(120),
    )
    .await
    .map_err(|_| "creation_unknown")?;
    receipt["state"] = json!("created");
    files::replace_receipt(&path, &receipt)?;
    publish(plan, &config, &worker).map_err(|_| "config_publish")?;
    Ok(plan.public())
}

pub(super) async fn run(
    mut invocation: Invocation,
    sql: &[u8],
    directory: &Path,
    cancel: &Cancellation,
    timeout: Duration,
) -> Checked<()> {
    let mut bytes = invocation.prefix;
    bytes.extend_from_slice(sql);
    let file = client::input_file(directory, &bytes)?;
    invocation
        .command
        .current_dir(directory)
        .stdin(Stdio::from(file.reopen().map_err(|_| "client_input")?))
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cancel.check().map_err(|_| "request_cancelled")?;
    let group = crate::process::ChildGroup::new().map_err(|_| "client_spawn")?;
    let mut child = group
        .spawn(invocation.command)
        .map_err(|_| "client_spawn")?;
    let started = Instant::now();
    loop {
        cancel.check().map_err(|_| "request_cancelled")?;
        if started.elapsed() >= timeout {
            return Err("client_timeout");
        }
        if let Some(status) = child.try_wait().map_err(|_| "client_wait")? {
            return if status.success() {
                Ok(())
            } else {
                Err("client_exit")
            };
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn publish(plan: &Plan<'_>, config: &Path, worker: &Path) -> Checked<()> {
    let mut text = plan.input.environment_text.clone();
    for (index, (key, mode)) in [
        ("shared", "shared"),
        ("dedicated-a", "dedicated"),
        ("dedicated-b", "dedicated"),
    ]
    .iter()
    .enumerate()
    {
        let mut fields = plan.binding.as_object().ok_or("binding_invalid")?.clone();
        // 仅复制连接字段，控制库其他选项不能变成未知的租户字段。
        fields.retain(|k, _| {
            [
                "host",
                "port",
                "username",
                "password",
                "tls_mode",
                "tls_ca",
                "tls_client_cert",
                "tls_client_key",
            ]
            .contains(&k.as_str())
        });
        fields.extend([
            ("key".into(), json!(key)),
            ("display_name".into(), json!(format!("CI {key}"))),
            ("kind".into(), json!("mysql")),
            ("mode".into(), json!(mode)),
            ("database".into(), json!(plan.schemas[index + 1])),
            ("max_connections".into(), json!(8)),
        ]);
        text.push_str("\n\n[[tenant_data.targets]]\n");
        for (key, value) in fields {
            if !value.is_null() {
                text.push_str(&format!("{key} = {value}\n"));
            }
        }
    }
    for (directory, id) in [(config, plan.api_worker_id), (worker, 2)] {
        let rendered = instance_text(&text, id)?;
        // 再解析输出，确保 Unicode、引号及转义没有破坏配置语义。
        let _: toml::Table = toml::from_str(&rendered).map_err(|_| "config_publish")?;
        files::write_new(&directory.join("app.test.toml"), rendered.as_bytes())?;
    }
    Ok(())
}

pub(super) fn instance_text(text: &str, id: u64) -> Checked<String> {
    let boundary = regex::Regex::new(r"(?m)^\s*\[").map_err(|_| "config_publish")?;
    let offset = boundary.find(text).map_or(text.len(), |m| m.start());
    let (head, tail) = text.split_at(offset);
    let key = regex::Regex::new(r#"(?m)^[ \t]*(?:snowflake_worker_id|'snowflake_worker_id'|"snowflake_worker_id")[ \t]*=.*$"#).map_err(|_| "config_publish")?;
    let head = if key.is_match(head) {
        key.replace_all(head, format!("snowflake_worker_id = {id}"))
            .into_owned()
    } else {
        format!("snowflake_worker_id = {id}\n{head}")
    };
    Ok(format!("{head}{tail}"))
}
