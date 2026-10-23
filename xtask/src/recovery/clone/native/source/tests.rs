//! 来源域、摘要编码与完整清单契约的行为回归。
use serde_json::{Value, json};

use super::{domains, protocol};

fn inventory(paths: &[&str]) -> Value {
    let mut paths = paths.to_vec();
    paths.sort_unstable();
    json!({"source":{"snapshot":{"head":"a".repeat(40),"patch_sha256":"b".repeat(64),
        "files":[],"clean":true},"worktree_fingerprint":format!("sha256:{}", "c".repeat(64))},
        "files":paths.iter().map(|path| json!({"path":path,"sha256":"d".repeat(64)})).collect::<Vec<_>>(),
        "guard":{"head":"a".repeat(40),"index_sha256":"e".repeat(64),"modes_sha256":"f".repeat(64)}})
}

fn request(action: &str, value: &Value, repository: &str) -> crate::Result<Value> {
    protocol::request(&json!({"action":action,"inventory":value,"repository":repository}))
}

#[test]
fn path_domains_preserve_conservative_product_and_reject_escapes() {
    for path in [
        "Cargo.lock",
        "config/app.toml",
        "vendor/lib.rs",
        "xtask/src/main.rs",
        "unknown/input.dat",
        "README.en.md.rs",
        "other/README.en.md",
        "src/content.en.md",
    ] {
        assert_eq!(domains::source_domain(path).unwrap(), "product");
    }
    for path in [
        "tools/python/check.py",
        "crates/app/tests/fixture.json",
        ".github/workflows/ci.yml",
        "tests/test.rs",
        "xtask/tests/fixture.json",
    ] {
        assert_eq!(domains::source_domain(path).unwrap(), "test_tools");
    }
    for path in [
        "README.md",
        "README.en.md",
        "CHANGELOG.en.md",
        "docs/data.en.md",
    ] {
        assert_eq!(domains::source_domain(path).unwrap(), "support");
    }
    for path in [
        "",
        ".",
        "../outside",
        "C:/outside",
        "C:relative",
        "nested\\file",
        "/absolute",
        "a//b",
        "a/./b",
        "a/",
        "a/../b",
    ] {
        assert!(domains::source_domain(path).is_err(), "{path}");
    }
}

#[test]
fn backend_roles_and_tools_preserve_independent_api_worker_inputs() {
    let value = inventory(&[
        "Cargo.toml",
        "crates/ryframe-api/src/lib.rs",
        "crates/ryframe/src/main.rs",
        "crates/ryframe/src/bin/ryframe_worker.rs",
        "crates/ryframe/src/bin/ryframe_worker/probe.rs",
        "crates/ryframe/src/reset/mod.rs",
        "crates/ryframe-generator/src/lib.rs",
        "crates/ryframe/src/boot.rs",
        "crates/db/tests/check.rs",
        "openapi/openapi.json",
        "tools/python/source_inventory.py",
        "xtask/src/main.rs",
        "unknown/input.dat",
    ]);
    let before = request("domains", &value, "backend").unwrap();
    let api = before["product"]["api"]["files"].as_array().unwrap();
    let worker = before["product"]["worker"]["files"].as_array().unwrap();
    assert!(api.contains(&json!("crates/ryframe-api/src/lib.rs")));
    assert!(api.contains(&json!("crates/ryframe/src/main.rs")));
    assert!(api.contains(&json!("openapi/openapi.json")));
    assert!(!worker.contains(&json!("openapi/openapi.json")));
    assert!(!worker.contains(&json!("crates/ryframe-api/src/lib.rs")));
    assert!(!api.contains(&json!("crates/ryframe/src/bin/ryframe_worker.rs")));
    assert!(worker.contains(&json!("crates/ryframe/src/bin/ryframe_worker.rs")));
    for paths in [api, worker] {
        for path in [
            "crates/ryframe/src/reset/mod.rs",
            "crates/ryframe-generator/src/lib.rs",
            "tools/python/source_inventory.py",
            "xtask/src/main.rs",
            "crates/db/tests/check.rs",
        ] {
            assert!(!paths.contains(&json!(path)), "{path}");
        }
        assert!(paths.contains(&json!("unknown/input.dat")));
        assert!(paths.contains(&json!("crates/ryframe/src/boot.rs")));
    }
    let mut changed = value.clone();
    for item in changed["files"].as_array_mut().unwrap() {
        if item["path"] == "tools/python/source_inventory.py" {
            item["sha256"] = json!("1".repeat(64));
        }
    }
    let after = request("domains", &changed, "backend").unwrap();
    assert_eq!(before["product"], after["product"]);
    assert_ne!(before["tools"], after["tools"]);
    assert_ne!(before["full"], after["full"]);
}

