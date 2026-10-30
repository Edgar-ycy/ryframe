//! 只调用现有 reference 正式职责；原始输出不进入继承流或公开诊断。
use super::{plan::Authority, *};
use crate::process::{ChildGroup, child_command};
use std::{
    io::Read,
    path::PathBuf,
    process::Stdio,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

struct Output {
    code: Option<i32>,
    success: bool,
    stdout: String,
    stderr: String,
}
pub(super) fn run(
    runtime: &Runtime,
    authority: &Authority,
    file: &Path,
    reports: &Path,
    failure: &Path,
    verify: impl Fn() -> Result<()>,
) -> Result<Value> {
    let mut code = None;
    let mut phase = "input_guard";
    let mut launch_attempted = false;
    let mut stdout = String::new();
    let mut stderr = String::new();
    let result: Result<Value> = (|| {
        verify()?;
        let output = collect(
            runtime,
            file,
            &authority.secrets,
            &mut phase,
            &mut launch_attempted,
            &mut code,
        )?;
        code = output.code;
        stdout = output.stdout;
        stderr = output.stderr;
        phase = "report_publication";
        write_new(
            &reports.join("execution.json"),
            &json!({"tool":bound(&std::env::current_exe()?)?,
            "operation":"reference.dataset","exit_code":code,"stdout":stdout,"stderr":stderr}),
        )?;
        phase = "child_exit";
        check(output.success, "参考数据准备子进程未成功或输出未完整确认")?;
        phase = "post_guard";
        verify()?;
        phase = "result_binding";
        let result_file =
            PathBuf::from(text(&authority.value["work_dir"])?).join("dataset/result.json");
        let guard = snapshot(&result_file, MAX_BYTES)?;
        phase = "result_guard";
        verify()?;
        unchanged(&guard)?;
        Ok(
            json!({"status":"reference_fixture_dataset_prepared","plan":bound(file)?,
            "work_dir":authority.value["work_dir"],"dataset":guard["descriptor"],
            "remote_writes":{"business_data":true,"objects":true}}),
        )
    })();
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            let record = failure_record(
                phase,
                launch_attempted,
                code,
                error.as_ref(),
                &stdout,
                &stderr,
                &authority.secrets,
            );
            Err(failed(failure, reports, &record, code, &authority.secrets))
        }
    }
}
fn failed(
    failure: &Path,
    reports: &Path,
    record: &Value,
    code: Option<i32>,
    secrets: &[String],
) -> Box<dyn std::error::Error> {
    let launch_attempted = record["launch_attempted"] == true;
    let published = publish_failure(failure, reports, record);
    let publication = match published {
        Ok(()) => "已保留失败证据、原退出码及reports，禁止覆盖或重放".to_owned(),
        Err(error) => {
            let detail = failure_record(
                "failure_publication",
                launch_attempted,
                code,
                error.as_ref(),
                "",
                "",
                secrets,
            );
            format!(
                "失败证据发布也未确认；原因={}；系统码={}；保留原退出码及reports现场，禁止覆盖或重放",
                detail["cause"].as_str().unwrap_or("unknown"),
                detail["os_code"]
            )
        }
    };
    let message = format!(
        "参考数据准备未确认；阶段={}；原因={}；系统码={}；{}；{}",
        record["stage"].as_str().unwrap_or("unknown"),
        record["cause"].as_str().unwrap_or("unknown"),
        record["os_code"],
        if launch_attempted {
            "子进程已尝试发起，写入结果可能未知"
        } else {
            "子进程尚未发起，未发起远端写入"
        },
        publication
    );
    super::super::dependency::failure(&message, code.filter(|code| *code != 0))
}
pub(super) fn publish_failure(file: &Path, reports: &Path, record: &Value) -> Result<()> {
    let mut record = record.clone();
    record["report_directory"] = json!(reports);
    write_new(file, &record)
}
pub(super) fn failure_record(
    stage: &'static str,
    launch_attempted: bool,
    code: Option<i32>,
    error: &(dyn std::error::Error + 'static),
    stdout: &str,
    stderr: &str,
    secrets: &[String],
) -> Value {
    let mut current = Some(error);
    let mut os_code = None;
    while let Some(error) = current {
        os_code = error
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::raw_os_error)
            .or(os_code);
        current = error.source();
    }
    json!({"format_version":1,"kind":"reference-fixture-dataset-failure",
        "status":if launch_attempted {"failed_unknown_writes"}else{"failed_not_started"},
        "stage":stage,"launch_attempted":launch_attempted,"write_outcome":if launch_attempted {"unknown"}else{"not_started"},
        "cause":redact(error.to_string().as_bytes(),secrets),"os_code":os_code,"returncode":code,
        "stdout":redact(stdout.as_bytes(),secrets),"stderr":redact(stderr.as_bytes(),secrets)})
}
fn collect(
    runtime: &Runtime,
    plan: &Path,
    secrets: &[String],
    phase: &mut &'static str,
    launch_attempted: &mut bool,
    code: &mut Option<i32>,
) -> Result<Output> {
    *phase = "child_creation";
    let executable = std::env::current_exe()?;
    let mut command = child_command(&executable);
    command
        .args(["check", "recovery", "dataset", "--plan"])
        .arg(plan)
        .arg("--write")
        .current_dir(&runtime.execution)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    super::super::dependency::clean_environment(&mut command);
    runtime.configure_environment(&mut command)?;
    let started = Instant::now();
    let deadline = started + Duration::from_secs(TIMEOUT_SECONDS);
    let group = ChildGroup::new()?;
    *launch_attempted = true;
    let mut child = group.spawn(command)?;
    *phase = "child_output";
    let stdout = child.take_stdout_reader()?.ok_or("参考数据缺少输出管道")?;
    let stderr = child.take_stderr_reader()?.ok_or("参考数据缺少诊断管道")?;
    let out = reader(stdout);
    let err = reader(stderr);
    *phase = "child_wait";
    let mut status = None;
    while Instant::now() < deadline {
        if let Some(done) = child.try_wait()? {
            status = Some(done);
            *code = done.code();
            break;
        }
        thread::sleep(
            Duration::from_millis(50).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
    // 同一受管子树按现有析构合同回收；超时只留下未知写入，不启动第二次准备。
    drop(child);
    check(
        status.is_some(),
        "参考数据准备达到43200秒期限，结果未知，禁止重放",
    )?;
    *phase = "child_output";
    let out = out
        .recv_timeout(
            Duration::from_secs(15).min(deadline.saturating_duration_since(Instant::now())),
        )
        .map_err(|_| "参考数据stdout未在剩余期限内完整收取")??;
    let err = err
        .recv_timeout(
            Duration::from_secs(15).min(deadline.saturating_duration_since(Instant::now())),
        )
        .map_err(|_| "参考数据stderr未在剩余期限内完整收取")??;
    let complete = !out.1 && !err.1;
    Ok(Output {
        code: status.and_then(|s| s.code()),
        success: status.is_some_and(|s| s.success()) && complete,
        stdout: safe_output(Some(out), secrets),
        stderr: safe_output(Some(err), secrets),
    })
}
fn reader(input: Box<dyn Read + Send>) -> mpsc::Receiver<std::io::Result<(Vec<u8>, bool)>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(super::super::process::capture(input, MAX_LOG as usize));
    });
    receiver
}
pub(super) fn redact(bytes: &[u8], secrets: &[String]) -> String {
    crate::recovery::clone::native::maintenance::redact_with(
        &String::from_utf8_lossy(bytes),
        secrets.iter().cloned(),
    )
}
fn safe_output(output: Option<(Vec<u8>, bool)>, secrets: &[String]) -> String {
    match output {
        Some((bytes, false)) => redact(&bytes, secrets),
        _ => "输出未完整确认；原始输出不公开，保留阶段现场核对且禁止重放".into(),
    }
}
