//! 生产者共用已发布登记链，不把 amendment 正文当作完整请求。
use std::path::Path;

use serde_json::Value;

use super::super::{json, resources::common, seed};
use crate::Result;

pub(crate) fn resolve(
    backend: &Path,
    run: &Path,
    stage: &str,
    descriptor: &Value,
    current: Option<u64>,
    cleanup: bool,
) -> Result<Value> {
    match stage {
        "post-copy" => {
            Ok(seed::history::post(backend, run, Some(descriptor), current, cleanup)?.request)
        }
        "seed-runtime" => {
            let path = common::bound_file(backend, descriptor, "seed 生产者登记")?;
            if path != run.join("seed-runtime.json") {
                return Err("seed 生产者登记不属于本 run".into());
            }
            let request = json::read(&path)?;
            if common::binding(&path)? != *descriptor {
                return Err("seed 生产者登记在读取期间变化".into());
            }
            Ok(request)
        }
        _ => Err("原生生产者历史不属于明确阶段".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recovery::clone::native::{digest, lifecycle::state};
    use serde_json::json;
    use std::fs;

    fn published(run: &Path, mode: &str, result: &Value) {
        let (number, _) = state::begin(run, "post-copy", mode, &json!({})).unwrap();
        state::finish(run, number, Some(result), None).unwrap();
    }

    fn historical_session(backend: &Path, run: &Path, request: &Value, descriptor: &Value) -> u64 {
        let (number, attempt) = state::begin(run, "post-copy", "schedules", &json!({})).unwrap();
        let output = run.join("post-copy").join(format!("attempt-{number:04}"));
        fs::create_dir_all(&output).unwrap();
        let source = backend.join("xtask/src/recovery/clone/native/session.rs");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, "// 历史生产者 fixture\n").unwrap();
        let tool = json!({"path":backend.join(".local-tests/tool.exe"),"sha256":"a".repeat(64)});
        let plan = super::super::command::build(&json!({"backend":backend,"run_dir":run,
            "attempt":number,"kind":"session","output":output,"tool":tool,
            "registration":request,"configuration":null}))
        .unwrap();
        let manifest = common::binding(&run.join("manifest.json")).unwrap();
        let controller = state::controller_path(run, number);
        json::write_new(
            &controller,
            &json!({"format_version":1,"kind":"devex-stage-controller",
            "attempt":number,"attempt_sha256":digest::plan_hash(&attempt).unwrap(),
            "owner":{"directory":run,"manifest_sha256":manifest["sha256"]}}),
        )
        .unwrap();
        let launch = output.join("session-launch.json");
        json::write_new(&launch, &json!({"format_version":1,"kind":"devex-post-producer-launch",
            "run_manifest":manifest,"registration":descriptor,"controller":common::binding(&controller).unwrap(),
            "attempt":number,"producer_kind":"session","command":plan["argv"],"tool":tool,
            "source":plan["source"]})).unwrap();
        json::write_new(
            &output.join("session-process.json"),
            &json!({"format_version":1,
            "kind":"devex-post-producer","launch":common::binding(&launch).unwrap(),
            "identity":{"pid":std::process::id(),"started":"1","executable":tool["path"]},
            "producer_owner":uuid::Uuid::new_v4().to_string()}),
        )
        .unwrap();
        state::finish(
            run,
            number,
            Some(&json!({"status":"historical_session_finished"})),
            None,
        )
        .unwrap();
        number
    }

    #[test]
    fn amendment_resolves_full_request_and_keeps_historical_binding() {
        let root = tempfile::tempdir().unwrap();
        let backend = root.path();
        let run = backend.join(".local-tests/run");
        fs::create_dir_all(run.join("post-copy/amendments")).unwrap();
        json::write_new(&run.join("manifest.json"), &json!({})).unwrap();
        state::initialize(&run).unwrap();
        let original = json!({"api_environment":{"original":true},"private_metadata":"unchanged"});
        json::write_new(&run.join("post-copy.json"), &original).unwrap();
        let base = common::binding(&run.join("post-copy.json")).unwrap();
        published(
            &run,
            "register",
            &json!({"status":"post_copy_registered","registration":base}),
        );
        assert_eq!(
            resolve(backend, &run, "post-copy", &base, None, false).unwrap(),
            original
        );
        let historical = historical_session(backend, &run, &original, &base);
        let corrected =
            json!({"api_environment":{"corrected":true},"private_metadata":"unchanged"});
        let path = run.join("post-copy/amendments/0001.json");
        let amendment = json!({"format_version":1,"kind":"devex-clone-post-copy-amendment",
            "sequence":1,"run_manifest":state::manifest_binding(&run).unwrap(),"predecessor":base,
            "request_sha256":digest::plan_hash(&corrected).unwrap(),
            "api_environment":corrected["api_environment"],"invalidated_runtime":null});
        json::write_new(&path, &amendment).unwrap();
        let descriptor = common::binding(&path).unwrap();
        published(
            &run,
            "amend",
            &json!({"status":"post_copy_amended","registration":descriptor,
            "sequence":1,"predecessor":base,"runtime_directory":run.join("post-copy").join("runtime-0001"),
            "remote_writes":0,"worker_must_remain_stopped":true,"restore_qualified":false}),
        );
        assert_ne!(json::read(&path).unwrap(), corrected);
        assert_eq!(
            resolve(backend, &run, "post-copy", &descriptor, None, true).unwrap(),
            corrected
        );
        assert_eq!(
            resolve(backend, &run, "post-copy", &base, None, true).unwrap(),
            original
        );
        // 清理可以读历史；启动仍必须通过完整失效证据，不能借清理模式放行。
        assert!(resolve(backend, &run, "post-copy", &descriptor, None, false).is_err());
        assert!(resolve(backend, &run, "post-copy", &base, None, false).is_err());
        let attempts = state::load(&run, false).unwrap();
        let mut registrations = json!({});
        for attempt in state::attempts(&attempts).unwrap() {
            registrations[attempt["number"].as_u64().unwrap().to_string()] =
                json!({"request":corrected,"binding":descriptor});
        }
        let quiet = json!({"backend":backend,"run_dir":run,"current_attempt":null,"registrations":registrations});
        assert_eq!(
            super::super::inspection::quiet(&quiet).unwrap()["quiet"],
            true
        );
        fs::write(state::controller_path(&run, historical), b"{}\n").unwrap();
        assert!(super::super::inspection::quiet(&quiet).is_err());
        fs::write(&path, b"{}\n").unwrap();
        assert!(resolve(backend, &run, "post-copy", &descriptor, None, true).is_err());
    }

    #[test]
    fn seed_registration_stays_independently_bound_to_its_run() {
        let root = tempfile::tempdir().unwrap();
        let backend = root.path();
        let run = backend.join(".local-tests/run");
        fs::create_dir_all(&run).unwrap();
        let path = run.join("seed-runtime.json");
        let request = json!({"identity_plan":{"path":"private-plan"}});
        json::write_new(&path, &request).unwrap();
        let descriptor = common::binding(&path).unwrap();
        assert_eq!(
            resolve(backend, &run, "seed-runtime", &descriptor, None, true).unwrap(),
            request
        );
        let other = run.join("other.json");
        fs::copy(&path, &other).unwrap();
        assert!(
            resolve(
                backend,
                &run,
                "seed-runtime",
                &common::binding(&other).unwrap(),
                None,
                true
            )
            .is_err()
        );
        fs::write(&path, b"{}\n").unwrap();
        assert!(resolve(backend, &run, "seed-runtime", &descriptor, None, true).is_err());
    }

    #[test]
    fn missing_launch_cannot_hide_abnormal_logs_from_inspection_or_quiet() {
        let root = tempfile::tempdir().unwrap();
        let backend = root.path();
        let run = backend.join(".local-tests/run");
        fs::create_dir_all(&run).unwrap();
        json::write_new(&run.join("manifest.json"), &json!({})).unwrap();
        state::initialize(&run).unwrap();
        let request = json!({"identity_plan":{"path":"private-plan"}});
        let path = run.join("seed-runtime.json");
        json::write_new(&path, &request).unwrap();
        let descriptor = common::binding(&path).unwrap();
        let (number, attempt) =
            state::begin(&run, "seed-runtime", "identities-apply", &json!({})).unwrap();
        let payload = json!({"backend":backend,"run_dir":run,"attempt":attempt,
            "registration":request,"registration_binding":descriptor});
        assert!(
            super::super::inspection::inspect(&payload)
                .unwrap()
                .is_null()
        );
        let output = run
            .join("seed-runtime")
            .join(format!("attempt-{number:04}"));
        fs::create_dir_all(&output).unwrap();
        fs::write(output.join("session.stderr.log"), "未知写入证据").unwrap();
        let error = super::super::inspection::inspect(&payload).unwrap_err();
        assert!(error.to_string().contains("缺少明确启动收据"));
        let registrations = json!({number.to_string():{"request":request,"binding":descriptor}});
        let error = super::super::inspection::quiet(&json!({"backend":backend,"run_dir":run,
            "current_attempt":null,"registrations":registrations}))
        .unwrap_err();
        assert!(error.to_string().contains("缺少明确启动收据"));
    }
}
