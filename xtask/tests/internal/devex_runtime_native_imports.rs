use super::*;
use crate::devex::import_samples::{
    ImportSampleManifest, ManifestFile, PerformanceRequest, model, model_sha256,
    performance_username,
};

fn registered_sample(root: &Path) -> imports::Samples {
    let namespace = "a".repeat(20);
    let content = b"registered-sample";
    fs::write(root.join("0-0.xlsx"), content).unwrap();
    let request = PerformanceRequest {
        template: root.join("template.xlsx"),
        template_sha256: "b".repeat(64),
        model_sha256: model_sha256().unwrap(),
        directory: root.into(),
        namespace: namespace.clone(),
        concurrency: 1,
        cycles: 1,
    };
    let manifest = ImportSampleManifest {
        format_version: 1,
        namespace: namespace.clone(),
        template_sha256: request.template_sha256.clone(),
        model: model(),
        model_sha256: request.model_sha256.clone(),
        concurrency: 1,
        cycles: 1,
        files: vec![ManifestFile {
            sample: "0:0".into(),
            filename: "0-0.xlsx".into(),
            username: performance_username(&namespace, 0, 0),
            sha256: artifacts::sha(content),
        }],
    };
    imports::Samples::verify(root, &manifest, &request).unwrap()
}

#[test]
fn multipart_uses_registered_or_explicit_file_and_rejects_unknown_or_changed_samples() {
    let (address, calls, worker) = server(vec![
        (
            "200 OK",
            "Content-Type: application/json\r\n",
            r#"{"code":200}"#,
        ),
        (
            "200 OK",
            "Content-Type: application/json\r\n",
            r#"{"code":200}"#,
        ),
    ]);
    asynchronous(async {
        let temp = tempfile::tempdir().unwrap();
        let samples = registered_sample(temp.path());
        let fixed = temp.path().join("fixed.xlsx");
        fs::write(&fixed, b"explicit-file").unwrap();
        let mut config = pool(1);
        config["contract"]["timeout_ms"] = json!(1000);
        config["bindings"]["api_url"] = json!(address);
        let catalog = BTreeMap::from([(
            "post_upload".into(),
            json!({"method":"POST","path":"/upload"}),
        )]);
        let identity =
            json!({"tenant_id":"system","username":"fixture","client_address":"198.18.0.1"});
        let mut session =
            request::Session::new(&config, &catalog, identity, Some(&samples)).unwrap();
        let pacing = pacer(temp.path());
        let cancel = process::Cancellation::default();
        let step = |source: Value| {
            let mut file = json!({"field":"file","filename":"sample.xlsx","type":"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"});
            file.as_object_mut()
                .unwrap()
                .extend(source.as_object().unwrap().clone());
            json!({"operation":"post_upload","multipart":[file]})
        };
        session
            .request(&step(json!({"sample":"0:0"})), &pacing, &cancel)
            .await
            .unwrap();
        session
            .request(&step(json!({"path":fixed})), &pacing, &cancel)
            .await
            .unwrap();
        assert_eq!(session.requests, 2);
        let error = session
            .request(&step(json!({"sample":"100:0"})), &pacing, &cancel)
            .await
            .unwrap_err();
        assert_eq!(error.category, "import_unknown_sample");
        fs::write(temp.path().join("0-0.xlsx"), b"changed").unwrap();
        let error = session
            .request(&step(json!({"sample":"0:0"})), &pacing, &cancel)
            .await
            .unwrap_err();
        assert_eq!(error.category, "import_sample_changed");
        assert_eq!(session.requests, 2);
    });
    worker.join().unwrap();
    let calls = calls.lock().unwrap();
    assert!(calls[0].contains("registered-sample"));
    assert!(calls[1].contains("explicit-file"));
}
