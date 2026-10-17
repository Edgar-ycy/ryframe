use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use chrono::NaiveDate;
use serde_json::{Value, json};

use super::{
    check::supply_chain::{self, cargo_graph, local_patch, policy, policy::Policy, workflow},
    workspace::root_dir,
};

const DATE: &str = "2026-08-20";
const TOOLS: &str =
    "cargo-audit@0.22.2,cargo-deny@0.20.2,cargo-cyclonedx@0.5.9,sccache@0.17.0,trivy@0.72.0";
const ACTION_SHA: &str = "2222222222222222222222222222222222222222";
const UPLOAD_SHA: &str = "3333333333333333333333333333333333333333";
const DIGEST_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DIGEST_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn test_date() -> NaiveDate {
    NaiveDate::parse_from_str(DATE, "%Y-%m-%d").unwrap()
}

#[allow(clippy::too_many_arguments)]
fn profile(
    name: &str,
    package: &str,
    features: Value,
    required_packages: Value,
    required_features: Value,
    forbidden_packages: Value,
    forbidden_features: Value,
    maximum: Value,
) -> Value {
    json!({
        "name": name,
        "command": {
            "package": package,
            "no_default_features": true,
            "features": features,
            "edges": ["normal", "build"]
        },
        "constraints": {
            "required_packages": required_packages,
            "required_features": required_features,
            "forbidden_packages": forbidden_packages,
            "forbidden_features": forbidden_features,
            "max_unique_packages": maximum
        }
    })
}

fn policy_document(service_images: Value, local_patches: Value) -> Value {
    let aws_features = json!({
        "jsonwebtoken": ["aws_lc_rs"],
        "reqwest": ["__rustls-aws-lc-rs"],
        "rustls": ["aws_lc_rs"],
        "sqlx": ["tls-rustls-aws-lc-rs"],
        "tokio-rustls": ["aws_lc_rs"]
    });
    json!({
        "schema_version": 4,
        "tools": {
            "cargo-audit": "0.22.2",
            "cargo-deny": "0.20.2",
            "cargo-cyclonedx": "0.5.9",
            "sccache": "0.17.0",
            "trivy": "0.72.0"
        },
        "local_patch_licenses": local_patches,
        "service_images": service_images,
        "vulnerability_gate": {
            "severities": ["HIGH", "CRITICAL"],
            "exceptions": []
        },
        "runtime_feature_tree_gate": {
            "profiles": [
                profile(
                    "API",
                    "ryframe",
                    json!(["bin-api", "runtime-swagger-ui"]),
                    json!(["aws-lc-rs", "aws-lc-sys"]),
                    aws_features.clone(),
                    json!(["ring"]),
                    json!({"rustls": ["ring"]}),
                    Value::Null
                ),
                profile(
                    "Worker",
                    "ryframe",
                    json!(["bin-worker"]),
                    json!(["aws-lc-rs", "aws-lc-sys"]),
                    aws_features,
                    json!(["ring", "ryframe-api"]),
                    json!({"rustls": ["ring"]}),
                    Value::Null
                ),
                profile(
                    "Migrate",
                    "ryframe",
                    json!(["bin-migrate"]),
                    json!([]),
                    json!({}),
                    json!(["ryframe-api", "axum", "reqwest"]),
                    json!({}),
                    Value::Null
                ),
                profile(
                    "GeneratorDefault",
                    "ryframe-generator",
                    json!([]),
                    json!([]),
                    json!({}),
                    json!(["sea-orm", "sqlx", "ryframe-tenant-db", "tokio"]),
                    json!({}),
                    json!(90)
                )
            ]
        },
        "dependency_graph_exceptions": [{
            "package": "optional-package",
            "version": "1.2.3",
            "enforcement": "must_be_absent_from_resolved_graph",
            "owner": "security-team",
            "expires": "2099-12-31",
            "reason": "仅允许保留在锁文件中，不得进入实际构建图"
        }]
    })
}

fn load_policy(document: &Value) -> Policy {
    policy::parse(&document.to_string(), test_date()).unwrap()
}

