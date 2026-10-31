use super::*;

fn fixture() -> (Value, Value, std::path::PathBuf) {
    let runtime = std::path::absolute("registry-runtime").unwrap();
    let identities = (0..5)
        .map(|n| {
            json!({"identity":{"pid":42+n,"started":format!("{}",100+n),
        "executable":runtime.join("historical-executable"),"additional":{"preserved":[n,true]}}})
        })
        .collect::<Vec<_>>();
    let lineage = json!({"runtimes":[{"runtime_directory":runtime,"role_receipts":[
        {"role":"api","identity":identities[1]["identity"]},
        {"role":"worker","identity":identities[4]["identity"]}]}],"identities":identities});
    let binding = json!({"path":runtime.join("runtime.json"),"bytes":123,"sha256":"a".repeat(64)});
    (lineage, binding, runtime)
}

#[test]
fn latest_roles_keep_names_and_all_history_keeps_order_and_complete_identity() {
    let (lineage, binding, runtime) = fixture();
    let result = build(&lineage, &runtime, &binding, "seed-scope").unwrap();
    assert_eq!(result["format_version"], 1);
    assert_eq!(result["scope_id"], "seed-scope");
    assert_eq!(result["runtime_sha256"], "a".repeat(64));
    let processes = result["processes"].as_array().unwrap();
    let names = processes
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            "history-0001",
            "api",
            "history-0002",
            "history-0003",
            "worker"
        ]
    );
    for (process, item) in processes
        .iter()
        .zip(lineage["identities"].as_array().unwrap())
    {
        assert_eq!(process["identity"], item["identity"]);
    }
}

#[test]
fn duplicate_insufficient_missing_roles_and_unrepresented_role_identity_are_rejected() {
    for case in [
        "duplicate",
        "insufficient",
        "missing-role",
        "unknown-role",
        "duplicate-role",
        "same-role-key",
        "missing-identity",
        "different-executable",
        "duplicate-runtime",
        "missing-runtime",
    ] {
        let (mut lineage, binding, runtime) = fixture();
        match case {
            "duplicate" => lineage["identities"][2] = lineage["identities"][0].clone(),
            "insufficient" => {
                let identities = lineage["identities"].as_array_mut().unwrap();
                let worker = identities[4].clone();
                let api = identities[1].clone();
                *identities = vec![api, worker];
            }
            "missing-role" => {
                lineage["runtimes"][0]["role_receipts"]
                    .as_array_mut()
                    .unwrap()
                    .pop();
            }
            "unknown-role" => lineage["runtimes"][0]["role_receipts"][1]["role"] = json!("other"),
            "duplicate-role" => lineage["runtimes"][0]["role_receipts"][1]["role"] = json!("api"),
            "same-role-key" => {
                lineage["runtimes"][0]["role_receipts"][1]["identity"] =
                    lineage["identities"][1]["identity"].clone()
            }
            "missing-identity" => {
                lineage["runtimes"][0]["role_receipts"][1]["identity"]["pid"] = json!(999)
            }
            "different-executable" => {
                lineage["runtimes"][0]["role_receipts"][1]["identity"]["executable"] =
                    json!("different")
            }
            "duplicate-runtime" => {
                let item = lineage["runtimes"][0].clone();
                lineage["runtimes"].as_array_mut().unwrap().push(item);
            }
            _ => lineage["runtimes"] = json!([]),
        }
        assert!(
            build(&lineage, &runtime, &binding, "seed-scope").is_err(),
            "{case}"
        );
    }
}

#[test]
fn malformed_json_types_and_runtime_binding_digest_are_rejected() {
    for case in [
        "runtimes",
        "identities",
        "pid",
        "started",
        "executable",
        "binding-bytes",
        "binding-sha",
        "binding-path",
    ] {
        let (mut lineage, mut binding, runtime) = fixture();
        match case {
            "runtimes" => lineage["runtimes"] = json!({}),
            "identities" => lineage["identities"] = json!({}),
            "pid" => lineage["identities"][0]["identity"]["pid"] = json!(true),
            "started" => lineage["identities"][0]["identity"]["started"] = json!(100),
            "executable" => lineage["identities"][0]["identity"]["executable"] = Value::Null,
            "binding-bytes" => binding["bytes"] = json!(-1),
            "binding-sha" => binding["sha256"] = json!("A".repeat(64)),
            _ => binding["path"] = json!(false),
        }
        assert!(
            build(&lineage, &runtime, &binding, "seed-scope").is_err(),
            "{case}"
        );
    }
}
