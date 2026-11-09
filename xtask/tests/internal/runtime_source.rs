use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use super::{
    cli::{CheckCommand, Command, RecoveryCommand, RuntimeCommand, SourceCommand, parse},
    recovery::source,
    workspace::root_dir,
};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);
const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

struct Fixture {
    directory: PathBuf,
    files: Paths,
}

struct Paths {
    plan: PathBuf,
    target: PathBuf,
    registration: PathBuf,
    build: PathBuf,
    bindings: PathBuf,
    launch: PathBuf,
    receipt: PathBuf,
    owner: PathBuf,
    generation: PathBuf,
    export: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = root_dir().join(".local-tests").join(format!(
            "xtask-runtime-source-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        let files = Paths {
            plan: write(&directory, "reference plan.json"),
            target: write(&directory, "target plan.json"),
            registration: write(&directory, "registration.json"),
            build: write(&directory, "build.json"),
            bindings: write(&directory, "bindings.json"),
            launch: write(&directory, "runtime-launch.json"),
            receipt: write(&directory, "runtime.json"),
            owner: write(&directory, "owner.json"),
            generation: write(&directory, "source-generation.json"),
            export: write(&directory, "source-export.json"),
        };
        Self { directory, files }
    }

    fn runtime_prefix(&self, operation: &str) -> Vec<String> {
        strings(&["check", "recovery", "runtime", operation])
    }

    fn source_arguments(&self) -> Vec<String> {
        vec![
            "--source-backend".into(),
            text(&root_dir()),
            "--source-frontend".into(),
            text(&self.directory),
        ]
    }

    fn control_arguments(&self) -> Vec<String> {
        vec![
            "--runtime-registration".into(),
            text(&self.files.registration),
            "--target-plan".into(),
            text(&self.files.target),
        ]
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.directory.exists() {
            fs::remove_dir_all(&self.directory).unwrap();
        }
    }
}