fn workflow_with_run_steps(steps: &str) -> String {
    let steps = steps
        .lines()
        .map(|line| format!("      {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "jobs:\n  supply-chain:\n    steps:\n\
         \x20     - uses: taiki-e/install-action@{ACTION_SHA}\n\
         \x20       with:\n\
         \x20         tool: cargo-audit@0.22.2,cargo-deny@0.20.2,cargo-cyclonedx@0.5.9,sccache@0.17.0,trivy@0.72.0\n\
         \x20         fallback: none\n{steps}\n\
         \x20     - uses: actions/upload-artifact@{UPLOAD_SHA}\n"
    )
}

fn workflow_with_service_image(service: &str, image: &str) -> String {
    workflow_with_run_steps(
        "- run: cargo cyclonedx --format json\n\
         - run: trivy image --format cyclonedx target\n\
         - run: cargo xtask check ci security report cyclonedx --input /tmp/cargo.json --require-reproducible\n\
         - run: cargo xtask check ci security report cyclonedx --input /tmp/image.json\n\
         - run: cargo xtask check ci security report trivy --input /tmp/trivy.json",
    )
    .replace(
        "  supply-chain:\n    steps:",
        &format!(
            "  supply-chain:\n    services:\n      {service}:\n        image: {image}\n    steps:"
        ),
    )
}

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let parent = root_dir().join(".local-tests/supply-chain-unit");
        fs::create_dir_all(&parent).unwrap();
        let root = parent.join(format!(
            "case-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self { root }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        assert_eq!(
            self.root.parent().unwrap(),
            root_dir().join(".local-tests/supply-chain-unit")
        );
        fs::remove_dir_all(&self.root).unwrap();
    }
}

fn write_policy(fixture: &Fixture, document: &Value) -> PathBuf {
    let path = fixture.path("policy.json");
    fs::write(&path, serde_json::to_vec(document).unwrap()).unwrap();
    path
}

fn write_workflow(fixture: &Fixture, source: &str) -> (PathBuf, PathBuf) {
    let directory = fixture.path("workflows");
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("ci.yml");
    fs::write(&path, source).unwrap();
    (directory, path)
}

#[test]
fn complete_policy_and_checked_in_feature_surfaces_are_accepted() {
    let loaded = load_policy(&policy_document(json!([]), json!([])));
    assert_eq!(loaded.tools["trivy"], "0.72.0");
    assert_eq!(loaded.runtime_feature_tree_gate.profiles.len(), 4);

    let checked = policy::load_file(
        &root_dir().join("tools/policies/supply_chain_policy.json"),
        test_date(),
    )
    .unwrap();
    let profiles = checked
        .runtime_feature_tree_gate
        .profiles
        .iter()
        .map(|profile| profile.name.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        profiles,
        ["API", "GeneratorDefault", "Migrate", "Worker"]
            .into_iter()
            .collect()
    );
    let worker = checked
        .runtime_feature_tree_gate
        .profiles
        .iter()
        .find(|profile| profile.name == "Worker")
        .unwrap();
    assert!(
        worker
            .constraints
            .forbidden_packages
            .contains(&"ryframe-api".to_owned())
    );
}

#[test]
fn malformed_runtime_policy_fails_closed() {
    let mut invalid = Vec::new();
    let mut value = policy_document(json!([]), json!([]));
    value["runtime_feature_tree_gate"]["profiles"][0]
        .as_object_mut()
        .unwrap()
        .remove("command");
    invalid.push(value);
    let mut value = policy_document(json!([]), json!([]));
    value["runtime_feature_tree_gate"]["profiles"][1]["name"] = json!("API");
    invalid.push(value);
    let mut value = policy_document(json!([]), json!([]));
    value["runtime_feature_tree_gate"]["profiles"][0]["constraints"]["forbidden_packages"] =
        json!(["aws-lc-rs"]);
    invalid.push(value);
    let mut value = policy_document(json!([]), json!([]));
    value["runtime_feature_tree_gate"]["profiles"][0]["constraints"]["forbidden_features"] =
        json!({"rustls": ["aws_lc_rs"]});
    invalid.push(value);
    let mut value = policy_document(json!([]), json!([]));
    value["runtime_feature_tree_gate"]["profiles"][0]["command"]["no_default_features"] =
        json!("true");
    invalid.push(value);
    let mut value = policy_document(json!([]), json!([]));
    value["runtime_feature_tree_gate"]["profiles"][0]["command"]["edges"] = json!(["normal"]);
    invalid.push(value);
    let mut value = policy_document(json!([]), json!([]));
    value["runtime_feature_tree_gate"]["profiles"][3]["constraints"]["max_unique_packages"] =
        json!(0);
    invalid.push(value);
    let mut value = policy_document(json!([]), json!([]));
    value["runtime_feature_tree_gate"]["profiles"][1]["command"] =
        value["runtime_feature_tree_gate"]["profiles"][0]["command"].clone();
    invalid.push(value);
    let mut value = policy_document(json!([]), json!([]));
    value["runtime_feature_tree_gate"]["profiles"]
        .as_array_mut()
        .unwrap()
        .pop();
    invalid.push(value);
    for value in invalid {
        assert!(policy::parse(&value.to_string(), test_date()).is_err());
    }

    let mut expired = policy_document(json!([]), json!([]));
    expired["dependency_graph_exceptions"][0]["expires"] = json!(DATE);
    expired["dependency_graph_exceptions"][0]["owner"] = json!("");
    assert!(policy::parse(&expired.to_string(), test_date()).is_err());
}

