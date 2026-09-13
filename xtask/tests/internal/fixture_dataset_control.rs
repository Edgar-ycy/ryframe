use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use serde_json::Value;

use super::{
    cli::{
        CheckCommand, Command, FixtureControlCommand, FixtureDatasetCommand, FixtureDatasetInputs,
        FixtureSide, RecoveryCommand, parse,
    },
    recovery::fixture_control::private_invocation_at,
    workspace::root_dir,
};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    directory: PathBuf,
    input: PathBuf,
    input_two: PathBuf,
    controlled_dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = root_dir().join(".local-tests").join(format!(
            "xtask-fixture-dataset-control-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        let input = directory.join("input one.json");
        let input_two = directory.join("input two.json");
        fs::write(&input, b"{}\n").unwrap();
        fs::write(&input_two, b"{}\n").unwrap();
        let controlled_dir = directory.join("controlled directory");
        fs::create_dir(&controlled_dir).unwrap();
        Self {
            directory,
            input,
            input_two,
            controlled_dir,
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.directory.join(name)
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
    value.to_str().unwrap().to_owned()
}

fn parse_control(arguments: Vec<String>) -> Result<FixtureControlCommand, super::cli::CliError> {
    parse(arguments).map(|cli| match cli.command {
        Command::Check(CheckCommand::Recovery(RecoveryCommand::FixtureControl(command))) => {
            *command
        }
        _ => panic!("公开请求没有解析为 fixture dataset 类型"),
    })
}

fn command(domain: &str, operation: Option<&str>, values: Vec<String>) -> Vec<String> {
    let mut arguments = ["check", "recovery", "fixture", domain]
        .map(str::to_owned)
        .to_vec();
    if let Some(operation) = operation {
        arguments.push(operation.to_owned());
    }
    arguments.extend(values);
    arguments
}

#[test]
fn parses_dataset_plan_and_prepare_into_exact_private_protocols() {
    let fixture = Fixture::new();
    let work_dir = fixture.controlled_dir.join("数据 work");
    let output = fixture.controlled_dir.join("dataset plan.json");
    let plan = parse_control(command(
        "dataset",
        Some("plan"),
        vec![
            "--environment".to_owned(),
            text(&fixture.input),
            "--runtime".to_owned(),
            text(&fixture.controlled_dir),
            "--work-dir".to_owned(),
            text(&work_dir),
            "--output".to_owned(),
            text(&output),
            "--side".to_owned(),
            "base".to_owned(),
            "--write".to_owned(),
        ],
    ))
    .unwrap();
    assert!(matches!(
        plan,
        FixtureControlCommand::Dataset(FixtureDatasetCommand::Plan { .. })
    ));
    let invocation = private_invocation_at(&plan, &root_dir()).unwrap();
    assert_eq!(invocation.script, "scripts/reference_fixture_dataset.py");
    let protocol: Value = serde_json::from_str(&invocation.protocol).unwrap();
    assert_eq!(protocol["domain"], "dataset");
    assert_eq!(protocol["operation"], "plan");
    assert_eq!(protocol["environment"], text(&fixture.input));
    assert_eq!(protocol["runtime"], text(&fixture.controlled_dir));
    assert_eq!(protocol["work_dir"], text(&work_dir));
    assert_eq!(protocol["output"], text(&output));
    assert_eq!(protocol["side"], "base");
    assert_eq!(protocol["write"], true);

    let saved_plan = fixture.controlled_dir.join("saved plan.json");
    fs::write(&saved_plan, b"{}\n").unwrap();
    let prepare = parse_control(command(
        "dataset",
        Some("prepare"),
        vec![
            "--environment".to_owned(),
            text(&fixture.input),
            "--runtime".to_owned(),
            text(&fixture.controlled_dir),
            "--plan".to_owned(),
            text(&saved_plan),
            "--side".to_owned(),
            "candidate".to_owned(),
            "--write".to_owned(),
        ],
    ))
    .unwrap();
    assert!(matches!(
        prepare,
        FixtureControlCommand::Dataset(FixtureDatasetCommand::Prepare { .. })
    ));
    let invocation = private_invocation_at(&prepare, &root_dir()).unwrap();
    let protocol: Value = serde_json::from_str(&invocation.protocol).unwrap();
    assert_eq!(protocol["operation"], "prepare");
    assert_eq!(protocol["plan"], text(&saved_plan));
    assert_eq!(protocol["side"], "candidate");
    assert_eq!(protocol["write"], true);
    assert!(protocol.get("work_dir").is_none());
    assert!(protocol.get("output").is_none());
}

#[test]
fn rejects_ambiguous_dataset_writes_sides_and_path_contracts() {
    let fixture = Fixture::new();
    let base = vec![
        "--environment".to_owned(),
        text(&fixture.input),
        "--runtime".to_owned(),
        text(&fixture.controlled_dir),
        "--work-dir".to_owned(),
        text(&fixture.controlled_dir.join("new dataset")),
        "--output".to_owned(),
        text(&fixture.controlled_dir.join("new plan.json")),
        "--side".to_owned(),
        "base".to_owned(),
        "--write".to_owned(),
    ];
    let mut cases = Vec::new();
    let mut missing_write = base.clone();
    missing_write.pop();
    cases.push(missing_write);
    let mut seed = base.clone();
    seed[9] = "seed".to_owned();
    cases.push(seed);
    let mut duplicate = base.clone();
    duplicate.extend(["--side".to_owned(), "candidate".to_owned()]);
    cases.push(duplicate);
    let mut unknown = base.clone();
    unknown.extend(["--plan".to_owned(), text(&fixture.input_two)]);
    cases.push(unknown);
    let mut relative = base.clone();
    relative[3] = "relative runtime".to_owned();
    cases.push(relative);
    let mut outside = base.clone();
    outside[1] = text(&root_dir().join("Cargo.toml"));
    cases.push(outside);
    let mut existing_work = base.clone();
    existing_work[5] = text(&fixture.controlled_dir);
    cases.push(existing_work);
    let mut existing_output = base;
    existing_output[7] = text(&fixture.input_two);
    cases.push(existing_output);
    let same_work_and_output = vec![
        "--environment".to_owned(),
        text(&fixture.input),
        "--runtime".to_owned(),
        text(&fixture.controlled_dir),
        "--work-dir".to_owned(),
        text(&fixture.controlled_dir.join("shared dataset target")),
        "--output".to_owned(),
        text(&fixture.controlled_dir.join("shared dataset target")),
        "--side".to_owned(),
        "base".to_owned(),
        "--write".to_owned(),
    ];
    cases.push(same_work_and_output);
    let missing_output_parent = vec![
        "--environment".to_owned(),
        text(&fixture.input),
        "--runtime".to_owned(),
        text(&fixture.controlled_dir),
        "--work-dir".to_owned(),
        text(&fixture.controlled_dir.join("new dataset")),
        "--output".to_owned(),
        text(
            &fixture
                .controlled_dir
                .join("missing parent")
                .join("new plan.json"),
        ),
        "--side".to_owned(),
        "base".to_owned(),
        "--write".to_owned(),
    ];
    cases.push(missing_output_parent);
    for values in cases {
        assert!(parse_control(command("dataset", Some("plan"), values)).is_err());
    }

    let missing_plan = vec![
        "--environment".to_owned(),
        text(&fixture.input),
        "--runtime".to_owned(),
        text(&fixture.controlled_dir),
        "--plan".to_owned(),
        text(&fixture.path("missing plan.json")),
        "--side".to_owned(),
        "base".to_owned(),
        "--write".to_owned(),
    ];
    assert!(parse_control(command("dataset", Some("prepare"), missing_plan)).is_err());
    assert!(parse_control(command("dataset", Some("plan"), Vec::new())).is_err());
}

#[test]
fn rejects_dataset_artifacts_outside_the_selected_runtime() {
    let fixture = Fixture::new();
    let other_runtime = fixture.path("other runtime");
    fs::create_dir_all(&other_runtime).unwrap();
    let unrelated_plan = other_runtime.join("unrelated plan.json");
    fs::write(&unrelated_plan, b"{}\n").unwrap();
    let mut plan = vec![
        "--environment".to_owned(),
        text(&fixture.input),
        "--runtime".to_owned(),
        text(&fixture.controlled_dir),
        "--work-dir".to_owned(),
        text(&other_runtime.join("new dataset")),
        "--output".to_owned(),
        text(&fixture.controlled_dir.join("related plan.json")),
        "--side".to_owned(),
        "base".to_owned(),
        "--write".to_owned(),
    ];
    assert!(parse_control(command("dataset", Some("plan"), plan.clone())).is_err());
    plan[5] = text(&fixture.controlled_dir.join("related dataset"));
    plan[7] = text(&other_runtime.join("new plan.json"));
    assert!(parse_control(command("dataset", Some("plan"), plan)).is_err());

    assert!(
        parse_control(command(
            "dataset",
            Some("prepare"),
            vec![
                "--environment".to_owned(),
                text(&fixture.input),
                "--runtime".to_owned(),
                text(&fixture.controlled_dir),
                "--plan".to_owned(),
                text(&unrelated_plan),
                "--side".to_owned(),
                "candidate".to_owned(),
                "--write".to_owned(),
            ],
        ))
        .is_err()
    );
}

#[test]
fn private_dataset_fields_recheck_relationships_and_side() {
    let fixture = Fixture::new();
    let other_runtime = fixture.path("other runtime");
    fs::create_dir_all(&other_runtime).unwrap();
    let unrelated_plan = other_runtime.join("plan.json");
    fs::write(&unrelated_plan, b"{}\n").unwrap();
    let inputs = FixtureDatasetInputs {
        environment: fixture.input.clone(),
        runtime: fixture.controlled_dir.clone(),
        side: FixtureSide::Base,
    };
    let invalid = [
        FixtureDatasetCommand::Plan {
            inputs: inputs.clone(),
            work_dir: other_runtime.join("new dataset"),
            output: fixture.controlled_dir.join("plan.json"),
        },
        FixtureDatasetCommand::Plan {
            inputs: inputs.clone(),
            work_dir: fixture.controlled_dir.join("new dataset"),
            output: other_runtime.join("new plan.json"),
        },
        FixtureDatasetCommand::Plan {
            inputs: inputs.clone(),
            work_dir: fixture.controlled_dir.join("same target"),
            output: fixture.controlled_dir.join("same target"),
        },
        FixtureDatasetCommand::Prepare {
            inputs: inputs.clone(),
            plan: unrelated_plan,
        },
        FixtureDatasetCommand::Plan {
            inputs: FixtureDatasetInputs {
                side: FixtureSide::Seed,
                ..inputs
            },
            work_dir: fixture.controlled_dir.join("new dataset"),
            output: fixture.controlled_dir.join("new plan.json"),
        },
    ];
    for command in invalid {
        assert!(
            private_invocation_at(&FixtureControlCommand::Dataset(command), &root_dir()).is_err()
        );
    }
}
