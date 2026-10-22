use super::*;

#[test]
fn suite_definitions_select_the_measured_workload() {
    let api = DevexSuite::RustColdBuild.definition("api").unwrap();
    assert_eq!(api.features, &["bin-api"]);
    assert_eq!(api.steps.len(), 1);
    assert!(
        api.steps[0]
            .args
            .windows(2)
            .any(|pair| pair == ["--bin", "ryframe"])
    );

    let save = DevexSuite::CargoDevSave.definition("config-only").unwrap();
    assert!(save.environment.is_empty());
    let args = save.steps[0]
        .args
        .iter()
        .map(|arg| arg.replace("{variant}", "config-only"))
        .collect::<Vec<_>>();
    assert!(
        args.windows(2)
            .any(|args| args == ["--case", "config-only"])
    );
    assert!(
        args.windows(2)
            .any(|args| args == ["--output", "{target}/cargo-dev-save-result.json"])
    );
    assert!(
        args.windows(2)
            .any(|args| args == ["--workspace-root", "{backend}"])
    );
    let frontend = DevexSuite::FrontendFast.definition("default").unwrap();
    assert_eq!(frontend.steps[0].args, &["pnpm", "check"]);
    let baseline_frontend = DevexSuite::FrontendFast
        .paired_definition(
            "default",
            Some(BaselineContract::LegacyStableReadinessB0V1),
            PairedArm::Baseline,
        )
        .unwrap();
    assert_eq!(baseline_frontend.steps[0].args, &["pnpm", "check:fast"]);
    let candidate_frontend = DevexSuite::FrontendFast
        .paired_definition(
            "default",
            Some(BaselineContract::LegacyStableReadinessB0V1),
            PairedArm::Candidate,
        )
        .unwrap();
    assert_eq!(candidate_frontend.steps[0].args, &["pnpm", "check"]);
    let workload = BaselineContract::LegacyStableReadinessB0V1
        .workload_contract(DevexSuite::FrontendFast, "default")
        .unwrap();
    assert_eq!(workload.baseline_primitives.len(), 9);
    assert_eq!(workload.candidate_primitives.len(), 8);
    assert_eq!(
        workload
            .candidate_primitives
            .iter()
            .find(|primitive| primitive.id == "imports")
            .unwrap()
            .covers,
        ["imports", "api-operation-policy"]
    );

    let incremental = DevexSuite::RustIncremental
        .definition("application")
        .unwrap();
    assert_eq!(
        incremental.steps[0].args,
        &["check", "--locked", "-p", "ryframe-application"]
    );
    assert_eq!(incremental.features, &["application"]);
    assert_eq!(
        DevexSuite::RustIncremental
            .incremental_source("application")
            .unwrap(),
        Some("crates/ryframe-application/src/lib.rs")
    );
    assert!(DevexSuite::RustColdBuild.definition("application").is_err());

    let generator = DevexSuite::ResourceGenerator.definition("post").unwrap();
    assert!(generator.steps[0].args.contains(&"{target}"));
    assert!(
        api.remove_environment
            .contains(&"CMAKE_C_COMPILER_LAUNCHER")
    );
    assert!(
        api.remove_environment
            .contains(&"CMAKE_CXX_COMPILER_LAUNCHER")
    );
}