fn local_patch_entry(mit: &[u8], apache: &[u8]) -> Value {
    let digest = |content: &[u8]| {
        use sha2::{Digest, Sha256};
        Sha256::digest(content)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    json!({
        "patch": "fixture-patch",
        "package": "fixture-package",
        "version": "1.2.3",
        "path": "vendor/fixture-package",
        "license_expression": "MIT OR Apache-2.0",
        "license_files": [
            {
                "spdx": "MIT",
                "file": "LICENSE-MIT",
                "sha256": digest(mit),
                "registry_source": {
                    "package": "fixture-source",
                    "version": "1.2.3",
                    "file": "LICENSE-MIT"
                }
            },
            {
                "spdx": "Apache-2.0",
                "file": "LICENSE-APACHE",
                "sha256": digest(apache),
                "registry_source": {
                    "package": "fixture-source",
                    "version": "1.2.3",
                    "file": "LICENSE-APACHE"
                }
            }
        ]
    })
}

fn patch_fixture(fixture: &Fixture) -> (Value, PathBuf, PathBuf, PathBuf) {
    let mit = b"fixture MIT license\n";
    let apache = b"fixture Apache license\n";
    let vendor = fixture.path("vendor/fixture-package");
    fs::create_dir_all(&vendor).unwrap();
    fs::write(
        fixture.path("Cargo.toml"),
        "[patch.crates-io]\nfixture-patch = { path = \"vendor/fixture-package\" }\n",
    )
    .unwrap();
    fs::write(
        vendor.join("Cargo.toml"),
        "[package]\nname = \"fixture-package\"\nversion = \"1.2.3\"\nlicense = \"MIT OR Apache-2.0\"\n",
    )
    .unwrap();
    fs::write(vendor.join("LICENSE-MIT"), mit).unwrap();
    fs::write(vendor.join("LICENSE-APACHE"), apache).unwrap();
    let cargo_home = fixture.path("cargo-home");
    let registry = cargo_home.join("registry/src/fixture-registry/fixture-source-1.2.3");
    fs::create_dir_all(&registry).unwrap();
    fs::write(registry.join("LICENSE-MIT"), mit).unwrap();
    fs::write(registry.join("LICENSE-APACHE"), apache).unwrap();
    (local_patch_entry(mit, apache), cargo_home, vendor, registry)
}

#[test]
fn local_patch_licenses_match_registry_hash_and_line_endings() {
    let fixture = Fixture::new();
    let (patch, cargo_home, vendor, registry) = patch_fixture(&fixture);
    let document = policy_document(json!([]), json!([patch]));
    let visible = load_policy(&document);
    assert!(local_patch::validate(&fixture.root, &visible, Some(&cargo_home)).is_empty());
    let empty_cargo_home = fixture.path("empty-cargo-home");
    fs::create_dir(&empty_cargo_home).unwrap();
    assert!(local_patch::validate(&fixture.root, &visible, Some(&empty_cargo_home)).is_empty());

    for directory in [&vendor, &registry] {
        let path = directory.join("LICENSE-APACHE");
        let bytes = fs::read(&path).unwrap();
        fs::write(
            &path,
            bytes
                .windows(1)
                .flat_map(|byte| {
                    if byte == b"\n" {
                        b"\r\n".to_vec()
                    } else {
                        byte.to_vec()
                    }
                })
                .collect::<Vec<_>>(),
        )
        .unwrap();
    }
    assert!(local_patch::validate(&fixture.root, &visible, Some(&cargo_home)).is_empty());

    fs::write(vendor.join("LICENSE-MIT"), b"tampered vendor license\n").unwrap();
    assert!(
        local_patch::validate(&fixture.root, &visible, Some(&cargo_home))
            .iter()
            .any(|error| error.contains("登记哈希不一致"))
    );
    fs::write(vendor.join("LICENSE-MIT"), b"fixture MIT license\n").unwrap();
    fs::write(
        registry.join("LICENSE-APACHE"),
        b"tampered registry license\n",
    )
    .unwrap();
    assert!(
        local_patch::validate(&fixture.root, &visible, Some(&cargo_home))
            .iter()
            .any(|error| error.contains("registry 原件不一致"))
    );
}

#[test]
fn local_patch_manifest_license_and_missing_sources_are_checked() {
    let fixture = Fixture::new();
    let (mut patch, cargo_home, _vendor, registry) = patch_fixture(&fixture);
    let registry_source = &mut patch["license_files"][1]["registry_source"];
    registry_source.as_object_mut().unwrap().remove("file");
    registry_source["manifest_license"] = json!("Apache-2.0");
    fs::write(
        registry.join("Cargo.toml"),
        "[package]\nname = \"fixture-source\"\nversion = \"1.2.3\"\nlicense = \"Apache-2.0\"\n",
    )
    .unwrap();
    let document = policy_document(json!([]), json!([patch]));
    let visible = load_policy(&document);
    assert!(local_patch::validate(&fixture.root, &visible, Some(&cargo_home)).is_empty());

    fs::write(
        registry.join("Cargo.toml"),
        "[package]\nname = \"fixture-source\"\nversion = \"1.2.3\"\nlicense = \"MIT\"\n",
    )
    .unwrap();
    assert!(
        local_patch::validate(&fixture.root, &visible, Some(&cargo_home))
            .iter()
            .any(|error| error.contains("registry 清单许可证不一致"))
    );

    let fixture = Fixture::new();
    let (patch, cargo_home, vendor, _) = patch_fixture(&fixture);
    fs::remove_file(vendor.join("LICENSE-APACHE")).unwrap();
    let visible = load_policy(&policy_document(json!([]), json!([patch])));
    assert!(
        local_patch::validate(&fixture.root, &visible, Some(&cargo_home))
            .iter()
            .any(|error| error.contains("Apache-2.0 文件"))
    );

    let fixture = Fixture::new();
    let (patch, _, _, _) = patch_fixture(&fixture);
    fs::write(
        fixture.path("Cargo.toml"),
        "[patch.crates-io]\nfixture-patch = { path = \"vendor/fixture-package\" }\nunknown = { path = \"vendor/unknown\" }\n",
    )
    .unwrap();
    let visible = load_policy(&policy_document(json!([]), json!([patch])));
    let empty_cargo_home = fixture.path("empty-cargo-home");
    fs::create_dir(&empty_cargo_home).unwrap();
    assert!(
        local_patch::validate(&fixture.root, &visible, Some(&empty_cargo_home))
            .iter()
            .any(|error| error.contains("未登记许可证原件"))
    );
}

#[test]
fn local_patch_policy_rejects_incomplete_hashes_and_paths() {
    let mit = b"fixture MIT license\n";
    let apache = b"fixture Apache license\n";
    let mut missing = local_patch_entry(mit, apache);
    missing["license_files"].as_array_mut().unwrap().pop();
    let mut unsafe_path = local_patch_entry(mit, apache);
    unsafe_path["path"] = json!("../vendor/fixture-package");
    let mut bad_hash = local_patch_entry(mit, apache);
    bad_hash["license_files"][0]["sha256"] = json!("A".repeat(64));
    for entry in [missing, unsafe_path, bad_hash] {
        assert!(
            policy::parse(
                &policy_document(json!([]), json!([entry])).to_string(),
                test_date()
            )
            .is_err()
        );
    }
}

#[test]
fn service_image_policy_and_workflow_set_must_match() {
    let fixture = Fixture::new();
    let image = format!("redis:7.4@sha256:{DIGEST_A}");
    let expected = json!([{
        "workflow": "ci.yml",
        "job": "supply-chain",
        "service": "redis",
        "image": image
    }]);
    let (directory, path) = write_workflow(&fixture, &workflow_with_service_image("redis", &image));
    assert!(
        workflow::validate(
            &directory,
            &load_policy(&policy_document(expected.clone(), json!([])))
        )
        .is_empty()
    );

    fs::write(
        &path,
        workflow_with_service_image("redis", &format!("redis:7.5@sha256:{DIGEST_A}")),
    )
    .unwrap();
    assert!(
        workflow::validate(
            &directory,
            &load_policy(&policy_document(expected.clone(), json!([])))
        )
        .iter()
        .any(|error| error.contains("服务镜像引用漂移"))
    );
    fs::write(
        &path,
        workflow_with_service_image("redis", &format!("redis:7.4@sha256:{DIGEST_B}")),
    )
    .unwrap();
    assert!(
        workflow::validate(
            &directory,
            &load_policy(&policy_document(expected.clone(), json!([])))
        )
        .iter()
        .any(|error| error.contains("服务镜像引用漂移"))
    );

    fs::write(&path, workflow_with_run_steps(
        "- run: cargo cyclonedx --format json\n- run: trivy image --format cyclonedx target\n- run: cargo xtask check ci security report cyclonedx --input /tmp/cargo.json --require-reproducible\n- run: cargo xtask check ci security report cyclonedx --input /tmp/image.json\n- run: cargo xtask check ci security report trivy --input /tmp/trivy.json"
    )).unwrap();
    assert!(
        workflow::validate(
            &directory,
            &load_policy(&policy_document(expected, json!([])))
        )
        .iter()
        .any(|error| error.contains("策略声明的服务镜像不存在"))
    );
    fs::write(&path, workflow_with_service_image("redis", &image)).unwrap();
    assert!(
        workflow::validate(
            &directory,
            &load_policy(&policy_document(json!([]), json!([])))
        )
        .iter()
        .any(|error| error.contains("工作流服务镜像未在策略声明"))
    );
}

#[test]
fn unsafe_service_image_policy_entries_fail_closed() {
    for invalid in [
        json!([{
            "workflow": "../ci.yml",
            "job": "supply-chain",
            "service": "redis",
            "image": format!("redis:7.4@sha256:{DIGEST_A}")
        }]),
        json!([{
            "workflow": "ci.yml",
            "job": "supply-chain",
            "service": "redis",
            "image": "redis:7.4"
        }]),
        json!([{
            "workflow": "ci.yml",
            "job": "supply-chain",
            "service": "redis",
            "image": format!("redis@sha256:{DIGEST_A}")
        }]),
        json!([
            {
                "workflow": "ci.yml",
                "job": "supply-chain",
                "service": "redis",
                "image": format!("redis:7.4@sha256:{DIGEST_A}")
            },
            {
                "workflow": "ci.yml",
                "job": "supply-chain",
                "service": "redis",
                "image": format!("redis:7.4@sha256:{DIGEST_A}")
            }
        ]),
        json!([{
            "workflow": "ci.yml",
            "job": "supply-chain",
            "service": "redis",
            "image": format!("redis:7.4@sha256:{DIGEST_A}"),
            "unknown": true
        }]),
    ] {
        assert!(
            policy::parse(
                &policy_document(invalid, json!([])).to_string(),
                test_date()
            )
            .is_err()
        );
    }
}

#[test]
fn yaml_services_support_inline_maps_aliases_empty_maps_and_yaml12_booleans() {
    let digest = DIGEST_A;
    for source in [
        format!(
            "jobs: {{integration: {{services: {{redis: {{image: 'redis:7.4@sha256:{digest}'}}}}}}}}\n"
        ),
        format!(
            "jobs:\n  integration:\n    services: &shared\n      redis:\n        image: redis:7.4@sha256:{digest}\n  repeated:\n    services: *shared\n"
        ),
        format!(
            "jobs:\n  integration:\n    services:\n      redis: &redis-service\n        image: redis:7.4@sha256:{digest}\n  repeated:\n    services: {{redis: *redis-service}}\n"
        ),
        "jobs:\n  integration:\n    services: {}\n  check:\n    runs-on: ubuntu-latest\n".into(),
        format!("jobs:\n  on:\n    services:\n      yes:\n        image: redis@sha256:{digest}\n"),
    ] {
        assert!(
            workflow::validate_service_images(Path::new("ci.yml"), &source).is_empty(),
            "{source}"
        );
    }
    let duplicate = format!(
        "jobs:\n  integration:\n    services:\n      redis:\n        image: redis@sha256:{DIGEST_A}\n        image: redis@sha256:{DIGEST_B}\n"
    );
    assert!(
        workflow::validate_service_images(Path::new("ci.yml"), &duplicate)
            .iter()
            .any(|error| error.contains("重复键"))
    );
    assert!(
        workflow::validate_service_images(
            Path::new("ci.yml"),
            "jobs:\n  integration:\n    services:\n"
        )
        .iter()
        .any(|error| error.contains("services 必须是对象"))
    );
    assert!(
        workflow::validate_service_images(
            Path::new("ci.yml"),
            "jobs:\n  integration:\n    services:\n      redis:\n        ports: [6379]\n"
        )
        .iter()
        .any(|error| error.contains("缺少固定 image"))
    );
}

#[test]
fn action_pins_install_tools_aliases_and_duplicate_keys_are_checked() {
    let action_sha = "1".repeat(40);
    let reusable_sha = "2".repeat(40);
    let install_sha = "3".repeat(40);
    let policy = load_policy(&policy_document(json!([]), json!([])));
    let inline = format!(
        "jobs: {{reusable: {{uses: 'owner/repo/.github/workflows/check.yml@{reusable_sha}'}}, check: {{steps: [{{uses: 'owner/action@{action_sha}'}}, {{uses: 'docker://owner/image@sha256:{DIGEST_A}'}}, {{uses: './.github/actions/local'}}, {{uses: 'taiki-e/install-action@{install_sha}', with: {{tool: 'sccache@0.17.0', fallback: 'none'}}}}]}}}}\n"
    );
    assert!(workflow::validate_action_uses(Path::new("ci.yml"), &inline, &policy).is_empty());
    let mutable = inline.replace(&format!("owner/action@{action_sha}"), "owner/action@main");
    assert!(
        workflow::validate_action_uses(Path::new("ci.yml"), &mutable, &policy)
            .iter()
            .any(|error| error.contains("action 未固定"))
    );
    let mutable_workflow = inline.replace(&format!("check.yml@{reusable_sha}"), "check.yml@main");
    assert!(
        workflow::validate_action_uses(Path::new("ci.yml"), &mutable_workflow, &policy)
            .iter()
            .any(|error| error.contains("可复用工作流 未固定"))
    );

    let alias = format!(
        "jobs:\n  check:\n    steps: &shared-steps\n      - &install-step\n        uses: taiki-e/install-action@{install_sha}\n        with:\n          tool: sccache@0.17.0\n          fallback: none\n  repeated:\n    steps: *shared-steps\n"
    );
    assert!(workflow::validate_action_uses(Path::new("ci.yml"), &alias, &policy).is_empty());
    let drift = alias.replace("fallback: none", "fallback: automatic");
    assert_eq!(
        workflow::validate_action_uses(Path::new("ci.yml"), &drift, &policy)
            .iter()
            .filter(|error| error.contains("必须禁用 fallback"))
            .count(),
        2
    );
    let version_drift = alias.replace("sccache@0.17.0", "sccache@latest");
    assert_eq!(
        workflow::validate_action_uses(Path::new("ci.yml"), &version_drift, &policy)
            .iter()
            .filter(|error| error.contains("未固定到完整版本"))
            .count(),
        2
    );
    let mixed_case = alias
        .replace("taiki-e/install-action@", "Taiki-E/Install-Action@")
        .replace("fallback: none", "fallback: automatic");
    assert_eq!(
        workflow::validate_action_uses(Path::new("ci.yml"), &mixed_case, &policy)
            .iter()
            .filter(|error| error.contains("必须禁用 fallback"))
            .count(),
        2
    );

    let duplicate = format!(
        "jobs:\n  check:\n    steps:\n      - uses: owner/action@{action_sha}\n        uses: owner/action@{action_sha}\n"
    );
    assert!(
        workflow::validate_action_uses(Path::new("ci.yml"), &duplicate, &policy)
            .iter()
            .any(|error| error.contains("重复键"))
    );

    let invalid_steps = format!(
        "jobs:\n  mapping:\n    steps: {{uses: owner/action@{action_sha}}}\n  entries:\n    steps:\n      - 42\n      - uses: [owner/action@{action_sha}]\n      - run: [cargo, cyclonedx]\n"
    );
    let errors = workflow::validate_action_uses(Path::new("ci.yml"), &invalid_steps, &policy);
    for expected in [
        "steps 必须是数组",
        "steps[0] 必须是对象",
        "uses 必须是静态字符串",
        "run 必须是静态字符串",
    ] {
        assert!(
            errors.iter().any(|error| error.contains(expected)),
            "{errors:?}"
        );
    }
}

#[test]
fn workflow_environment_contexts_are_rejected_only_at_unsafe_levels() {
    let fixture = Fixture::new();
    let (directory, path) = write_workflow(
        &fixture,
        "jobs:\n  integration:\n    runs-on: ubuntu-latest\n    env:\n      ARTIFACT_DIR: ${{ runner.temp }}/integration\n    steps:\n      - run: echo ok\n",
    );
    let policy = load_policy(&policy_document(json!([]), json!([])));
    assert!(
        workflow::validate(&directory, &policy)
            .iter()
            .any(|error| error.contains("不能引用 runner 上下文"))
    );
    fs::write(
        &path,
        "jobs:\n  integration:\n    runs-on: ubuntu-latest\n    steps:\n      - env:\n          ARTIFACT_DIR: ${{ runner.temp }}/integration\n        run: echo ok\n",
    )
    .unwrap();
    assert!(
        !workflow::validate(&directory, &policy)
            .iter()
            .any(|error| error.contains("runner 上下文"))
    );
}

#[test]
fn workflow_requires_every_policy_tool_and_artifact_upload() {
    let fixture = Fixture::new();
    let baseline = workflow_with_run_steps(
        "- run: cargo cyclonedx --format json\n- run: trivy image --format cyclonedx target\n- run: cargo xtask check ci security report cyclonedx --input /tmp/cargo.json --require-reproducible\n- run: cargo xtask check ci security report cyclonedx --input /tmp/image.json\n- run: cargo xtask check ci security report trivy --input /tmp/trivy.json",
    );
    let without_upload = baseline
        .lines()
        .filter(|line| !line.contains("actions/upload-artifact"))
        .collect::<Vec<_>>()
        .join("\n");
    let without_tool = baseline.replace(",sccache@0.17.0", "");
    let policy = load_policy(&policy_document(json!([]), json!([])));
    for (source, expected) in [
        (without_upload, "actions/upload-artifact"),
        (without_tool, "工具 sccache@0.17.0"),
    ] {
        let directory = fixture.path(expected);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("ci.yml"), source).unwrap();
        assert!(
            workflow::validate(&directory, &policy)
                .iter()
                .any(|error| error.contains(expected))
        );
    }
}

