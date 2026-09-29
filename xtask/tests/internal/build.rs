use std::path::Path;

use super::{
    build::{
        BuildArtifact, BuildExecutor, build_plan, cargo_executable_from_messages, render_plan,
    },
    cli::{BuildOptions, BuildProfile},
};

fn options(profile: BuildProfile, real: bool) -> BuildOptions {
    BuildOptions {
        profile,
        real,
        plan: false,
    }
}

#[test]
fn release_plan_is_the_default_build_contract() {
    let plan = build_plan(
        options(BuildProfile::Release, false),
        Path::new("D:/后端 workspace"),
        Path::new("D:/前端 workspace"),
    );

    assert_eq!(plan.profile, BuildProfile::Release);
    assert!(!plan.real);
    assert_eq!(plan.tasks.len(), 3);
    let api = &plan.tasks[0];
    assert_eq!(api.id, "backend-api");
    assert!(api.dependencies.is_empty());
    assert_eq!(api.roles, ["API"]);
    assert_eq!(api.executor, BuildExecutor::Cargo);
    assert_eq!(api.artifact, BuildArtifact::Executable("ryframe"));
    assert!(api.arguments.iter().any(|argument| argument == "--release"));
    assert!(
        api.arguments
            .windows(2)
            .any(|pair| pair == ["--target-dir", "target/build"])
    );
    assert!(
        api.arguments
            .iter()
            .any(|argument| argument == "--no-default-features")
    );
    assert!(
        api.arguments
            .windows(2)
            .any(|pair| pair == ["--features", "bin-api"])
    );
    let worker = &plan.tasks[1];
    assert_eq!(worker.id, "backend-worker");
    assert_eq!(worker.dependencies, ["backend-api"]);
    assert_eq!(worker.roles, ["Worker"]);
    assert_eq!(worker.artifact, BuildArtifact::Executable("ryframe-worker"));
    assert!(
        worker
            .arguments
            .windows(2)
            .any(|pair| pair == ["--features", "bin-worker"])
    );
    assert!(
        !worker
            .arguments
            .iter()
            .any(|argument| argument == "bin-api")
    );
    let frontend = &plan.tasks[2];
    assert_eq!(frontend.dependencies, ["backend-api", "backend-worker"]);
    assert_eq!(frontend.roles, ["前端生产产物"]);
    assert_eq!(frontend.executor, BuildExecutor::Pnpm);
    assert_eq!(frontend.artifact, BuildArtifact::FrontendDirectory);
    assert_eq!(frontend.arguments, ["build"]);
}

#[test]
fn dev_real_plan_and_rendering_share_the_effective_build_spec() {
    let backend_root = Path::new("D:/后端 workspace");
    let frontend_root = Path::new("D:/前端 workspace");
    let plan = build_plan(
        options(BuildProfile::Dev, true),
        backend_root,
        frontend_root,
    );
    let preview_plan = build_plan(
        BuildOptions {
            profile: BuildProfile::Dev,
            real: true,
            plan: true,
        },
        backend_root,
        frontend_root,
    );
    assert_eq!(preview_plan, plan);
    let api = &plan.tasks[0];
    let worker = &plan.tasks[1];
    let frontend = &plan.tasks[2];

    assert!(!api.arguments.iter().any(|argument| argument == "--release"));
    assert_eq!(frontend.arguments, ["build", "--real"]);
    let rendered = render_plan(&plan);
    assert!(rendered.contains("构建计划：profile=dev，real=true"));
    assert!(rendered.contains("D:/后端 workspace"));
    assert!(rendered.contains("D:/前端 workspace"));
    assert!(rendered.contains(&api.arguments.join(" ")));
    assert!(rendered.contains(&worker.arguments.join(" ")));
    assert!(rendered.contains(&frontend.arguments.join(" ")));
    assert!(rendered.contains("host target；dev"));
    assert!(rendered.contains("jobs=继承 Cargo 有效配置"));
    assert!(rendered.contains("Cargo 工作区清单、锁文件与所选包依赖图"));
    assert!(rendered.contains("真实同源配置与构建收据"));
}

#[test]
fn cargo_artifact_parser_requires_the_named_binary_artifact() {
    let messages = concat!(
        "{\"reason\":\"compiler-artifact\",\"target\":{\"name\":\"dependency\",\"kind\":[\"lib\"]},\"executable\":null}\n",
        "{\"reason\":\"compiler-artifact\",\"target\":{\"name\":\"ryframe\",\"kind\":[\"bin\"]},\"executable\":\"D:/target/ryframe.exe\"}\n",
    );
    assert_eq!(
        cargo_executable_from_messages("ryframe", messages).unwrap(),
        Path::new("D:/target/ryframe.exe")
    );
    assert!(cargo_executable_from_messages("ryframe-worker", messages).is_err());
}
