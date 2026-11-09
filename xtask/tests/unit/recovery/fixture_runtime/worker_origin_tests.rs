use super::*;

#[test]
fn worker_origin_rejects_independent_root_source_program_and_file_drift_before_intent() {
    let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for case in [
        "root",
        "source",
        "program",
        "binding",
        "source-drift",
        "program-drift",
        "binding-drift",
    ] {
        let mut f = Fixture::new("dev");
        f.execute("bind").unwrap();
        let request = f.request("browser");
        let paths = plan::Paths {
            backend: f.backend.clone(),
            environment: f.bootstrap.clone(),
            output: f.runtime.clone(),
            binding: path(&request["browser_binding"]).unwrap(),
        };
        let context = plan::make(&mut f, &paths, "run-a", "dev", true, false, false).unwrap();
        let product = fs::read(f.backend.join("Cargo.toml")).unwrap();
        match case {
            "root" => f.worker_origin["root"] = json!(f.backend),
            "source" => f.worker_origin["source"]["head"] = json!("0".repeat(40)),
            "program" => f.worker_origin["executable"]["path"] = json!(f.tool),
            "binding" => {
                let foreign = f.runtime.join("foreign-origin.json");
                fs::copy(
                    f.worker_origin["binding"]["path"].as_str().unwrap(),
                    &foreign,
                )
                .unwrap();
                f.worker_origin["binding"] = bound(&foreign).unwrap();
            }
            "source-drift" => {
                let root = path(&f.worker_origin["root"]).unwrap();
                fs::write(root.join("Cargo.toml"), b"changed source").unwrap();
            }
            "program-drift" | "binding-drift" => {
                let field = if case == "program-drift" {
                    "executable"
                } else {
                    "binding"
                };
                let file = path(&f.worker_origin[field]["path"]).unwrap();
                fs::write(
                    &file,
                    vec![b'x'; fs::metadata(&file).unwrap().len() as usize],
                )
                .unwrap();
            }
            _ => unreachable!(),
        }
        if case.ends_with("-drift") {
            assert!(context.unchanged().is_err(), "{case}");
        }
        assert!(f.execute("browser").is_err(), "{case}");
        assert_eq!(
            fs::read(f.backend.join("Cargo.toml")).unwrap(),
            product,
            "{case}"
        );
        assert!(!f.outputs()["intent"].exists(), "{case}");
        assert!(!f.outputs()["result"].exists(), "{case}");
        assert!(
            !f.calls.iter().any(|call| call.starts_with("frontend:")),
            "{case}"
        );
    }
}
