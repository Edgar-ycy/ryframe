use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use super::{
    check::TaskExecutor,
    ci::{deployment_commands, deployment_required_at, plan_at, plan_for_required, run_at},
    cli::{DeploymentOptions, DeploymentPhase},
};

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

fn options(phase: DeploymentPhase) -> DeploymentOptions {
    DeploymentOptions {
        phase,
        base: String::new(),
        head: String::new(),
        github_output: (phase == DeploymentPhase::Source).then(|| PathBuf::from("output")),
        image: (phase == DeploymentPhase::Image).then(|| "ryframe-ci:test".to_owned()),
        expected_commit: (phase == DeploymentPhase::Image).then(|| SHA.to_owned()),
    }
}

fn executors(plan: &super::check::TaskPlan) -> Vec<TaskExecutor> {
    plan.tasks.iter().map(|task| task.executor).collect()
}

#[test]
fn deployment_phases_use_one_sequential_plan_without_repeating_static_checks() {
    let skipped = plan_for_required(DeploymentPhase::Source, false).unwrap();
    assert_eq!(executors(&skipped), [TaskExecutor::CiDeploymentChanges]);

    let source = plan_for_required(DeploymentPhase::Source, true).unwrap();
    assert_eq!(
        executors(&source),
        [
            TaskExecutor::CiDeploymentChanges,
            TaskExecutor::CiDeploymentStatic,
            TaskExecutor::CiDeploymentCompose,
            TaskExecutor::CiDeploymentNginx,
            TaskExecutor::CiDeploymentPrometheus,
        ]
    );
    for (index, task) in source.tasks.iter().enumerate() {
        let dependencies = if index == 0 {
            Vec::new()
        } else {
            vec![source.tasks[index - 1].id]
        };
        assert_eq!(task.dependencies, dependencies);
    }

    let image = plan_for_required(DeploymentPhase::Image, true).unwrap();
    assert_eq!(
        executors(&image),
        [
            TaskExecutor::CiDeploymentChanges,
            TaskExecutor::CiDeploymentImage,
        ]
    );
    assert!(
        image
            .tasks
            .iter()
            .all(|task| task.executor != TaskExecutor::CiDeploymentStatic)
    );
}

#[test]
fn deployment_commands_preserve_fixed_images_paths_and_exact_check_counts() {
    let root = std::env::current_dir().unwrap().join("后端 root");
    let certificate = root.join("runner temp/certificate");
    let source = options(DeploymentPhase::Source);
    let image = options(DeploymentPhase::Image);

    let static_commands =
        deployment_commands(TaskExecutor::CiDeploymentStatic, &root, &source, None).unwrap();
    assert_eq!(static_commands.len(), 1);
    assert_eq!(static_commands[0].program, "python");
    assert_eq!(
        static_commands[0].arguments,
        ["tools/python/check_deployment_assets.py"]
    );

    let compose =
        deployment_commands(TaskExecutor::CiDeploymentCompose, &root, &source, None).unwrap();
    assert_eq!(compose.len(), 1);
    assert_eq!(compose[0].program, "docker");
    assert_eq!(compose[0].arguments[0], "compose");
    assert!(
        compose[0]
            .arguments
            .iter()
            .any(|value| value.ends_with("/tools/fixtures/deployment/deploy.env"))
    );
    assert!(
        compose[0]
            .arguments
            .iter()
            .any(|value| value.ends_with("/deploy/compose.prod.yml"))
    );

    let nginx = deployment_commands(
        TaskExecutor::CiDeploymentNginx,
        &root,
        &source,
        Some(&certificate),
    )
    .unwrap();
    assert_eq!(nginx.len(), 2);
    assert_eq!(nginx[0].program, "openssl");
    assert_eq!(nginx[1].program, "docker");
    assert!(nginx[1].arguments.iter().any(|value| {
        value == "nginx@sha256:65645c7bb6a0661892a8b03b89d0743208a18dd2f3f17a54ef4b76fb8e2f2a10"
    }));
    assert!(nginx[1].arguments.iter().any(|value| {
        value.ends_with("/deploy/nginx/ryframe.conf:/etc/nginx/conf.d/default.conf:ro")
    }));

    let prometheus =
        deployment_commands(TaskExecutor::CiDeploymentPrometheus, &root, &source, None).unwrap();
    assert_eq!(prometheus.len(), 2);
    assert_eq!(
        prometheus[0].arguments[prometheus[0].arguments.len() - 3],
        "check"
    );
    assert_eq!(
        prometheus[1].arguments[prometheus[1].arguments.len() - 3],
        "test"
    );
    for command in &prometheus {
        assert!(command.arguments.iter().any(|value| {
            value
                == "prom/prometheus@sha256:8672a850efe2f9874702406c8318704edb363587f8c2ca88586b4c8fdb5cea24"
        }));
    }

    let image_commands =
        deployment_commands(TaskExecutor::CiDeploymentImage, &root, &image, None).unwrap();
    assert_eq!(image_commands.len(), 1);
    assert_eq!(
        image_commands[0].arguments,
        [
            "tools/python/check_deployment_assets.py",
            "--image",
            "ryframe-ci:test",
            "--expected-commit",
            SHA,
        ]
    );
}