#[test]
fn frontend_domains_keep_documents_tools_and_unknown_product_inputs() {
    let value = inventory(&[
        "src/app.vue",
        "public/icon.svg",
        "openapi/source.json",
        "package.json",
        "pnpm-lock.yaml",
        ".node-version",
        "vite.config.ts",
        "unknown/input.dat",
        "README.en.md",
        "CHANGELOG.en.md",
        "ARCHITECTURE.en.md",
        "scripts/check.mjs",
        "tests/check.ts",
        ".github/workflows/ci.yml",
        "config/tools.json",
        "vitest.config.ts",
        ".gitignore",
    ]);
    let domains = request("domains", &value, "frontend").unwrap();
    let product = domains["product"]["frontend"]["files"].as_array().unwrap();
    for path in [
        "src/app.vue",
        "public/icon.svg",
        "openapi/source.json",
        "package.json",
        "pnpm-lock.yaml",
        ".node-version",
        "vite.config.ts",
        "unknown/input.dat",
    ] {
        assert!(product.contains(&json!(path)), "{path}");
    }
    for path in [
        "README.en.md",
        "CHANGELOG.en.md",
        "ARCHITECTURE.en.md",
        "scripts/check.mjs",
        "tests/check.ts",
        ".github/workflows/ci.yml",
        "config/tools.json",
        "vitest.config.ts",
        ".gitignore",
    ] {
        assert!(!product.contains(&json!(path)), "{path}");
    }
    assert_eq!(domains["full"], value);
    assert_eq!(domains["tools"]["files"].as_array().unwrap().len(), 5);
}

#[test]
fn digest_is_python_compatible_utf8_sorted_compact_json() {
    let files = json!([{"path":"中文 来源.rs","sha256":"a".repeat(64)}]);
    let result = protocol::request(&json!({"action":"fingerprints","files":files})).unwrap();
    // 固定独立向量来自 Python json.dumps(ensure_ascii=False,sort_keys=True,separators=(',',':'))。
    assert_eq!(
        result["product"]["sha256"],
        "434cb81b85fb0110f9f59b6a79a4772247ae1fc24dba77c522040696667701c6"
    );
    assert_eq!(result["product"]["files"], 1);
    assert_eq!(
        result["support"]["sha256"],
        "4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945"
    );
    for files in [
        json!([{"path":"a","sha256":"a".repeat(64)},{"path":"a","sha256":"a".repeat(64)}]),
        json!([{"path":"b","sha256":"a".repeat(64)},{"path":"a","sha256":"a".repeat(64)}]),
        json!([{"path":"a","sha256":"INVALID"}]),
        json!([{"path":"../escape","sha256":"a".repeat(64)}]),
        json!([{"path":"a","sha256":"a".repeat(64),"extra":true}]),
    ] {
        assert!(protocol::request(&json!({"action":"fingerprints","files":files})).is_err());
    }
}

#[test]
fn complete_inventory_and_domains_reject_tampering_and_unknown_fields() {
    let value = inventory(&["Cargo.toml", "README.en.md"]);
    for (path, replacement) in [
        ("/source/snapshot/clean", json!(1)),
        ("/source/snapshot/head", json!("bad")),
        ("/source/worktree_fingerprint", json!("sha256:bad")),
        ("/guard/head", json!("b".repeat(40))),
        ("/guard/index_sha256", json!("bad")),
        ("/files/0/path", json!("../outside")),
        ("/files/0/sha256", json!("BAD")),
    ] {
        let mut changed = value.clone();
        *changed.pointer_mut(path).unwrap() = replacement;
        assert!(
            protocol::request(&json!({"action":"validate","inventory":changed})).is_err(),
            "{path}"
        );
    }
    for repository in ["backend", "frontend"] {
        let domains = request("domains", &value, repository).unwrap();
        assert_eq!(
            protocol::request(&json!({"action":"validate-domains","value":domains,
            "repository":repository}))
            .unwrap(),
            domains
        );
        let mut changed = domains;
        changed["tools"]["sha256"] = json!("0".repeat(64));
        assert!(
            protocol::request(&json!({"action":"validate-domains","value":changed,
            "repository":repository}))
            .is_err()
        );
    }
    assert!(request("domains", &value, "invalid").is_err());
    assert!(
        protocol::request(&json!({"action":"domain","path":"Cargo.toml","extra":true})).is_err()
    );
}

#[path = "tests/git.rs"]
mod git;

#[test]
fn capture_rejects_snapshot_fingerprint_and_guard_head_races() {
    let mut original = super::validate_inventory(&inventory(&["Cargo.toml"])).unwrap();
    original.source.snapshot.files = original.files.clone();
    for mode in [
        "snapshot",
        "snapshot-head",
        "added",
        "deleted",
        "fingerprint",
        "head",
    ] {
        let mut snapshots = 0;
        let mut fingerprints = 0;
        let result = super::capture_observed(
            || {
                snapshots += 1;
                let mut snapshot = original.source.snapshot.clone();
                if snapshots == 2 {
                    match mode {
                        "snapshot" => snapshot.clean = !snapshot.clean,
                        "snapshot-head" => snapshot.head = "0".repeat(40),
                        "added" => snapshot.files.push(super::FileRecord {
                            path: "new.rs".into(),
                            sha256: "1".repeat(64),
                        }),
                        "deleted" => snapshot.files.clear(),
                        _ => {}
                    }
                }
                Ok(snapshot)
            },
            |_| {
                fingerprints += 1;
                Ok(if mode == "fingerprint" && fingerprints == 2 {
                    format!("sha256:{}", "1".repeat(64))
                } else {
                    original.source.worktree_fingerprint.clone()
                })
            },
            || {
                let mut guard = original.guard.clone();
                if mode == "head" {
                    guard.head = "0".repeat(40);
                }
                Ok((original.files.clone(), guard))
            },
        );
        assert!(result.is_err(), "{mode}");
    }
}
