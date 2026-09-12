use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use serde_json::Value;

use super::{
    cli::{
        CheckCommand, Command, MonitoringBindOptions, MonitoringCommand, MonitoringOperation,
        MonitoringPorts, MonitoringTools, RecoveryCommand, parse,
    },
    recovery::monitoring::private_invocation_at,
};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    directory: PathBuf,
    run: PathBuf,
    runtime: PathBuf,
    target: PathBuf,
    token: PathBuf,
    tool: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = super::workspace::root_dir()
            .join(".local-tests")
            .join(format!(
                "xtask-monitoring-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
        let run = directory.join("监控 run");
        fs::create_dir_all(&run).unwrap();
        let create = |name: &str| {
            let path = directory.join(name);
            fs::write(&path, b"{}\n").unwrap();
            path
        };
        let runtime = create("runtime.json");
        let target = create("target.json");
        let tool = create("tool.exe");
        let token = run.join("metrics-token.txt");
        fs::write(&token, b"secret-placeholder\n").unwrap();
        Self {
            directory,
            run,
            runtime,
            target,
            token,
            tool,
        }
    }

    fn binding(&self) -> PathBuf {
        self.run.join("binding.json")
    }

    fn bind_arguments(&self) -> Vec<String> {
        let mut arguments = vec!["bind".into()];
        let binding = self.binding();
        for (name, value) in [
            ("--runtime-receipt", self.runtime.as_path()),
            ("--target-plan", self.target.as_path()),
            ("--output", binding.as_path()),
            ("--metrics-token-file", self.token.as_path()),
            ("--prometheus", self.tool.as_path()),
            ("--promtool", self.tool.as_path()),
            ("--alertmanager", self.tool.as_path()),
            ("--amtool", self.tool.as_path()),
        ] {
            arguments.extend([name.into(), text(value)]);
        }
        arguments.extend([
            "--run-id".into(),
            "restore-monitoring-24".into(),
            "--prometheus-port".into(),
            "29090".into(),
            "--alertmanager-port".into(),
            "29091".into(),
            "--webhook-port".into(),
            "29092".into(),
            "--write".into(),
        ]);
        arguments
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.directory.exists() {
            fs::remove_dir_all(&self.directory).unwrap();
        }
    }
}

fn text(value: &Path) -> String {
    value.to_string_lossy().into_owned()
}

fn parse_monitoring(arguments: Vec<String>) -> Result<MonitoringCommand, String> {
    let mut values = vec!["check".into(), "recovery".into(), "monitoring".into()];
    values.extend(arguments);
    match parse(values).map_err(|error| error.to_string())?.command {
        Command::Check(CheckCommand::Recovery(RecoveryCommand::Monitoring(command))) => Ok(command),
        other => Err(format!("解析到错误命令：{other:?}")),
    }
}

#[test]
fn parses_bind_and_lifecycle_operations_into_typed_requests() {
    let fixture = Fixture::new();
    assert_eq!(
        parse_monitoring(fixture.bind_arguments()).unwrap(),
        MonitoringCommand::Bind(Box::new(MonitoringBindOptions {
            runtime_receipt: fixture.runtime.clone(),
            target_plan: fixture.target.clone(),
            output: fixture.binding(),
            run_id: "restore-monitoring-24".into(),
            metrics_token_file: fixture.token.clone(),
            tools: MonitoringTools {
                prometheus: fixture.tool.clone(),
                promtool: fixture.tool.clone(),
                alertmanager: fixture.tool.clone(),
                amtool: fixture.tool.clone(),
            },
            ports: MonitoringPorts {
                prometheus: 29090,
                alertmanager: 29091,
                webhook: 29092,
            },
        }))
    );
    fs::write(fixture.binding(), b"{}\n").unwrap();
    for operation in [
        MonitoringOperation::Start,
        MonitoringOperation::Observe,
        MonitoringOperation::Close,
        MonitoringOperation::Result,
        MonitoringOperation::Status,
    ] {
        let mut arguments = vec![
            operation.as_str().into(),
            "--binding".into(),
            text(&fixture.binding()),
        ];
        if operation.writes() {
            arguments.push("--write".into());
        }
        assert_eq!(
            parse_monitoring(arguments).unwrap(),
            MonitoringCommand::Lifecycle {
                operation,
                binding: fixture.binding(),
            }
        );
    }
}

#[test]
fn rejects_ambiguous_unsafe_or_incomplete_requests_before_python() {
    let fixture = Fixture::new();
    let other_token = fixture.run.join("other-token.txt");
    fs::write(&other_token, b"other\n").unwrap();
    let base = fixture.bind_arguments();
    let replace = |name: &str, value: &str| {
        let mut arguments = base.clone();
        let index = arguments.iter().position(|item| item == name).unwrap() + 1;
        arguments[index] = value.into();
        arguments
    };
    for arguments in [
        Vec::new(),
        vec!["unknown".into()],
        base[..base.len() - 1].to_vec(),
        [base.clone(), vec!["--run-id".into(), "other".into()]].concat(),
        replace("--run-id", "Invalid"),
        replace("--prometheus-port", "80"),
        replace("--alertmanager-port", "29090"),
        replace("--output", &text(&fixture.run.join("other.json"))),
        replace("--metrics-token-file", &text(&other_token)),
        vec!["status".into(), "--binding".into(), "relative.json".into()],
        vec![
            "status".into(),
            "--binding".into(),
            text(&fixture.target),
            "--write".into(),
        ],
    ] {
        assert!(
            parse_monitoring(arguments.clone()).is_err(),
            "{arguments:?}"
        );
    }
    assert_eq!(
        parse_monitoring(vec!["--help".into()]).unwrap(),
        MonitoringCommand::Help
    );
    assert_eq!(
        parse_monitoring(vec!["start".into(), "--help".into()]).unwrap(),
        MonitoringCommand::LifecycleHelp(MonitoringOperation::Start)
    );
}

#[test]
fn private_protocol_uses_exact_fields_and_rechecks_inputs() {
    let fixture = Fixture::new();
    let command = parse_monitoring(fixture.bind_arguments()).unwrap();
    let invocation = private_invocation_at(&command, &super::workspace::root_dir()).unwrap();
    assert_eq!(invocation.script, "scripts/restore_monitoring_delivery.py");
    let value: Value = serde_json::from_str(&invocation.protocol).unwrap();
    assert_eq!(value["format_version"], 1);
    assert_eq!(value["kind"], "ryframe-xtask-recovery-monitoring");
    assert_eq!(value["operation"], "bind");
    assert_eq!(value["binding"], Value::Null);
    assert_eq!(value["output"], text(&fixture.binding()));
    assert_eq!(value["write"], true);
    fs::remove_file(&fixture.runtime).unwrap();
    assert!(private_invocation_at(&command, &super::workspace::root_dir()).is_err());
}
