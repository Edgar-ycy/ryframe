use super::*;
use std::collections::BTreeSet;

fn binding(label: &str) -> Value {
    json!({"path":label,"bytes":1,"sha256":"a".repeat(64)})
}
fn values() -> (Value, Value, Value, runtime_model::Inputs) {
    let identity = json!({"stage":binding("identity-result"),"identity":{"verified":true},"producer":null,"departments":{"confirmed":true}});
    let capacity =
        json!({"stage":binding("capacity-result"),"plan":binding("quota-plan"),"confirmed":[]});
    let seed = json!({"post_copy":binding("post-copy"),"post_verify":binding("post-verify")});
    let post = json!({"copy_stage_receipt":binding("copy-stage"),"copy_result":binding("copy-result"),"ledger_head":binding("ledger-head"),
        "api_environment":binding("api-environment"),"private_fixture":"must-not-enter-public"});
    let evidence = json!({"run_manifest":binding("manifest"),"seed_registration":binding("seed"),"seed_registration_stage":binding("seed-stage"),
        "identity":identity,"capacity":capacity,"seed_handoff":binding("handoff"),"seed_close":binding("close"),"lineage":{"identities":[]},"storage":{"bound":true}});
    let inputs = runtime_model::Inputs {
        runtime: PathBuf::from("runtime-fixture"),
        handoff: json!({"identity":identity,"capacity":capacity}),
        environment: json!({"APP_ENV":"test"}),
    };
    (evidence, seed, post, inputs)
}

#[test]
fn public_anchor_set_and_private_outputs_match_python_projection() {
    let (evidence, seed, post, inputs) = values();
    let expected_runtime = inputs.runtime.clone();
    let expected_handoff = inputs.handoff.clone();
    let result = assemble(
        Path::new("unused-test-backend"),
        evidence.clone(),
        &seed,
        post.clone(),
        inputs,
    )
    .unwrap();
    let actual = result
        .anchors
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        actual,
        BTreeSet::from([
            "run_manifest",
            "source_to_seed",
            "post_copy",
            "post_verify",
            "seed_registration",
            "seed_registration_stage",
            "capacity",
            "departments",
            "identity",
            "seed_handoff",
            "seed_close",
            "lineage",
            "storage",
            "source_environment"
        ])
    );
    assert_eq!(
        result.anchors["source_to_seed"],
        json!({"copy_stage_receipt":post["copy_stage_receipt"],"copy_result":post["copy_result"],"ledger_head":post["ledger_head"]})
    );
    for key in [
        "run_manifest",
        "seed_registration",
        "seed_registration_stage",
        "capacity",
        "identity",
        "seed_handoff",
        "seed_close",
        "lineage",
        "storage",
    ] {
        assert_eq!(result.anchors[key], evidence[key]);
    }
    assert_eq!(
        result.anchors["departments"],
        evidence["identity"]["departments"]
    );
    assert_eq!(result.anchors["post_copy"], seed["post_copy"]);
    assert_eq!(result.anchors["post_verify"], seed["post_verify"]);
    assert_eq!(
        result.anchors["source_environment"],
        post["api_environment"]
    );
    assert!(!result.anchors.to_string().contains("must-not-enter-public"));
    assert_eq!(result.post, post);
    assert_eq!(result.runtime, expected_runtime);
    assert_eq!(result.handoff, expected_handoff);
    assert_eq!(result.environment, json!({"APP_ENV":"test"}));
}

