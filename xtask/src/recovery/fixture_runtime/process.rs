use super::{
    dependency::{Dependencies, FrontendCommand, Python, failure},
    model::*,
};
use crate::{Result, process::ChildGroup, recovery::clone::native::json as strict};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    process::Stdio,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

pub(super) fn command(binding: &Value, arguments: &Value, windows: bool) -> Result<Vec<String>> {
    let args = arguments.as_array().ok_or("前端参数必须为数组")?;
    let mut command = if windows {
        vec![
            text(&binding["tools"]["node"]["path"])?.into(),
            text(&binding["tools"]["corepack_entry"]["path"])?.into(),
            "pnpm".into(),
        ]
    } else {
        vec![
            text(&binding["tools"]["corepack"]["path"])?.into(),
            "pnpm".into(),
        ]
    };
    for arg in args {
        command.push(text(arg)?.into());
    }
    Ok(command)
}
pub(super) fn run(api: &mut Python, invocation: FrontendCommand<'_>) -> Result<Value> {
    let FrontendCommand {
        binding,
        arguments,
        environment,
        log,
        directory: directory_path,
        timeout,
        secrets,
    } = invocation;
    safe(directory_path)?;
    fs::create_dir(directory_path)?;
    let mut input = tempfile::tempfile_in(api.root.join(".local-tests"))?;
    input.write_all(&serde_json::to_vec(&json!({"directory":directory_path,"scope_id":binding["scope_id"],"frontend":binding["frontend"]["path"],"command":command(binding,arguments,cfg!(windows))?,"environment":environment}))?)?;
    input.seek(SeekFrom::Start(0))?;
    let mut launch = api.command();
    launch
        .arg("--launch")
        .stdin(input)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = ChildGroup::new()?.spawn(launch)?;
    let reader = child
        .take_stdout_reader()?
        .ok_or("浏览器受控进程缺少日志管道")?;
    let (tx, rx) = mpsc::channel();
    let capture = thread::spawn(move || {
        let _ = tx.send(capture(reader, MAX_LOG as usize));
    });
    let started = Instant::now();
    let mut code = None;
    let result = (|| {
        loop {
            if let Some(status) = child.try_wait()? {
                code = status.code().filter(|code| *code != 0);
                check(status.success(), "前端命令失败；核对脱敏日志与原始退出状态")?;
                break;
            }
            check(
                started.elapsed() < Duration::from_secs(timeout),
                "前端命令超时，必须回收原进程树",
            )?;
            thread::sleep(Duration::from_millis(20));
        }
        evidence(api, directory_path, text(&binding["scope_id"])?)
    })();
    let cleanup = if result.is_err() {
        api.call(
            "terminate",
            json!({"directory":directory_path,"scope_id":binding["scope_id"]}),
        )
        .map(|_| ())
    } else {
        Ok(())
    };
    drop(child);
    let log_result = finish_log(rx, capture, log, secrets);
    match result {
        Ok(value) => {
            cleanup?;
            log_result?;
            Ok(value)
        }
        Err(_) => Err(failure(
            if cleanup.is_err() {
                "前端命令失败且整树清理未确认；保留现场，禁止重放"
            } else if log_result.is_err() {
                "前端命令失败且日志未完整保存；保留原进程和失败证据"
            } else {
                "前端命令失败；原进程树已回收，核对脱敏日志与退出码"
            },
            code,
        )),
    }
}
type Captured = std::io::Result<(Vec<u8>, bool)>;
fn finish_log(
    rx: mpsc::Receiver<Captured>,
    capture: thread::JoinHandle<()>,
    log: &Path,
    secrets: &[String],
) -> Result<()> {
    let captured = rx
        .recv_timeout(Duration::from_secs(15))
        .map_err(|_| "浏览器日志管道未在整树关闭后结束");
    let log_result: Result<()> = (|| {
        let (mut bytes, overflow) = captured?.map_err(|_| "浏览器日志读取失败")?;
        let mut secrets = secrets.to_vec();
        secrets.sort_by_key(|v| std::cmp::Reverse(v.len()));
        for secret in secrets {
            if !secret.is_empty() {
                bytes = replace(&bytes, secret.as_bytes(), b"[REDACTED]");
            }
        }
        let mut stream = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(log)?;
        stream.write_all(&bytes)?;
        stream.sync_all()?;
        check(
            !overflow,
            "前端日志超过64MiB；保留有界日志，不能认定完整成功",
        )
    })();
    if capture.is_finished() {
        let _ = capture.join();
    }
    log_result
}

