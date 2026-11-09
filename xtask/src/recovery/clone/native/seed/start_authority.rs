//! 新 seed 只在同一准备、原 API 退场与当前控制器完整核对后获得启动前像。

use std::path::Path;

use serde_json::{Value, json};

use super::{Current, Registration, history, latest, runtime_model};
use crate::{
    Result,
    recovery::{
        clone::native::{
            cache::handoff::registration, json as document, lifecycle::state,
            post_process::runtime, resources::common, target_verify::authority::require_closed,
        },
        full_stack::worker::{controller, role::Role},
    },
};

pub(super) fn verify(backend: &Path, run: &Path, number: u64) -> Result<Value> {
    let saved = state::load(run, true)?;
    let attempt = state::running_attempt(&saved)?.ok_or("seed 启动缺少当前运行阶段")?;
    if attempt["stage"] != "seed-runtime"
        || attempt["mode"] != "start"
        || attempt["number"].as_u64() != Some(number)
    {
        return Err("启动授权只属于当前 seed start 阶段".into());
    }
    let current = Current::load(backend, run)?;
    let registered = Registration::load(backend, run, false)?;
    let post = history::post(
        backend,
        run,
        Some(&registered.value["post_copy"]),
        None,
        false,
    )?;
    let inputs = runtime_model::inputs(backend, run)?;
    let handoff = common::binding(&inputs.runtime.join("handoff.json"))?;
    let proof = retirement(
        backend,
        run,
        &saved,
        number,
        &post,
        &registered.value,
        &handoff,
    )?;
    let process = common::read_bound(backend, &proof["api_process"], "seed 原 API 进程收据")?;
    let runtime = registration::cleanup_runtime(backend, run, &post)?;
    let product = runtime::product_root(backend, &runtime)?;
    let seed_runtime =
        common::read_bound(backend, &inputs.handoff["runtime"], "seed 固定运行合同")?;
    if runtime != seed_runtime
        || process["scope_id"] != runtime["scope_id"]
        || process["identity"]["executable"] != runtime["artifacts"]["api"]["path"]
    {
        return Err("seed 启动必须复用原 API 和 Worker 运行合同".into());
    }
    let identity = common::validate_process_identity(&process["identity"])?;
    require_pid_absent(&identity, &mut common::process_identity)?;
    let observed = controller::observe_bound(
        Role::Api,
        &product,
        &post.runtime,
        &post.request["api_environment"],
    )?;
    if observed != json!({"state":"stopped","identity":null}) {
        return Err("seed 启动前未取得原 API 完整监督树关闭证明".into());
    }
    require_closed(document::text(&inputs.handoff["api_url"], "seed API 地址")?)?;
    require_closed(document::text(
        &runtime["worker_ready_url"],
        "seed Worker 地址",
    )?)?;
    require_pid_absent(&identity, &mut common::process_identity)?;
    registered.assert_unchanged(backend, false)?;
    current.check(backend)?;
    if state::load(run, true)? != saved
        || retirement(
            backend,
            run,
            &saved,
            number,
            &post,
            &registered.value,
            &handoff,
        )? != proof
    {
        return Err("seed 启动授权期间原退场、登记或阶段证据变化".into());
    }
    Ok(
        json!({"handoff":handoff,"retirement":proof["stage"],"original_api":proof["api_process"],
        "original_tree":proof["api_tree"],"remote_writes":0}),
    )
}

