use super::*;

const CURRENT: &str = include_str!("../../../../.github/workflows/extended-ci.yml");

#[test]
fn accepts_current_full_stack_workflow() {
    run(&crate::workspace::root_dir()).unwrap();
}

#[test]
fn accepts_folded_option_layout_and_additional_options() {
    let flag = r#"--output "$RUNNER_TEMP/ryframe-full-stack/source-pair.json""#;
    let changed = CURRENT.replacen(
        flag,
        "--output   \"$RUNNER_TEMP/ryframe-full-stack/source-pair.json\"\n          --future-option",
        1,
    );
    assert_ne!(changed, CURRENT);
    validate(&changed).unwrap();
}

#[test]
fn rejects_drift_in_source_pair_and_stage_ownership() {
    for (old, new, expected) in [
        (
            "--input \"$RUNNER_TEMP",
            "--input \"$UNREGISTERED",
            "verify-pair",
        ),
        (
            "--output \"$RUNNER_TEMP",
            "--unexpected \"$RUNNER_TEMP",
            "record-pair",
        ),
        (
            "--frontend-dir \"$GITHUB_WORKSPACE/frontend\"",
            "--frontend-dir \"$GITHUB_WORKSPACE/other\"",
            "record-pair",
        ),
        (
            "--input \"$RUNNER_TEMP/ryframe-full-stack/source-pair.json\"",
            "--input \"$RUNNER_TEMP/ryframe-full-stack/source-pair.json\" --input=other",
            "verify-pair",
        ),
        (
            "business-fixture\" --write",
            "business-fixture\" --write --write",
            "业务夹具",
        ),
        (
            "cargo xtask check recovery fixture\n",
            "cargo xtask check recovery fixture-review\n",
            "缺少全栈步骤",
        ),
        (
            "working-directory: ${{ matrix.backend }}",
            "working-directory: backend",
            "full-stack-config",
        ),
        (
            "--target-dir \"$RUNNER_TEMP/ryframe-full-stack-tool\"",
            "--target-dir target/xtask-run",
            "cargo build --locked",
        ),
        (
            "/debug/xtask\" check recovery full-stack collect",
            "/release/xtask\" check recovery full-stack collect",
            "full-stack collect",
        ),
        (
            "full-stack prepare'",
            "full-stack prepare && cargo xtask check recovery full-stack prepare'",
            "的 run",
        ),
        (
            "- name: 启动真实 API、Worker 并等待就绪\n",
            "- name: 启动真实 API、Worker 并等待就绪\n        if: ${{ false }}\n",
            "无条件",
        ),
        (
            "收集进程与基础设施日志\n        if: ${{ always() }}",
            "收集进程与基础设施日志\n        if: ${{ success() }}",
            "full-stack collect 的 if",
        ),
        (
            "working-directory: ${{ matrix.frontend }}",
            "working-directory: frontend",
            "前端生产构建",
        ),
        (
            "- name: 构建前端生产产物\n",
            "- name: 构建前端生产产物\n        if: ${{ false }}\n",
            "无条件",
        ),
        (
            "corepack pnpm build --source-commit",
            "corepack pnpm build && corepack pnpm check --source-commit",
            "前端生产构建",
        ),
    ] {
        assert_rejected(CURRENT.replacen(old, new, 1), expected, old);
    }
}

#[test]
fn rejects_source_verification_before_frontend_build() {
    let start = CURRENT
        .find("      - name: 复核全栈源码组合未变化")
        .unwrap();
    let end = CURRENT
        .find("      - name: 收集进程与基础设施日志")
        .unwrap();
    let build = CURRENT.find("      - name: 构建前端生产产物").unwrap();
    let changed = format!(
        "{}{}{}{}",
        &CURRENT[..build],
        &CURRENT[start..end],
        &CURRENT[build..start],
        &CURRENT[end..],
    );
    assert_rejected(changed, "步骤顺序", "前端构建前复核来源");
}

#[test]
fn rejects_fixture_generation_before_source_record() {
    let record = CURRENT.find("      - name: 记录本次全栈源码组合").unwrap();
    let fixture = CURRENT
        .find("      - name: 生成业务 crate fixture")
        .unwrap();
    let end = CURRENT
        .find("      - name: 输出业务 crate fixture 失败诊断")
        .unwrap();
    let changed = format!(
        "{}{}{}{}",
        &CURRENT[..record],
        &CURRENT[fixture..end],
        &CURRENT[record..fixture],
        &CURRENT[end..],
    );
    assert_rejected(changed, "步骤顺序", "记录来源前生成夹具");
}

#[test]
fn rejects_collection_before_source_verification() {
    let verify = CURRENT
        .find("      - name: 复核全栈源码组合未变化")
        .unwrap();
    let collect = CURRENT
        .find("      - name: 收集进程与基础设施日志")
        .unwrap();
    let upload = CURRENT.find("      - name: 保存全栈诊断产物").unwrap();
    let changed = format!(
        "{}{}{}{}",
        &CURRENT[..verify],
        &CURRENT[collect..upload],
        &CURRENT[verify..collect],
        &CURRENT[upload..],
    );
    assert_rejected(changed, "步骤顺序", "来源复核前收集");
}

#[test]
fn rejects_missing_attempt_matrix_or_public_diagnostics_binding() {
    let start = CURRENT.find("      - name: 保存全栈诊断产物").unwrap();
    let end = CURRENT[start + 1..].find("\n      - name: ").unwrap() + start + 1;
    let upload = &CURRENT[start..end];
    for (old, new, expected) in [
        (
            "-${{ github.run_attempt }}${{ matrix.artifact_suffix }}",
            "${{ matrix.artifact_suffix }}",
            "github.run_attempt",
        ),
        (
            "include-hidden-files: true",
            "include-hidden-files: false",
            "包含隐藏文件",
        ),
        (
            "business-fixture/fixture.json",
            "business-fixture/other.json",
            "公开收据",
        ),
        (
            "business-fixture/prepare.log",
            "business-fixture/playwright-real",
            "playwright-real",
        ),
        ("${{ always() }}", "${{ success() }}", "${{ always() }}"),
    ] {
        assert_eq!(upload.matches(old).count(), 1, "{old}");
        let changed = format!(
            "{}{}{}",
            &CURRENT[..start],
            upload.replacen(old, new, 1),
            &CURRENT[end..]
        );
        assert_rejected(changed, expected, old);
    }
    assert_rejected(
        CURRENT.replacen("artifact_suffix: -business", "artifact_suffix: \"\"", 1),
        "business 的 artifact_suffix",
        "business 矩阵产物后缀",
    );
}

fn assert_rejected(changed: String, expected: &str, mutation: &str) {
    assert_ne!(changed, CURRENT, "变更未命中：{mutation}");
    let error = validate(&changed).expect_err(mutation);
    assert!(error.contains(expected), "{mutation}：{error}");
}
