//! 后继 arm 只从已验证的共享导出与对应 fresh 工作区派生，预览不写入。
use super::{arm_target, output_value};
use crate::{
    Result,
    cli::FixtureSuccessorArm,
    local_test_path::LocalTestPathKind,
    recovery::{
        clone::native::{digest, input, json as document, lifecycle::manifest, resources::common},
        inputs::publish,
        runtime::dataset_authority::reads::Window,
        source::evidence::comparison::published,
    },
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const SUCCESSOR_KIND: &str = "devex-clone-seed-successor-arm-input";
const INDEPENDENT_B1_KIND: &str = "devex-clone-seed-independent-b1-arm-input";

#[derive(PartialEq)]
struct Observation {
    request: Value,
    export_identity: Value,
    source_state: Value,
    target_generation: Value,
    target_state: Value,
}

pub(in crate::recovery::fixture_control) fn run(
    backend: &Path,
    options: &FixtureSuccessorArm,
) -> Result<Value> {
    let output = options.output.as_deref();
    if let Some(output) = output {
        output_value(output, backend, "arm-request 输出")?;
    }
    let first = observe(backend, options)?;
    if let Some(output) = output {
        publish::publish_verified(output, &first.request, || {
            Ok(observe(backend, options)? == first)
        })?;
    }
    summary(&first.request, options.side.as_str(), output.is_some())
}

fn summary(request: &Value, side: &str, written: bool) -> Result<Value> {
    let status = match (request["kind"] == INDEPENDENT_B1_KIND, written) {
        (true, true) => "independent_b1_arm_request_published",
        (true, false) => "independent_b1_arm_request_planned",
        (false, true) => "successor_arm_request_published",
        (false, false) => "successor_arm_request_planned",
    };
    Ok(json!({
        "status":status,
        "request_sha256":digest::plan_hash(request)?,"side":side,"remote_writes":0,
    }))
}

fn observe(backend: &Path, options: &FixtureSuccessorArm) -> Result<Observation> {
    let window = Window::begin()?;
    window.finish(build(backend, options))
}

fn build(backend: &Path, options: &FixtureSuccessorArm) -> Result<Observation> {
    input::name(&options.id)?;
    let side = options.side.as_str();
    if !matches!(side, "base" | "candidate") {
        return Err("successor arm 只能选择 base 或 candidate".into());
    }
    if options.independent_b1 && side != "candidate" {
        return Err("独立 B1 ARM 只能选择 candidate".into());
    }
    let successor = file_binding(backend, &options.successor, "arm 审阅关系")?;
    let export = file_binding(backend, &options.source_export_result, "arm 共享导出结果")?;
    let source = published::verified(backend, &export, Some(&successor))?;
    let relation = &source.source["review_successor"];
    if source.source["seed_target"]["side"] != "seed"
        || relation["source_result"] != source.identity["source_registration"]
        || source.identity["source_rebind"].is_null()
        || source.identity["source_generation"].is_null()
    {
        return Err("successor arm 缺少原 seed、显式重绑定或已发布代次".into());
    }
    let bridges = document::array(&source.source["manifest"]["build_bridges"], "arm 构建桥接")?;
    let source_scope = &source.source["request"]["source"];
    let (target, review) = if options.independent_b1 {
        let (target, review) = arm_target::observe_independent_b1(
            backend,
            &options.workspace,
            &successor,
            source_scope,
            bridges,
            false,
        )?;
        (target, Some(review))
    } else {
        let target = arm_target::observe(
            backend,
            &options.workspace,
            side,
            relation,
            source_scope,
            bridges,
        )?;
        (target, None)
    };
    let copy = crate::local_test_path::validate_local_test_path(
        &options.copy_directory,
        backend,
        LocalTestPathKind::NewDirectory,
    )?;
    source.check(backend)?;
    let request = json!({
        "format_version":1, "kind":null,
        "source_registration":source.identity["source_registration"], "review_successor":successor,
        "source_export":source.identity["export"], "source_export_result":export,
        "source_rebind":source.identity["source_rebind"], "source_generation":source.identity["source_generation"],
        "id":options.id, "initialized":target.initialized, "target_registration":target.registration,
        "target_initialized_files":target.files, "target_environment":target.environment,
        "copy_directory":manifest::path_text(&copy.components().collect::<PathBuf>(), "arm 复制目录")?,
        "build_bridges":bridges,
    });
    Ok(Observation {
        request: request_with_mode(request, review),
        export_identity: source.identity,
        source_state: source.state,
        target_generation: target.generation,
        target_state: target.state,
    })
}

fn request_with_mode(mut request: Value, review: Option<arm_target::IndependentB1Review>) -> Value {
    let kind = match review {
        Some(review) => {
            request["independent_b1_review"] = review.binding;
            request["independent_b1_collision_projection"] = review.collision_projection;
            INDEPENDENT_B1_KIND
        }
        None => SUCCESSOR_KIND,
    };
    request["kind"] = json!(kind);
    request
}

pub(super) fn file_binding(backend: &Path, path: &Path, label: &str) -> Result<Value> {
    let path = common::common_file(backend, path, label)?
        .components()
        .collect::<PathBuf>();
    common::binding(&path)
}

#[cfg(test)]
#[test]
fn arm_request_summary_keeps_the_existing_safe_fields() {
    let request = json!({"kind":"devex-clone-seed-successor-arm-input","id":"arm-base"});
    let planned = summary(&request, "base", false).unwrap();
    assert_eq!(
        planned,
        json!({"status":"successor_arm_request_planned",
        "request_sha256":digest::plan_hash(&request).unwrap(),"side":"base","remote_writes":0})
    );
    let mut published = planned;
    published["status"] = json!("successor_arm_request_published");
    assert_eq!(summary(&request, "base", true).unwrap(), published);
}

#[cfg(test)]
#[test]
fn independent_b1_request_binds_review_and_original_evidence() {
    let common = json!({"format_version":1,"kind":null,"id":"arm-candidate",
        "source_export_result":{"path":"same-published-export"},
        "review_successor":{"path":"original-relationship"},
        "target_registration":{"path":"new-target"}});
    let mut expected = common.clone();
    expected["kind"] = json!(SUCCESSOR_KIND);
    assert_eq!(
        serde_json::to_vec(&request_with_mode(common.clone(), None)).unwrap(),
        serde_json::to_vec(&expected).unwrap()
    );

    let binding = json!({"path":"independent-review","bytes":20,"sha256":"a".repeat(64),
        "canonical_sha256":"b".repeat(64)});
    let projection =
        json!({"scopes":[{"owner":"measurement.candidate","identity":"candidate-scope"}]});
    expected["kind"] = json!(INDEPENDENT_B1_KIND);
    expected["independent_b1_review"] = binding.clone();
    expected["independent_b1_collision_projection"] = projection.clone();
    let request = request_with_mode(
        common,
        Some(arm_target::IndependentB1Review {
            binding,
            collision_projection: projection,
        }),
    );
    assert_eq!(request, expected);
    for (written, status) in [
        (false, "independent_b1_arm_request_planned"),
        (true, "independent_b1_arm_request_published"),
    ] {
        assert_eq!(
            summary(&request, "candidate", written).unwrap(),
            json!({
                "status":status,"request_sha256":digest::plan_hash(&request).unwrap(),
                "side":"candidate","remote_writes":0,
            })
        );
    }
}