fn retirement(
    backend: &Path,
    run: &Path,
    saved: &Value,
    current: u64,
    post: &history::Post,
    registered: &Value,
    handoff: &Value,
) -> Result<Value> {
    let prepare = latest(saved, "seed-runtime", &["prepare"], Some(current), false)?;
    let old = latest(
        saved,
        "runtime-target",
        &["start", "stop"],
        Some(current),
        false,
    )?;
    let prepared = common::read_bound(backend, &prepare["result"], "seed 启动准备结果")?;
    let stopped = common::read_bound(backend, &old["result"], "seed 原 API 退场结果")?;
    let evidence = &stopped["seed_retirement"];
    document::exact(
        evidence,
        &["prepare", "api_process", "api_tree"],
        "seed 原 API 退场绑定",
    )?;
    if old["mode"] != "stop"
        || old["number"].as_u64() <= prepare["number"].as_u64()
        || prepared["status"] != "seed_runtime_prepared"
        || prepared["handoff"] != *handoff
        || evidence["prepare"] != prepare["result"]
        || evidence["api_process"] != registered["api_process"]
        || stopped["format_version"] != 1
        || stopped["kind"] != "devex-clone-runtime-control"
        || stopped["operation"] != "stop"
        || stopped["runtime_directory"] != json!(post.runtime)
        || stopped["processes"] != json!({"api":{"state":"stopped","identity":null,"ready":false}})
        || stopped["producer_history"] != json!(post.runtime.join("producer-history.json"))
        || common::bound_file(backend, &evidence["api_process"], "原 API 进程")?
            != post.runtime.join("api.json")
        || common::bound_file(backend, &evidence["api_tree"], "原 API 监督树")?
            != post.runtime.join("api-tree.json")
    {
        return Err("seed 启动需要当前准备之后同一原 API 的完整退场结果".into());
    }
    let runtime = common::read_bound(backend, handoff, "seed 运行交接")?;
    if stopped["scope_id"] != runtime["scope_id"]
        || runtime["original_api"] != evidence["api_process"]
        || common::binding(&run.join("seed-runtime/runtime/handoff.json"))? != *handoff
    {
        return Err("seed 原 API 退场作用域或运行交接变化".into());
    }
    Ok(
        json!({"stage":old["result"],"api_process":evidence["api_process"],"api_tree":evidence["api_tree"]}),
    )
}

fn require_pid_absent(
    identity: &Value,
    inspect: &mut dyn FnMut(u32) -> Result<Option<Value>>,
) -> Result<()> {
    let pid = u32::try_from(identity["pid"].as_u64().ok_or("seed 原 API PID 无效")?)?;
    if inspect(pid)?.is_some() {
        return Err("原 API 尚未停止或 PID 已复用，不能启动新运行对".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recovery::clone::native::seed::fixture::Fixture;

    #[test]
    fn original_api_pid_must_be_absent_instead_of_reused() {
        let identity = json!({"pid":991,"started":"100"});
        for live in [identity.clone(), json!({"pid":991,"started":"200"})] {
            assert!(require_pid_absent(&identity, &mut |_| Ok(Some(live.clone()))).is_err());
        }
        assert!(require_pid_absent(&identity, &mut |_| Err("unknown".into())).is_err());
        assert!(
            require_pid_absent(&identity, &mut |pid| {
                assert_eq!(pid, 991);
                Ok(None)
            })
            .is_ok()
        );
    }

    #[test]
    fn retirement_must_follow_the_latest_prepare_and_match_both_original_files() {
        let mut fixture = Fixture::new();
        let old = fixture.old.clone();
        let registered = fixture.request.clone();
        let api = registered["api_process"].clone();
        let tree = fixture.file("unrelated-tree", &json!({"fixture":true}));
        let tree_path = old.join("api-tree.json");
        document::write_new(&tree_path, &json!({"original":true})).unwrap();
        let original_tree = common::binding(&tree_path).unwrap();
        let runtime = fixture.run.join("seed-runtime/runtime");
        std::fs::create_dir_all(&runtime).unwrap();
        let handoff = runtime.join("handoff.json");
        document::write_new(&handoff, &json!({"scope_id":"test","original_api":api})).unwrap();
        let handoff = common::binding(&handoff).unwrap();
        let post = history::Post {
            request: Value::Null,
            descriptor: Value::Null,
            runtime: old.clone(),
        };
        let prep = json!({"status":"seed_runtime_prepared","handoff":handoff});
        let prepared = fixture.stage("seed-runtime", "prepare", Some(prep.clone()), None);
        let mut stopped = json!({"format_version":1,"kind":"devex-clone-runtime-control","operation":"stop",
            "runtime_directory":old,"scope_id":"test","producer_history":old.join("producer-history.json"),
            "processes":{"api":{"state":"stopped","identity":null,"ready":false}},
            "seed_retirement":{"prepare":prepared,"api_process":api,"api_tree":tree}});
        fixture.stage("runtime-target", "stop", Some(stopped.clone()), None);
        let verify = |fixture: &Fixture| {
            retirement(
                &fixture.backend,
                &fixture.run,
                &state::load(&fixture.run, true).unwrap(),
                999,
                &post,
                &registered,
                &handoff,
            )
        };
        assert!(verify(&fixture).is_err());
        stopped["seed_retirement"]["api_tree"] = original_tree;
        let stop = fixture.stage("runtime-target", "stop", Some(stopped), None);
        assert_eq!(verify(&fixture).unwrap()["stage"], stop);
        fixture.stage("seed-runtime", "prepare", Some(prep), None);
        assert!(verify(&fixture).is_err());
    }
}