#[test]
fn workflow_command_gates_ignore_comments_and_accept_folded_commands() {
    let fixture = Fixture::new();
    let (directory, path) = write_workflow(
        &fixture,
        &workflow_with_run_steps(
            "- run: |\n    # cargo cyclonedx --format json\n    cargo cyclonedx\n    echo \"--format json\"\n    trivy image --format json target\n    echo \"--format cyclonedx\"\n    docker run --label owner/fake@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa owner/image:latest",
        ),
    );
    let policy = load_policy(&policy_document(json!([]), json!([])));
    let errors = workflow::validate(&directory, &policy);
    for expected in [
        "cargo cyclonedx --format json",
        "trivy image --format cyclonedx",
        "security report trivy",
        "docker run 镜像未固定",
    ] {
        assert!(
            errors.iter().any(|error| error.contains(expected)),
            "{expected}: {errors:?}"
        );
    }

    fs::write(
        &path,
        workflow_with_run_steps(&format!(
            "- run: |\n    cargo cyclonedx \\\n      --format json\n    cargo xtask check ci security report cyclonedx \\\n      --input /tmp/cargo.json \\\n      --require-reproducible\n    cargo xtask check ci security report cyclonedx \\\n      --input /tmp/image.json\n    cargo xtask check ci security report trivy \\\n      --input /tmp/trivy.json\n    docker run --rm \\\n      owner/image@sha256:{DIGEST_B} check\n- run: >-\n    trivy image\n    --format cyclonedx\n    target\n- run: >-\n    docker pull\n    owner/image@sha256:{DIGEST_B}"
        )),
    )
    .unwrap();
    assert!(workflow::validate(&directory, &policy).is_empty());
}