#[test]
fn deployment_change_detection_is_conservative_and_fails_closed() {
    let repository = TestRepository::new();
    let base = repository.head();
    repository.write("README.md", "documentation only\n");
    repository.commit("docs");
    let docs_head = repository.head();
    assert!(!deployment_required_at(&repository.root, &base, &docs_head).unwrap());
    let github_output = repository.root.with_extension("github-output");
    fs::write(&github_output, "existing=value\n").unwrap();
    let source_options = DeploymentOptions {
        phase: DeploymentPhase::Source,
        base: base.clone(),
        head: docs_head.clone(),
        github_output: Some(github_output.clone()),
        image: None,
        expected_commit: None,
    };
    let source_plan = plan_at(&source_options, &repository.root).unwrap();
    assert_eq!(executors(&source_plan), [TaskExecutor::CiDeploymentChanges]);
    run_at(&source_options, &source_plan, &repository.root).unwrap();
    assert_eq!(
        fs::read_to_string(&github_output).unwrap(),
        "existing=value\nrequired=false\n"
    );
    fs::remove_file(github_output).unwrap();

    let mismatched_image = DeploymentOptions {
        phase: DeploymentPhase::Image,
        base: String::new(),
        head: String::new(),
        github_output: None,
        image: Some("ryframe:test".to_owned()),
        expected_commit: Some(SHA.to_owned()),
    };
    assert!(plan_at(&mismatched_image, &repository.root).is_err());

    repository.write("deploy/new.conf", "deployment\n");
    repository.commit("deploy");
    let deployment_head = repository.head();
    assert!(deployment_required_at(&repository.root, &base, &deployment_head).unwrap());
    assert!(deployment_required_at(&repository.root, "", "").unwrap());
    assert!(deployment_required_at(&repository.root, &"0".repeat(40), "").unwrap());
    assert!(
        deployment_required_at(&repository.root, &"1".repeat(40), &deployment_head).is_err(),
        "语法合法但不可解析的 base 必须失败关闭"
    );
    assert!(
        deployment_required_at(&repository.root, &base, &docs_head).is_err(),
        "有效 head 与实际检出不一致必须失败"
    );

    repository.write("unknown.txt", "untracked\n");
    assert!(deployment_required_at(&repository.root, "", "").is_err());
}

#[test]
fn deployment_change_detection_covers_every_indirect_static_and_image_input() {
    let repository = TestRepository::new();
    for (index, path) in [
        "docs/operations.md",
        ".github/workflows/extended-ci.yml",
        "migrations/src/m20260101.rs",
        "openapi/openapi.json",
    ]
    .into_iter()
    .enumerate()
    {
        let base = repository.head();
        repository.write(path, &format!("input {index}\n"));
        repository.commit(&format!("input-{index}"));
        let head = repository.head();
        assert!(
            deployment_required_at(&repository.root, &base, &head).unwrap(),
            "间接部署输入必须选择部署门禁：{path}"
        );
    }
}

struct TestRepository {
    root: PathBuf,
}

impl TestRepository {
    fn new() -> Self {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("target/ci-security-deployment-tests");
        fs::create_dir_all(&parent).unwrap();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = parent.join(format!("{}-{nonce}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let repository = Self { root };
        repository.git(&["init", "--quiet"]);
        repository.git(&["config", "core.autocrlf", "false"]);
        repository.git(&["config", "user.name", "RyFrame Test"]);
        repository.git(&["config", "user.email", "ryframe@example.invalid"]);
        repository.write("README.md", "initial\n");
        repository.commit("initial");
        repository
    }

    fn write(&self, relative: &str, content: &str) {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    fn commit(&self, message: &str) {
        self.git(&["add", "."]);
        self.git(&["commit", "--quiet", "--message", message]);
    }

    fn head(&self) -> String {
        let output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&self.root)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn git(&self, arguments: &[&str]) {
        let status = Command::new("git")
            .args(arguments)
            .current_dir(&self.root)
            .status()
            .unwrap();
        assert!(status.success(), "git {arguments:?}");
    }
}

impl Drop for TestRepository {
    fn drop(&mut self) {
        let expected_parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("target/ci-security-deployment-tests");
        assert_eq!(self.root.parent(), Some(expected_parent.as_path()));
        fs::remove_dir_all(&self.root).unwrap();
    }
}
