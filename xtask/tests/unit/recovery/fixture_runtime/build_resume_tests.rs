use super::*;
use std::collections::BTreeMap;

fn paused_prefix() -> Prepared {
    let prepared = Prepared::new();
    let inputs = prepared.inputs();
    fs::create_dir(&prepared.output).unwrap();
    let pair = fixture_control::source_pair::execution::pair(
        &inputs.coordinator,
        &inputs.root,
        &inputs.execution_source,
    )
    .unwrap();
    write_new(&prepared.output.join("source-pair.json"), &pair).unwrap();
    let inventory = source::capture(&inputs.root).unwrap();
    let context = context(&inputs, &mut |program, args| {
        prepared.command(program, args)
    })
    .unwrap();
    let binaries = full_stack::build::run(&inputs.root, &prepared.output, &mut |program, args| {
        prepared.command(program, args)
    })
    .unwrap();
    let backend = backend_build(&binaries, &inventory, &context).unwrap();
    write_new(&prepared.output.join("backend-build.json"), &backend).unwrap();
    configuration::copy(&inputs.root, &prepared.output, &inputs.configuration).unwrap();
    prepared
}

fn retained_files(directory: &Path) -> BTreeMap<PathBuf, String> {
    let mut result = BTreeMap::new();
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            result.extend(retained_files(&path));
        } else {
            result.insert(path.clone(), digest::file(&path).unwrap().sha256);
        }
    }
    result
}

#[test]
fn paused_complete_build_resumes_registration_without_compiling_or_rewriting_accepted_prefix() {
    let prepared = paused_prefix();
    let inputs = prepared.inputs();
    let before = retained_files(&prepared.output);
    let sidecar = prepared
        .output
        .with_file_name("runtime-build-configuration");
    let configuration = retained_files(&sidecar);
    let value = resume::run(&inputs, &prepared.output, &mut |program, args| {
        assert!(matches!(
            (program, args.first().map(String::as_str)),
            ("cargo", Some("-V")) | ("rustc", Some("-vV"))
        ));
        prepared.command(program, args)
    })
    .unwrap();
    assert_eq!(value["status"], "reference_fixture_runtime_built");
    assert_eq!(value["resumed"], true);
    assert_eq!(value["remote_writes"], 0);
    for (path, hash) in &before {
        assert_eq!(&digest::file(path).unwrap().sha256, hash);
    }
    assert_eq!(retained_files(&sidecar), configuration);
    assert!(prepared.output.join("runtime.json").is_file());
    assert!(prepared.output.join("runtime-evidence.json").is_file());
    super::super::super::runtime::Runtime::load(
        &inputs.coordinator,
        &inputs.bootstrap,
        &prepared.output,
    )
    .unwrap()
    .verify()
    .unwrap();
    let registered = retained_files(&prepared.output);
    assert!(
        resume::run(&inputs, &prepared.output, &mut |_, _| panic!(
            "已登记前缀不能重放"
        ))
        .is_err()
    );
    assert_eq!(retained_files(&prepared.output), registered);
}