#[test]
fn legacy_or_ambiguous_report_commands_and_sensitive_references_are_rejected() {
    let fixture = Fixture::new();
    let legacy = format!(
        "{}\n      - run: python tools/python/check_supply_chain.py --cyclonedx /tmp/legacy.json",
        workflow_with_run_steps(
            "- run: cargo cyclonedx --format json\n- run: trivy image --format cyclonedx target\n- run: cargo xtask check ci security report cyclonedx --input /tmp/cargo.json --require-reproducible\n- run: cargo xtask check ci security report cyclonedx --input /tmp/image.json\n- run: cargo xtask check ci security report trivy --input /tmp/trivy.json"
        )
    );
    let (directory, path) = write_workflow(&fixture, &legacy);
    let policy = load_policy(&policy_document(json!([]), json!([])));
    assert!(
        workflow::validate(&directory, &policy)
            .iter()
            .any(|error| error.contains("禁止直接调用"))
    );

    let sensitive = "jobs:\n  check:\n    steps:\n      - uses: ${{ secrets.DEPLOY_TOKEN }}\n";
    let errors = workflow::validate_action_uses(Path::new("ci.yml"), sensitive, &policy);
    assert!(errors.iter().any(|error| error.contains("未固定")));
    assert!(!errors.iter().any(|error| error.contains("DEPLOY_TOKEN")));

    fs::write(
        &path,
        workflow_with_run_steps(
            "- run: cargo cyclonedx --format json\n- run: trivy image --format cyclonedx target\n- run: cargo xtask check ci security report cyclonedx --input /tmp/a.json --input /tmp/b.json --require-reproducible\n- run: cargo xtask check ci security report cyclonedx --input /tmp/image.json\n- run: cargo xtask check ci security report trivy --input /tmp/trivy.json --require-reproducible",
        ),
    )
    .unwrap();
    let errors = workflow::validate(&directory, &policy);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("--input 不能重复"))
    );
    assert!(errors.iter().any(|error| error.contains("trivy 不支持")));

    fs::write(&path, "jobs: [\n").unwrap();
    assert!(
        workflow::validate(&directory, &policy)
            .iter()
            .any(|error| error.contains("无法安全解析"))
    );
}