#[test]
fn assembled_source_keeps_only_the_original_bound_temporary_environment() {
    let fixture = tempfile::tempdir_in(crate::workspace::root_dir()).unwrap();
    let backend = fixture.path();
    let temporary = backend.join(".local-tests/private");
    std::fs::create_dir_all(&temporary).unwrap();
    let (evidence, seed, post, mut inputs) = values();
    let environment = json!({"APP_ENV":"test","TEMP":temporary,"TMP":temporary});
    inputs.environment = environment.clone();
    let result = assemble(backend, evidence, &seed, post.clone(), inputs).unwrap();
    assert_eq!(result.environment, environment);
    assert_eq!(
        result.anchors["source_environment"],
        post["api_environment"]
    );
    let private_path = serde_json::to_string(&temporary).unwrap();
    assert!(!result.anchors.to_string().contains(&private_path));

    for invalid in [
        json!({"APP_ENV":"test","TEMP":temporary}),
        json!({"APP_ENV":"test","temp":temporary,"tmp":temporary}),
        json!({"APP_ENV":"test","TEMP":temporary,"TMP":temporary.join("other")}),
        json!({"APP_ENV":"test","TEMP":backend,"TMP":backend}),
        json!({"APP_ENV":"dev","TEMP":temporary,"TMP":temporary}),
    ] {
        let (evidence, seed, post, mut inputs) = values();
        inputs.environment = invalid;
        assert!(assemble(backend, evidence, &seed, post, inputs).is_err());
    }
}

#[test]
fn identity_capacity_handoff_and_private_environment_cannot_drift() {
    let (evidence, seed, post, inputs) = values();
    assert!(
        require_handoff(
            &evidence["identity"],
            &evidence["capacity"],
            &inputs.handoff
        )
        .is_ok()
    );
    for key in ["identity", "capacity"] {
        let mut wrong = inputs.handoff.clone();
        wrong[key] = json!({"other_generation":true});
        assert!(require_handoff(&evidence["identity"], &evidence["capacity"], &wrong).is_err());
    }
    let mut inputs = inputs;
    inputs.environment["APP_PASSWORD"] = json!("private-fixture-secret");
    let error = assemble(
        Path::new("unused-test-backend"),
        evidence,
        &seed,
        post,
        inputs,
    )
    .err()
    .unwrap()
    .to_string();
    assert!(!error.contains("private-fixture-secret"));
}

#[test]
fn explicit_quiet_callback_consumes_each_identity_in_order_and_stops_on_error() {
    let lineage = json!({"identities":[{"identity":{"pid":42,"started":"1","executable":"one"}},
        {"identity":{"pid":43,"started":"2","executable":"two"}},{"identity":{"pid":44,"started":"3","executable":"three"}}]});
    let mut observed = Vec::new();
    require_quiet(&lineage, &mut |identity| {
        observed.push(identity.clone());
        Ok(())
    })
    .unwrap();
    assert_eq!(
        observed,
        lineage["identities"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["identity"].clone())
            .collect::<Vec<_>>()
    );
    observed.clear();
    let result = require_quiet(&lineage, &mut |identity| {
        observed.push(identity.clone());
        if observed.len() == 2 {
            Err("still running".into())
        } else {
            Ok(())
        }
    });
    assert!(result.is_err());
    assert_eq!(observed.len(), 2);
    assert!(
        require_quiet(&json!({"identities":[{}]}), &mut |_| panic!(
            "missing identity must not reach observer"
        ))
        .is_err()
    );
}

#[test]
fn current_attempt_must_be_the_unique_running_source_registration() {
    let saved = json!({"attempts":[{"number":1,"stage":"seed-runtime","mode":"register","status":"passed"},
        {"number":2,"stage":"seed-runtime","mode":"source-register","status":"running"}]});
    assert!(require_current(&saved, 2).is_ok());
    assert!(require_current(&saved, 0).is_err());
    assert!(require_current(&saved, 3).is_err());
    for case in ["duplicate", "mode", "status", "second-running"] {
        let mut invalid = saved.clone();
        match case {
            "duplicate" => {
                let row = invalid["attempts"][1].clone();
                invalid["attempts"].as_array_mut().unwrap().push(row);
            }
            "mode" => invalid["attempts"][1]["mode"] = json!("source-rebind"),
            "status" => invalid["attempts"][1]["status"] = json!("passed"),
            _ => invalid["attempts"][0]["status"] = json!("running"),
        }
        assert!(require_current(&invalid, 2).is_err(), "{case}");
    }
}