fn write(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    fs::write(&path, b"{}\n").unwrap();
    path
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

fn text(path: &Path) -> String {
    path.to_str().unwrap().to_owned()
}

fn parsed_recovery(arguments: Vec<String>) -> RecoveryCommand {
    match parse(arguments).unwrap().command {
        Command::Check(CheckCommand::Recovery(command)) => command,
        _ => panic!("命令没有解析为 recovery"),
    }
}

#[test]
fn parses_runtime_build_registration_and_start_into_typed_requests() {
    let fixture = Fixture::new();
    let build_output = fixture.directory.join("new-build.json");
    let mut build = fixture.runtime_prefix("build");
    build.extend(fixture.source_arguments());
    build.extend(strings(&[
        "--expected-head",
        SHA_A,
        "--expected-frontend-head",
        SHA_B,
        "--output",
        build_output.to_str().unwrap(),
        "--write",
    ]));
    let RecoveryCommand::Runtime(RuntimeCommand::Build(_)) = parsed_recovery(build) else {
        panic!("Build没有解析为原生请求")
    };
    assert!(!build_output.exists());

    let mut register = fixture.runtime_prefix("register");
    register.extend(strings(&[
        "--plan",
        fixture.files.plan.to_str().unwrap(),
        "--target-plan",
        fixture.files.target.to_str().unwrap(),
        "--output",
        fixture
            .directory
            .join("new-registration.json")
            .to_str()
            .unwrap(),
        "--write",
    ]));
    let mut without_write = register.clone();
    without_write.pop();
    assert!(parse(without_write).is_err());
    let RecoveryCommand::Runtime(RuntimeCommand::Register(_)) = parsed_recovery(register) else {
        panic!("Register没有解析为原生请求")
    };
    assert!(!fixture.directory.join("new-registration.json").exists());

    let mut start = fixture.runtime_prefix("start");
    start.extend(fixture.control_arguments());
    start.extend(fixture.source_arguments());
    start.extend(strings(&[
        "--build-receipt",
        fixture.files.build.to_str().unwrap(),
        "--bindings",
        fixture.files.bindings.to_str().unwrap(),
        "--timeout",
        "12.5",
        "--write",
    ]));
    let RecoveryCommand::Runtime(RuntimeCommand::Start(start)) = parsed_recovery(start) else {
        panic!("start 没有解析为结构化请求")
    };
    assert_eq!(start.timeout, std::time::Duration::from_millis(12_500));
}

#[test]
fn parses_runtime_control_and_receipt_operations_into_typed_requests() {
    let fixture = Fixture::new();
    let mut status = fixture.runtime_prefix("status");
    status.extend(fixture.control_arguments());
    let mut with_write = status.clone();
    with_write.push("--write".into());
    assert!(parse(with_write).is_err());
    let RecoveryCommand::Runtime(RuntimeCommand::Status(_)) = parsed_recovery(status) else {
        panic!("Status没有解析为原生只读请求")
    };

    let mut stop = fixture.runtime_prefix("stop");
    stop.extend(fixture.control_arguments());
    stop.extend(strings(&["--generation", "2"]));
    assert!(parse(stop.clone()).is_err());
    stop.push("--write".into());
    let RecoveryCommand::Runtime(RuntimeCommand::Stop(_)) = parsed_recovery(stop) else {
        panic!("Stop 没有解析为原生写请求")
    };

    let mut recover = fixture.runtime_prefix("recover");
    recover.extend(fixture.control_arguments());
    recover.extend(strings(&["--generation", "2"]));
    recover.extend(vec![
        "--owner".into(),
        text(&fixture.files.owner),
        "--write".into(),
    ]);
    let RecoveryCommand::Runtime(RuntimeCommand::Recover(_)) = parsed_recovery(recover) else {
        panic!("Recover 没有解析为原生写请求")
    };

    let mut bind = fixture.runtime_prefix("bind");
    bind.extend(fixture.source_arguments());
    bind.extend(vec![
        "--build-receipt".into(),
        text(&fixture.files.build),
        "--launch-receipt".into(),
        text(&fixture.files.launch),
        "--bindings".into(),
        text(&fixture.files.bindings),
        "--output".into(),
        text(&fixture.directory.join("new-runtime.json")),
        "--write".into(),
    ]);
    assert!(matches!(
        parsed_recovery(bind),
        RecoveryCommand::Runtime(RuntimeCommand::Bind(_))
    ));

    let mut verify = fixture.runtime_prefix("verify");
    verify.extend(fixture.source_arguments());
    verify.extend(vec![
        "--bindings".into(),
        text(&fixture.files.bindings),
        "--receipt".into(),
        text(&fixture.files.receipt),
    ]);
    assert!(matches!(
        parsed_recovery(verify),
        RecoveryCommand::Runtime(RuntimeCommand::Verify(_))
    ));
}

#[test]
fn parses_source_operations_and_validates_native_paths() {
    let fixture = Fixture::new();
    let mut verify = strings(&["check", "recovery", "source", "verify"]);
    let source_output = fixture
        .directory
        .join("new-generation/verification/source-runtime.json");
    verify.extend(vec![
        "--source-generation".into(),
        text(&fixture.files.generation),
        "--output".into(),
        text(&source_output),
        "--write".into(),
    ]);
    let RecoveryCommand::Source(command @ SourceCommand::Verify(_)) = parsed_recovery(verify)
    else {
        panic!("source verify 没有解析为结构化请求")
    };
    source::validate_command(&command, &root_dir()).unwrap();

    fs::create_dir_all(source_output.parent().unwrap()).unwrap();
    let mut recover = strings(&["check", "recovery", "source", "verify-recover"]);
    recover.extend(vec![
        "--source-generation".into(),
        text(&fixture.files.generation),
        "--output".into(),
        text(&source_output),
        "--write".into(),
    ]);
    let RecoveryCommand::Source(command @ SourceCommand::VerifyRecover(_)) =
        parsed_recovery(recover)
    else {
        panic!("source verify-recover 没有解析为结构化请求")
    };
    source::validate_command(&command, &root_dir()).unwrap();

    let comparison = comparison_arguments(&fixture);
    assert!(matches!(
        parsed_recovery(comparison),
        RecoveryCommand::Source(SourceCommand::ComparisonCapture(_))
    ));
    let receipt = fixture.files.receipt.to_str().unwrap();
    assert!(matches!(
        parsed_recovery(strings(&[
            "check",
            "recovery",
            "source",
            "comparison-verify",
            "--receipt",
            receipt,
        ])),
        RecoveryCommand::Source(SourceCommand::ComparisonVerify { .. })
    ));
}

fn comparison_arguments(fixture: &Fixture) -> Vec<String> {
    let roots = (0..5)
        .map(|index| {
            let root = fixture.directory.join(format!("source-{index}"));
            fs::create_dir_all(root.join(".local-tests")).unwrap();
            fs::create_dir_all(root.join("dist/.vite")).unwrap();
            root
        })
        .collect::<Vec<_>>();
    let b0_build = write(&roots[1].join(".local-tests"), "build.json");
    let b1_build = write(&roots[3].join(".local-tests"), "build.json");
    let b0_frontend_build = write(&roots[2].join("dist/.vite"), "restore-build.json");
    let b1_frontend_build = write(&roots[4].join("dist/.vite"), "restore-build.json");
    vec![
        "check".into(),
        "recovery".into(),
        "source".into(),
        "comparison-capture".into(),
        "--b0-backend".into(),
        text(&roots[0]),
        "--b0-adapter-backend".into(),
        text(&roots[1]),
        "--b0-frontend".into(),
        text(&roots[2]),
        "--b0-backend-build".into(),
        text(&b0_build),
        "--b0-frontend-build".into(),
        text(&b0_frontend_build),
        "--b1-backend".into(),
        text(&roots[3]),
        "--b1-frontend".into(),
        text(&roots[4]),
        "--b1-backend-build".into(),
        text(&b1_build),
        "--b1-frontend-build".into(),
        text(&b1_frontend_build),
        "--source-export-result".into(),
        text(&fixture.files.export),
        "--output".into(),
        text(&fixture.directory.join("comparison.json")),
        "--write".into(),
    ]
}

#[test]
fn rejects_ambiguous_or_unsafe_runtime_and_source_arguments() {
    let fixture = Fixture::new();
    let mut valid_status = fixture.runtime_prefix("status");
    valid_status.extend(fixture.control_arguments());
    let mut duplicate = valid_status.clone();
    duplicate.extend(vec!["--target-plan".into(), text(&fixture.files.target)]);
    let mut read_write = valid_status.clone();
    read_write.push("--write".into());
    let mut unknown = valid_status.clone();
    unknown.extend(strings(&["--unknown", "value"]));
    for arguments in [duplicate, read_write, unknown] {
        assert!(parse(arguments).is_err());
    }

    let relative_source = strings(&[
        "check",
        "recovery",
        "runtime",
        "verify",
        "--source-backend",
        "relative",
        "--source-frontend",
        fixture.directory.to_str().unwrap(),
        "--bindings",
        fixture.files.bindings.to_str().unwrap(),
        "--receipt",
        fixture.files.receipt.to_str().unwrap(),
    ]);
    assert!(parse(relative_source).is_err());

    let mut missing_write = strings(&["check", "recovery", "source", "verify"]);
    missing_write.extend(vec![
        "--source-generation".into(),
        text(&fixture.files.generation),
        "--output".into(),
        text(&fixture.directory.join("source.json")),
    ]);
    assert!(parse(missing_write).is_err());
    let wrong_source_output = strings(&[
        "check",
        "recovery",
        "source",
        "verify",
        "--source-generation",
        fixture.files.generation.to_str().unwrap(),
        "--output",
        fixture.directory.join("source.json").to_str().unwrap(),
        "--write",
    ]);
    assert!(parse(wrong_source_output).is_err());
    let missing_recovery_output = fixture
        .directory
        .join("missing/verification/source-runtime.json");
    let missing_recovery = strings(&[
        "check",
        "recovery",
        "source",
        "verify-recover",
        "--source-generation",
        fixture.files.generation.to_str().unwrap(),
        "--output",
        missing_recovery_output.to_str().unwrap(),
        "--write",
    ]);
    assert!(parse(missing_recovery).is_err());

    let build_output = fixture.directory.join("relative-frontend-build.json");
    let mut relative_frontend = fixture.runtime_prefix("build");
    relative_frontend.extend(fixture.source_arguments());
    relative_frontend.extend(strings(&[
        "--expected-head",
        SHA_A,
        "--expected-frontend-head",
        SHA_B,
        "--output",
        build_output.to_str().unwrap(),
        "--write",
        "--frontend-dir",
        "relative-frontend",
    ]));
    assert!(parse(relative_frontend).is_err());

    let mut meaningless_frontend = valid_status;
    meaningless_frontend.extend(vec!["--frontend-dir".into(), text(&root_dir())]);
    assert!(parse(meaningless_frontend).is_err());
}

#[test]
fn rejects_invalid_runtime_values_and_source_relationships() {
    let fixture = Fixture::new();
    let mut verify = fixture.runtime_prefix("verify");
    verify.extend(fixture.source_arguments());
    verify.extend(vec![
        "--bindings".into(),
        text(&fixture.files.bindings),
        "--receipt".into(),
        text(&fixture.files.receipt),
    ]);
    let mut adapter_only = verify.clone();
    adapter_only.extend(strings(&[
        "--adapter-contract",
        "legacy-stable-readiness-b0-v1",
    ]));
    let mut product_only = verify.clone();
    product_only.extend(vec!["--product-backend".into(), text(&root_dir())]);
    let mut unknown_contract = verify;
    unknown_contract.extend(vec![
        "--adapter-contract".into(),
        "unknown".into(),
        "--product-backend".into(),
        text(&root_dir()),
    ]);
    for arguments in [adapter_only, product_only, unknown_contract] {
        assert!(parse(arguments).is_err());
    }

    let mut stop = fixture.runtime_prefix("stop");
    stop.extend(fixture.control_arguments());
    stop.extend(strings(&["--generation", "0", "--write"]));
    assert!(parse(stop).is_err());

    let mut build = fixture.runtime_prefix("build");
    build.extend(fixture.source_arguments());
    build.extend(vec![
        "--expected-head".into(),
        SHA_A.to_uppercase(),
        "--expected-frontend-head".into(),
        SHA_B.into(),
        "--output".into(),
        text(&fixture.directory.join("invalid-sha-build.json")),
        "--write".into(),
    ]);
    assert!(parse(build).is_err());

    let mut start = fixture.runtime_prefix("start");
    start.extend(fixture.control_arguments());
    start.extend(fixture.source_arguments());
    start.extend(vec![
        "--build-receipt".into(),
        text(&fixture.files.build),
        "--bindings".into(),
        text(&fixture.files.bindings),
    ]);
    start.extend(strings(&["--timeout", "NaN", "--write"]));
    assert!(parse(start).is_err());

    let mut duplicate_sources = comparison_arguments(&fixture);
    let b0 = option_value(&duplicate_sources, "--b0-backend");
    replace_option(&mut duplicate_sources, "--b1-frontend", b0);
    assert!(parse(duplicate_sources).is_err());

    let mut wrong_receipt = comparison_arguments(&fixture);
    replace_option(
        &mut wrong_receipt,
        "--b0-frontend-build",
        text(&fixture.files.build),
    );
    assert!(parse(wrong_receipt).is_err());
}

#[test]
fn rejects_linked_external_source_before_private_protocol_creation() {
    let fixture = Fixture::new();
    let target = fixture.directory.join("external-source");
    let linked = fixture.directory.join("linked-source");
    fs::create_dir(&target).unwrap();
    create_directory_link(&target, &linked);
    let mut verify = fixture.runtime_prefix("verify");
    verify.extend(fixture.source_arguments());
    verify.extend(vec![
        "--bindings".into(),
        text(&fixture.files.bindings),
        "--receipt".into(),
        text(&fixture.files.receipt),
    ]);
    replace_option(&mut verify, "--source-backend", text(&linked));
    assert!(parse(verify).is_err());
    #[cfg(unix)]
    fs::remove_file(&linked).unwrap();
    #[cfg(windows)]
    fs::remove_dir(&linked).unwrap();
}

#[cfg(unix)]
fn create_directory_link(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[cfg(windows)]
fn create_directory_link(target: &Path, link: &Path) {
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .status()
        .unwrap();
    assert!(status.success());
}

fn option_value(arguments: &[String], name: &str) -> String {
    let index = arguments.iter().position(|value| value == name).unwrap();
    arguments[index + 1].clone()
}

fn replace_option(arguments: &mut [String], name: &str, value: String) {
    let index = arguments
        .iter()
        .position(|current| current == name)
        .unwrap();
    arguments[index + 1] = value;
}

#[test]
fn help_is_rendered_by_the_typed_public_entry_without_filesystem_access() {
    for arguments in [
        strings(&["check", "recovery", "runtime", "--help"]),
        strings(&["check", "recovery", "runtime", "register", "--help"]),
        strings(&["check", "recovery", "source", "--help"]),
        strings(&["check", "recovery", "source", "comparison-verify", "--help"]),
    ] {
        assert!(matches!(
            parsed_recovery(arguments),
            RecoveryCommand::Runtime(RuntimeCommand::Help(_))
                | RecoveryCommand::Source(SourceCommand::Help(_))
        ));
    }
}
