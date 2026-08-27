use ryframe_generator::{AssetRoot, PlanAction, PlannedAsset, ResourceAssetPlan};

use super::resource::ensure_clean;

fn plan(action: PlanAction) -> ResourceAssetPlan {
    ResourceAssetPlan {
        assets: vec![PlannedAsset {
            resource: "post".into(),
            root: AssetRoot::Backend,
            path: "crates/ryframe-api/src/generated/post/mod.rs".into(),
            action,
            before: "旧内容\n".into(),
            after: "新内容\n".into(),
        }],
    }
}

#[test]
fn check_accepts_an_unchanged_plan() {
    ensure_clean(plan(PlanAction::Unchanged), "Post", "显式写入").expect("零差异应通过");
}

#[test]
fn check_rejects_drift_without_running_a_writer() {
    let error = ensure_clean(plan(PlanAction::Update), "Post", "显式写入")
        .expect_err("存在差异时应返回失败")
        .to_string();
    assert!(error.contains("生成结果与工作区不一致"));
    assert!(error.contains("显式写入"));
}
