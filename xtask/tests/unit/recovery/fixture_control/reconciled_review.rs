use super::*;

#[test]
fn successor_review_moves_execution_paths_without_claiming_service_control() {
    let workspace = crate::workspace::root_dir().join(".local-tests");
    std::fs::create_dir_all(&workspace).unwrap();
    let temporary = tempfile::tempdir_in(&workspace).unwrap();
    let execution = temporary.path().join("new");
    let parent = execution.join(".local-tests/reference-fixture");
    std::fs::create_dir_all(&parent).unwrap();
    let original_scope = json!({"backend_dir":"old","runtime_dir":"old-runtime",
        "identity_ledger":"old-identities","api_url":"api","worker_ready_url":"worker",
        "frontend_url":"frontend","scope_id":"scope","objects":{"endpoint":"storage"},
        "redis":{"namespace":"fixed"},"databases":[{"key":"shared-control","database":"fixed",
            "host":"fixed-server","connection_file":"old-mysql"}]});
    let original_review = json!({"services":{"rustfs":{"process_receipt":"old-process"}},
        "scopes":{"seed":original_scope,"base":original_scope,"candidate":original_scope},
        "execution_source":{"sha256":"old"}});
    let origin = Origin {
        review: original_review.clone(),
        review_binding: json!({"sha256":"old-review"}),
        marker: json!({"storage_run":{"path":temporary.path().join("old-service")}}),
        service: SeedService {
            run: temporary.path().join("old-service"),
            review: temporary.path().join("old-review.json"),
            request: Value::Null,
        },
    };
    let publication = json!({"published":{"backend":{"path":execution}},
        "original":{"source_pair":{"sources":{"backend":{"head":"backend-head"},
            "frontend":{"head":"frontend-head"}}}}});
    let proposal = json!({"execution_source":{"sha256":"new"},"future_root":parent.join("new-run"),
        "created_at":"2026-11-08T02:00:00Z"});
    let review = derive(
        &crate::workspace::root_dir(),
        &origin,
        &proposal,
        &publication,
    )
    .unwrap();
    assert_eq!(review["services"], original_review["services"]);
    assert_eq!(review["scopes"]["seed"]["backend_dir"], json!(execution));
    for side in SIDES {
        for name in [
            "scope_id",
            "api_url",
            "worker_ready_url",
            "frontend_url",
            "objects",
            "redis",
        ] {
            assert_eq!(review["scopes"][*side][name], original_scope[name]);
        }
        assert_eq!(
            review["scopes"][*side]["databases"][0]["host"],
            "fixed-server"
        );
    }
    assert_eq!(service_run(&review).unwrap(), Some(origin.service.run));
    assert!(
        query::dispatch(
            &crate::workspace::root_dir(),
            "service-run",
            &json!({"review":review}),
        )
        .is_err()
    );
}

#[test]
fn seed_request_keeps_resources_and_exact_database_credentials_across_execution_roots() {
    let databases = ["shared-control", "shared", "dedicated-a", "dedicated-b"]
        .map(|key| json!({"key":key,"database":format!("fixture_{key}"),
            "server_uuid":"fixed-physical-server","defaults_file":"old/mysql.cnf","defaults_sha256":"fixed-bytes"}));
    let original = json!({"side":"seed","id":"old","maintenance_build":{"sha256":"old"},
        "target":{"scope_id":"fixed","s3":{"endpoint":"old-storage","access_key":"fixture"},"databases":databases},
        "storage":{"redis":{"run_id":"original"},"rustfs":{"identity":{"pid":123}}},
        "reset":{"sentinel_key":"fixed-key","credential_version":"fixed-version"}});
    let mut successor = original.clone();
    successor["id"] = json!("new");
    successor["maintenance_build"]["sha256"] = json!("new");
    for database in successor["target"]["databases"].as_array_mut().unwrap() {
        database["defaults_file"] = json!("new/mysql.cnf");
    }
    verify_request_projection(&original, &successor).unwrap();
    for (pointer, value) in [
        ("/side", json!("base")),
        ("/id", json!("old")),
        ("/maintenance_build/sha256", json!("old")),
        ("/target/scope_id", json!("different")),
        ("/target/s3/endpoint", json!("different")),
        ("/storage/redis/run_id", json!("different")),
        ("/storage/rustfs/identity/pid", json!(456)),
        ("/target/databases/0/database", json!("different")),
        ("/target/databases/0/server_uuid", json!("different")),
        ("/target/databases/0/defaults_sha256", json!("different")),
        ("/target/databases/0/key", json!("shared")),
        ("/reset/sentinel_key", json!("different")),
    ] {
        let mut changed = successor.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        assert!(
            verify_request_projection(&original, &changed).is_err(),
            "{pointer}"
        );
    }
}