#[test]
fn resume_rejects_artifact_configuration_source_publication_and_context_drift_without_publication()
{
    for drift in [
        "artifact",
        "configuration",
        "source",
        "publication",
        "frontend",
        "context",
        "jobs",
        "target",
    ] {
        let prepared = paused_prefix();
        let inputs = prepared.inputs();
        match drift {
            "artifact" => {
                let binaries = document(&prepared.output.join("binaries.json")).unwrap().0;
                fs::write(path(&binaries["ryframe-reset"]).unwrap(), b"changed reset").unwrap();
            }
            "configuration" => fs::write(
                prepared
                    .output
                    .with_file_name("runtime-build-configuration")
                    .join("worker-config/app.toml"),
                b"changed configuration",
            )
            .unwrap(),
            "source" => fs::write(inputs.root.join("README.md"), b"changed source").unwrap(),
            "publication" => {
                fs::write(prepared.fixture.output.join("execution-source.json"), b"{}").unwrap()
            }
            "frontend" => fs::write(
                prepared.fixture.frontend.join("openapi/openapi.json"),
                b"{}",
            )
            .unwrap(),
            "jobs" => fs::write(
                inputs.root.join(".local-tests/tools.toml"),
                "[verification]\nbuild_jobs = 1\n",
            )
            .unwrap(),
            "target" => fs::write(
                inputs.root.join(".local-tests/tools.toml"),
                format!(
                    "[generator]\ntarget_dir = {}\n",
                    json!(inputs.root.join(".local-tests/other-target"))
                ),
            )
            .unwrap(),
            "context" => {}
            _ => unreachable!(),
        }
        let before = retained_files(&prepared.output);
        let result = resume::run(&inputs, &prepared.output, &mut |program, args| {
            if drift == "context" && program == "cargo" && args == ["-V"] {
                Ok("cargo changed\n".into())
            } else {
                assert_ne!(args.first().map(String::as_str), Some("build"));
                prepared.command(program, args)
            }
        });
        assert!(result.is_err(), "{drift}");
        assert!(!prepared.output.join("runtime.json").exists(), "{drift}");
        assert!(
            !prepared.output.join("runtime-evidence.json").exists(),
            "{drift}"
        );
        assert_eq!(retained_files(&prepared.output), before, "{drift}");
    }
}

#[test]
fn partial_failed_unknown_or_already_registered_prefixes_are_retained_and_never_replayed() {
    for problem in [
        "missing",
        "artifact-missing",
        "artifact-extra",
        "configuration-extra",
        "failure",
        "runtime",
        "unknown",
    ] {
        let prepared = paused_prefix();
        let inputs = prepared.inputs();
        match problem {
            "missing" => fs::remove_file(prepared.output.join("backend-build.json")).unwrap(),
            "artifact-missing" => {
                let binaries = document(&prepared.output.join("binaries.json")).unwrap().0;
                fs::remove_file(path(&binaries["ryframe-worker"]).unwrap()).unwrap();
            }
            "artifact-extra" => {
                fs::write(prepared.output.join("artifacts/unknown"), b"unknown").unwrap()
            }
            "configuration-extra" => fs::write(
                prepared
                    .output
                    .with_file_name("runtime-build-configuration")
                    .join("config/app.dev.toml"),
                b"unknown",
            )
            .unwrap(),
            "failure" => fs::write(prepared.output.join("failure.json"), b"{}").unwrap(),
            "runtime" => fs::write(prepared.output.join("runtime.json"), b"{}").unwrap(),
            "unknown" => fs::write(prepared.output.join("control.lock"), b"unknown").unwrap(),
            _ => unreachable!(),
        }
        let before = retained_files(&prepared.output);
        assert!(
            resume::run(&inputs, &prepared.output, &mut |_, _| panic!(
                "未知前缀不得调用工具链"
            ))
            .is_err(),
            "{problem}"
        );
        assert_eq!(retained_files(&prepared.output), before, "{problem}");
        assert!(
            !prepared.output.join("runtime-evidence.json").exists(),
            "{problem}"
        );
    }
}

#[test]
fn ordinary_build_and_resume_share_the_same_exclusive_registration_guard() {
    let prepared = paused_prefix();
    let inputs = prepared.inputs();
    let held = registration_guard(&inputs, &prepared.output).unwrap();
    let before = retained_files(&prepared.output);
    assert!(
        resume::run(&inputs, &prepared.output, &mut |_, _| panic!(
            "并发续接不得执行命令"
        ))
        .is_err()
    );
    assert_eq!(retained_files(&prepared.output), before);
    held.check().unwrap();
    let output = prepared.output.with_file_name("new-runtime");
    let other = registration_guard(&inputs, &output).unwrap();
    assert!(build(&inputs, &output, &mut |_, _| panic!("并发构建不得执行命令")).is_err());
    assert!(!output.exists());
    other.check().unwrap();
}
