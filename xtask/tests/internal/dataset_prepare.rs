use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

use serde_json::Value;

use super::{
    cli::{
        CheckCommand, Command, DatasetPrepareCommand, DatasetPrepareMode, RecoveryCommand,
        RecoverySide, parse,
    },
    recovery::dataset_prepare::private_invocation_at,
};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    directory: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = super::workspace::root_dir()
            .join(".local-tests")
            .join(format!(
                "xtask-dataset-prepare-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&directory).unwrap();
        Self { directory }
    }

    fn file(&self, name: &str) -> PathBuf {
        let path = self.directory.join(name);
        fs::write(&path, b"{}\n").unwrap();
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.directory.exists() {
            fs::remove_dir_all(&self.directory).unwrap();
        }
    }
}

fn command(arguments: Vec<String>) -> Result<DatasetPrepareCommand, String> {
    let mut values = vec![
        "check".to_owned(),
        "recovery".to_owned(),
        "dataset-prepare".to_owned(),
    ];
    values.extend(arguments);
    match parse(values).map_err(|error| error.to_string())?.command {
        Command::Check(CheckCommand::Recovery(RecoveryCommand::DatasetPrepare(command))) => {
            Ok(command)
        }
        other => Err(format!("解析到错误命令：{other:?}")),
    }
}

fn path(value: &std::path::Path) -> String {
    value.to_string_lossy().into_owned()
}

#[test]
fn parses_prepare_and_both_existing_sides_into_typed_requests() {
    let fixture = Fixture::new();
    let plan = fixture.file("计划.json");
    let preflight = fixture.file("预检.json");
    let dataset = fixture.file("数据.json");
    assert_eq!(
        command(vec![
            "--plan".into(),
            path(&plan),
            "--preflight".into(),
            path(&preflight),
            "--write".into(),
        ])
        .unwrap(),
        DatasetPrepareCommand::Run(super::cli::DatasetPrepareOptions {
            plan: plan.clone(),
            mode: DatasetPrepareMode::Prepare { preflight },
        })
    );
    for (side, expected) in [
        (None, RecoverySide::Target),
        (Some("source"), RecoverySide::Source),
    ] {
        let mut arguments = vec![
            "--plan".into(),
            path(&plan),
            "--verify-existing".into(),
            path(&dataset),
            "--write".into(),
        ];
        if let Some(side) = side {
            arguments.extend(["--side".into(), side.into()]);
        }
        assert_eq!(
            command(arguments).unwrap(),
            DatasetPrepareCommand::Run(super::cli::DatasetPrepareOptions {
                plan: plan.clone(),
                mode: DatasetPrepareMode::VerifyExisting {
                    dataset: dataset.clone(),
                    side: expected,
                },
            })
        );
    }
}

#[test]
fn rejects_incomplete_ambiguous_or_unsafe_public_arguments() {
    let fixture = Fixture::new();
    let plan = fixture.file("plan.json");
    let preflight = fixture.file("preflight.json");
    let outside = super::workspace::root_dir().join("Cargo.toml");
    let base = vec![
        "--plan".into(),
        path(&plan),
        "--preflight".into(),
        path(&preflight),
        "--write".into(),
    ];
    let cases = [
        Vec::new(),
        base[..base.len() - 1].to_vec(),
        [base.clone(), vec!["--side".into(), "target".into()]].concat(),
        [
            base.clone(),
            vec!["--verify-existing".into(), path(&preflight)],
        ]
        .concat(),
        [base.clone(), vec!["--plan".into(), path(&plan)]].concat(),
        [base.clone(), vec!["--unknown".into()]].concat(),
        vec![
            "--plan".into(),
            path(&outside),
            "--preflight".into(),
            path(&preflight),
            "--write".into(),
        ],
    ];
    for arguments in cases {
        assert!(command(arguments.clone()).is_err(), "{arguments:?}");
    }
    assert_eq!(
        command(vec!["--help".into()]).unwrap(),
        DatasetPrepareCommand::Help
    );
    assert!(command(vec!["--help".into(), "--write".into()]).is_err());
}

#[test]
fn private_protocol_contains_exact_fields_and_rechecks_paths() {
    let fixture = Fixture::new();
    let plan = fixture.file("计划.json");
    let dataset = fixture.file("数据.json");
    let options = super::cli::DatasetPrepareOptions {
        plan: plan.clone(),
        mode: DatasetPrepareMode::VerifyExisting {
            dataset: dataset.clone(),
            side: RecoverySide::Source,
        },
    };
    let invocation = private_invocation_at(&options, &super::workspace::root_dir()).unwrap();
    assert_eq!(invocation.script, "tools/js/restore_reference_dataset.mjs");
    let value: Value = serde_json::from_str(&invocation.protocol).unwrap();
    assert_eq!(value["format_version"], 1);
    assert_eq!(value["kind"], "ryframe-xtask-recovery-dataset-prepare");
    assert_eq!(value["side"], "source");
    assert_eq!(value["write"], true);
    assert_eq!(value["plan"], path(&plan));
    assert_eq!(value["verify_existing"], path(&dataset));
    assert!(value["preflight"].is_null());
    fs::remove_file(dataset).unwrap();
    assert!(private_invocation_at(&options, &super::workspace::root_dir()).is_err());
}