pub(super) fn capture(mut reader: impl Read, maximum: usize) -> std::io::Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    let mut overflow = false;
    let mut block = [0; 65536];
    loop {
        let count = reader.read(&mut block)?;
        if count == 0 {
            break;
        }
        if !overflow && bytes.len() + count <= maximum {
            bytes.extend_from_slice(&block[..count]);
        } else {
            overflow = true;
        }
    }
    Ok((bytes, overflow))
}
fn replace(raw: &[u8], needle: &[u8], replacement: &[u8]) -> Vec<u8> {
    let mut result = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        if raw[i..].starts_with(needle) {
            result.extend_from_slice(replacement);
            i += needle.len();
        } else {
            result.push(raw[i]);
            i += 1;
        }
    }
    result
}
pub(super) fn evidence(
    api: &mut impl Dependencies,
    directory: &Path,
    scope: &str,
) -> Result<Value> {
    let (tree, tree_guard) = document(&directory.join("frontend-tree.json"))?;
    let (process, process_guard) = document(&directory.join("frontend.json"))?;
    check(
        process["role"] == "frontend"
            && process["scope_id"] == scope
            && process["identity"] == tree["process"],
        "前端产品身份与原进程树不一致",
    )?;
    let completion = api.call(
        "completion",
        json!({"directory":directory,"scope_id":scope}),
    )?;
    check(completion["tree"] == tree, "前端进程树在证据绑定期间变化")?;
    let operation = text(&tree["operation_id"])?;
    let completion_path = directory.join(format!("frontend-members-{operation}-stopped.json"));
    let (proof, proof_guard) = document(&completion_path)?;
    check(
        completion["proof"] == proof && completion["completion"] == bound(&completion_path)?,
        "前端完整成员关闭证明与原进程树不一致",
    )?;
    unchanged(&tree_guard)?;
    unchanged(&process_guard)?;
    unchanged(&proof_guard)?;
    Ok(
        json!({"directory":directory,"process":process_guard["descriptor"],"tree":tree_guard["descriptor"],"completion":proof_guard["descriptor"]}),
    )
}
pub(super) fn verify(
    api: &mut impl Dependencies,
    value: &Value,
    directory: &Path,
    scope: &str,
) -> Result<()> {
    strict::exact(
        value,
        &["directory", "process", "tree", "completion"],
        "浏览器进程证据",
    )?;
    check(
        evidence(api, directory, scope)? == *value,
        "浏览器进程证据与当前文件不一致",
    )
}
pub(super) fn partial(api: &mut impl Dependencies, directory: &Path, scope: &str) -> Result<Value> {
    if let Ok(value) = evidence(api, directory, scope) {
        return Ok(value);
    }
    let mut value = json!({"directory":directory});
    for (name, label) in [("frontend.json", "process"), ("frontend-tree.json", "tree")] {
        let path = directory.join(name);
        if path.is_file() {
            value[label] = bound(&path)?;
        }
    }
    let tree = directory.join("frontend-tree.json");
    if tree.is_file() {
        let (tree, _) = document(&tree)?;
        if let Some(operation) = tree["operation_id"].as_str().filter(|s| {
            s.len() == 32
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }) {
            for (name, label) in [
                (
                    format!("frontend-tree-{operation}-result.json"),
                    "tree_result",
                ),
                (
                    format!("frontend-members-{operation}-stopped.json"),
                    "completion",
                ),
            ] {
                let path = directory.join(name);
                if path.is_file() {
                    value[label] = bound(&path)?;
                }
            }
        }
    }
    Ok(value)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capture_is_bounded_drains_and_redacts_across_chunks() {
        assert_eq!(capture(&b"abcdefgh"[..], 4).unwrap(), (vec![], true));
        assert_eq!(
            replace(
                b"aPrivateSecretbPrivateSecret",
                b"PrivateSecret",
                b"[REDACTED]"
            ),
            b"a[REDACTED]b[REDACTED]"
        );
    }
    #[test]
    fn windows_command_uses_bound_node_and_literal_arguments() {
        let binding = json!({"tools":{"corepack":{"path":"corepack.cmd"},"node":{"path":"node.exe"},"corepack_entry":{"path":"a b/corepack.js"}}});
        assert_eq!(
            command(&binding, &json!(["check", "a&b", "中文"]), true).unwrap(),
            [
                "node.exe",
                "a b/corepack.js",
                "pnpm",
                "check",
                "a&b",
                "中文"
            ]
        );
    }
}