#[test]
fn resolved_graph_and_feature_tree_constraints_fail_closed() {
    let policy = load_policy(&policy_document(json!([]), json!([])));
    assert_eq!(
        cargo_graph::resolved_graph_violations("root v0.1.0\noptional-package v1.2.3\n", &policy)
            .len(),
        1
    );
    let api = &policy.runtime_feature_tree_gate.profiles[0];
    let accepted = "aws-lc-rs v1.17.0|alloc,aws-lc-sys,default,ring-io,ring-sig-verify\naws-lc-sys v0.41.0|prebuilt-nasm\njsonwebtoken v11.0.0|aws_lc_rs\nreqwest v0.13.4|__rustls,__rustls-aws-lc-rs,rustls\nrustls v0.23.40|aws-lc-rs,aws_lc_rs,std,tls12\nsqlx v0.9.0|runtime-tokio,tls-rustls-aws-lc-rs\ntokio-rustls v0.26.4|aws_lc_rs,aws-lc-rs\n";
    assert!(cargo_graph::feature_tree_violations(accepted, api).is_empty());
    let rejected = "aws-lc-rs v1.17.0|default\njsonwebtoken v11.0.0|ring\nreqwest v0.13.4|__rustls\nrustls v0.23.40|ring\n";
    let errors = cargo_graph::feature_tree_violations(rejected, api);
    assert!(errors.iter().any(|error| error.contains("rustls")));
    assert!(errors.iter().any(|error| error.contains("缺少 aws-lc-sys")));
    let ring = "aws-lc-rs v1.17.0|default\naws-lc-sys v0.41.0|\njsonwebtoken v11.0.0|aws_lc_rs\nreqwest v0.13.4|__rustls-aws-lc-rs\nring v0.17.14|default\nrustls v0.23.40|aws_lc_rs\nsqlx v0.9.0|tls-rustls-aws-lc-rs\ntokio-rustls v0.26.4|aws_lc_rs\n";
    assert!(
        cargo_graph::feature_tree_violations(ring, api)
            .iter()
            .any(|error| error.contains("禁止的 ring"))
    );
    assert!(
        cargo_graph::feature_tree_violations("not-a-package\n", api)
            .iter()
            .any(|error| error.contains("格式无效"))
    );
    assert!(!cargo_graph::feature_tree_violations("", api).is_empty());
    let mut limited = api.clone();
    limited.constraints.max_unique_packages = Some(1);
    assert!(
        cargo_graph::feature_tree_violations(
            "ryframe v0.13.1|\nserde v1.0.0|derive\nserde v1.0.0|derive (*)\n",
            &limited
        )
        .iter()
        .any(|error| error.contains("unique package closure"))
    );
    let arguments = cargo_graph::feature_tree_args(api);
    assert_eq!(
        arguments,
        [
            "tree",
            "--locked",
            "-p",
            "ryframe",
            "--no-default-features",
            "--features",
            "bin-api,runtime-swagger-ui",
            "--edges",
            "normal,build",
            "--prefix",
            "none",
            "--format",
            "{p}|{f}"
        ]
    );
}

#[test]
fn checked_in_supply_chain_is_native_successful_and_read_only() {
    let root = root_dir();
    let workflow_directory = root.join(".github/workflows");
    let before = snapshot(&workflow_directory);
    supply_chain::run_at(
        &root,
        &root.join("tools/policies/supply_chain_policy.json"),
        &workflow_directory,
        false,
    )
    .unwrap();
    assert_eq!(snapshot(&workflow_directory), before);
}

fn snapshot(root: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    fn collect(root: &Path, output: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                collect(&path, output);
            } else {
                output.insert(path.clone(), fs::read(path).unwrap());
            }
        }
    }
    let mut output = std::collections::BTreeMap::new();
    collect(root, &mut output);
    output
}
