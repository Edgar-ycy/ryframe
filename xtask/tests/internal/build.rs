use std::path::Path;

use super::{
    build::{BuildExecutor, build_plan, render_plan},
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
    assert_eq!(plan.tasks.len(), 2);
    let backend = &plan.tasks[0];
    assert_eq!(backend.id, "backend-runtime");
    assert!(backend.dependencies.is_empty());
    assert_eq!(backend.roles, ["API", "Worker"]);
    assert_eq!(backend.executor, BuildExecutor::Cargo);
    assert!(
        backend
            .arguments
            .iter()
            .any(|argument| argument == "--release")
    );
    assert!(
        backend
            .arguments
            .windows(2)
            .any(|pair| pair == ["--target-dir", "target/build"])
    );
    assert!(
        backend
            .arguments
            .iter()
            .any(|argument| argument == "--no-default-features")
    );
    assert!(
        backend
            .arguments
            .windows(2)
            .any(|pair| pair == ["--features", "bin-api,bin-worker"])
    );
    let frontend = &plan.tasks[1];
    assert_eq!(frontend.dependencies, ["backend-runtime"]);
    assert_eq!(frontend.roles, ["前端生产产物"]);
    assert_eq!(frontend.executor, BuildExecutor::Pnpm);
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
    let backend = &plan.tasks[0];
    let frontend = &plan.tasks[1];

    assert!(
        !backend
            .arguments
            .iter()
            .any(|argument| argument == "--release")
    );
    assert_eq!(frontend.arguments, ["build", "--real"]);
    let rendered = render_plan(&plan);
    assert!(rendered.contains("构建计划：profile=dev，real=true"));
    assert!(rendered.contains("D:/后端 workspace"));
    assert!(rendered.contains("D:/前端 workspace"));
    assert!(rendered.contains(&backend.arguments.join(" ")));
    assert!(rendered.contains(&frontend.arguments.join(" ")));
    assert!(rendered.contains("host target；dev"));
    assert!(rendered.contains("jobs=继承 Cargo 有效配置"));
    assert!(rendered.contains("Cargo 工作区清单、锁文件与所选包依赖图"));
    assert!(rendered.contains("真实同源配置与构建收据"));
}
