use super::devex::{
    CacheState, DevexSuite, Distribution, ResourceGateDecisionEvidence, RunSummary,
    SourceFingerprints, comparison_checks, duration_acceptance,
};

#[test]
fn planned_duration_thresholds_accept_boundaries_and_reject_regressions() {
    let cases = [
        (DevexSuite::RustColdBuild, "api", 100.0, 90.0, 90.1),
        (DevexSuite::RustColdBuild, "worker", 100.0, 80.0, 80.1),
        (DevexSuite::RustColdBuild, "migrate", 100.0, 75.0, 75.1),
        (DevexSuite::CargoDevSave, "config-only", 100.0, 30.0, 30.1),
        (DevexSuite::CargoDevSave, "api-only", 100.0, 70.0, 70.1),
        (DevexSuite::CargoDevSave, "worker-only", 100.0, 70.0, 70.1),
        (
            DevexSuite::CargoDevSave,
            "cancellation",
            100.0,
            1_000.0,
            1_000.1,
        ),
        (
            DevexSuite::RustIncremental,
            "application",
            100.0,
            12_000.0,
            12_000.1,
        ),
        (
            DevexSuite::FrontendFast,
            "default",
            100.0,
            12_000.0,
            12_000.1,
        ),
        (DevexSuite::ResourceGate, "auto", 100.0, 60_000.0, 60_000.1),
    ];
    for (suite, variant, baseline, exact, failed) in cases {
        let summary = summary(suite, variant);
        let exact = duration_acceptance(&summary, &distribution(baseline), &distribution(exact));
        let failed = duration_acceptance(&summary, &distribution(baseline), &distribution(failed));
        assert_eq!(exact.len(), 1, "{} / {variant}", suite.as_str());
        assert!(exact[0].passed, "{} / {variant}", suite.as_str());
        assert!(!failed[0].passed, "{} / {variant}", suite.as_str());
    }
}

#[test]
fn retained_p95_reports_signed_change_and_keeps_regression_boundary() {
    let baseline = summary(DevexSuite::FrontendBuild, "default");
    for (value, observed, passed) in [
        (90.0, "-10.0%", true),
        (100.0, "+0.0%", true),
        (110.0, "+10.0%", true),
        (110.1, "+10.1%", false),
    ] {
        let checks = comparison_checks(
            &baseline,
            &baseline,
            &distribution(100.0),
            &distribution(value),
        )
        .unwrap();
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].observed, observed);
        assert_eq!(checks[0].passed, passed);
    }
}

#[test]
fn targeted_decision_evidence_rejects_tampering() {
    let mut evidence = ResourceGateDecisionEvidence {
        format_version: 1,
        recognized: true,
        mode: "targeted".into(),
        fallback: None,
        steps: vec!["resource-drift".into()],
        artifact_sha256: format!("sha256:{}", "0".repeat(64)),
    };
    assert!(evidence.is_targeted());
    evidence.steps = vec![" ".into()];
    assert!(!evidence.is_targeted());
    evidence.steps = vec!["resource-drift".into()];
    evidence.mode = "full".into();
    assert!(!evidence.is_targeted());
    evidence.mode = "targeted".into();
    evidence.artifact_sha256 = "sha256:invalid".into();
    assert!(!evidence.is_targeted());
}

fn distribution(value: f64) -> Distribution {
    Distribution {
        min: value,
        p50: value,
        p95: value,
        p99: value,
        max: value,
        mean: value,
    }
}

pub(super) fn summary(suite: DevexSuite, variant: &str) -> RunSummary {
    RunSummary {
        memory: None,
        runtime: None,
        schema_version: 1,
        run_id: "run".into(),
        suite,
        variant: variant.into(),
        cache_state: if suite == DevexSuite::RustColdBuild {
            CacheState::Cold
        } else {
            CacheState::Warm
        },
        compile_surface_fingerprint: "surface".into(),
        input_fingerprint: "input".into(),
        requested_runs: 1,
        samples: 1,
        passed: 1,
        failed: 0,
        duration_ms: None,
        sccache_version: None,
        sccache: None,
        resource_gate_targeted_decisions: 0,
        pairing: None,
        source_fingerprints: SourceFingerprints::default(),
    }
}
