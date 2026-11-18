use super::*;

const CONTROL: &str = "shared-control";
const SCHEDULE: &str = "sys_job_schedule";
const JOB: &str = "sys_background_job";
const EXECUTION: &str = "sys_job_schedule_execution";
const SQL_TIME: &str = "2026-09-01 00:00:00.123456";
const JSON_TIME: &str = "2026-09-01T00:00:00.123456Z";

const CLEANUP_JOBS: [(&str, &str); 3] = [
    ("system.export_result_cleanup", "system.export.cleanup"),
    (
        "system.message_retention_cleanup",
        "system.message.retention",
    ),
    (
        "system.data_retention_cleanup",
        "system.data_retention.cleanup",
    ),
];

const SCOPE_ERROR: &str = "全局调度父任务仅允许精确 system 维护目标及其登记 job type";
const NUMBER_ERROR: &str = "平台维护关联必须使用明确整数 ID";
const RANGE_ERROR: &str = "平台维护关联 ID 超出正整数范围";
const PARENT_ERROR: &str = "调度历史缺少父任务证据";
const TIME_ERROR: &str = "平台维护关联时间必须是完整 UTC 微秒时间";
const TIME_MISMATCH: &str = "平台维护父任务的计划时间或完成时间不匹配";
const PAYLOAD_ERROR: &str = "平台维护父任务必须保留当前受控载荷及精确触发时间";

fn payload(schedule_id: i64, trigger: &str) -> Value {
    json!({"schedule_id":schedule_id.to_string(),"trigger_kind":trigger,"scheduled_for":JSON_TIME})
}

fn cleanup(fixture: &mut Fixture, index: usize) {
    let (handler, job_type) = CLEANUP_JOBS[index];
    let schedule_id = 101 + index as i64;
    let job_id = 99_256_017_304_428_544_i64 + index as i64;
    fixture.add(
        CONTROL,
        SCHEDULE,
        &[
            ("id", json!(schedule_id)),
            ("tenant_id", json!("system")),
            ("handler_key", json!(handler)),
            ("enabled", json!(0)),
            ("del_flag", json!("0")),
            ("version", json!(1)),
            ("created_at", json!(SQL_TIME)),
            ("updated_at", json!(SQL_TIME)),
        ],
    );
    fixture.add(
        CONTROL,
        JOB,
        &[
            ("id", json!(job_id)),
            ("tenant_id", Value::Null),
            ("job_type", json!(job_type)),
            ("status", json!("succeeded")),
            ("completed_at", json!("2026-09-01 00:00:01.000001")),
            ("schedule_id", json!(schedule_id)),
            ("scheduled_for", json!(SQL_TIME)),
            ("payload", payload(schedule_id, "scheduled")),
        ],
    );
    fixture.add(
        CONTROL,
        EXECUTION,
        &[
            ("id", json!(99_256_017_220_542_464_i64 + index as i64)),
            ("tenant_id", json!("system")),
            ("schedule_id", json!(schedule_id)),
            ("background_job_id", json!(job_id)),
            ("outcome", json!("enqueued")),
            ("trigger_kind", json!("scheduled")),
            ("scheduled_for", json!(SQL_TIME)),
        ],
    );
}

fn enable_with_action(row: &mut TestRow) -> Value {
    row.insert("enabled".into(), json!(1));
    row.insert("next_run_at".into(), json!("2026-09-02 00:00:00"));
    let values = row.values().map(sql_literal).collect::<Vec<_>>().join(",");
    let parsed = row
        .keys()
        .cloned()
        .zip(native::sql::literals(&values).unwrap())
        .collect();
    json!({
        "action":"disable_schedule_via_api",
        "tenant_id":row["tenant_id"],
        "schedule_id":row["id"].to_string(),
        "handler_key":row["handler_key"],
        "expected_version":row["version"],
        "source_row_sha256":native::schedule::row_sha256(&parsed).unwrap(),
    })
}

fn assert_rejected(fixture: &mut Fixture, label: &str, reason: &str) {
    fixture.refresh();
    let output = "cleanup-rejected.json";
    let error = fixture.plan(output).unwrap_err();
    assert!(error.contains(reason), "{label}: {error}");
    assert!(
        !fixture
            .root
            .join(".local-tests")
            .join(output)
            .try_exists()
            .unwrap()
    );
}

fn reject_changes(
    fixture: &mut Fixture,
    cases: impl IntoIterator<Item = (&'static str, &'static str, Value, &'static str)>,
) {
    let original = fixture.data.clone();
    for (index, (table, field, value, reason)) in cases.into_iter().enumerate() {
        set_row(fixture, CONTROL, table, field, value);
        assert_rejected(fixture, &format!("{table}-{field}-{index}"), reason);
        fixture.data.clone_from(&original);
    }
}

#[test]
fn global_cleanup_schedule_relations_use_product_job_types_and_preserve_boundaries() {
    let mut fixture = Fixture::new();
    for index in 0..CLEANUP_JOBS.len() {
        cleanup(&mut fixture, index);
    }
    let actions = fixture
        .data
        .get_mut(CONTROL)
        .unwrap()
        .iter_mut()
        .filter(|(table, _)| table == SCHEDULE)
        .map(|(_, row)| enable_with_action(row))
        .collect();
    fixture.value["target_schedule_actions"] = Value::Array(actions);
    fixture.refresh();
    let before = snapshot(&fixture.export);
    let plan = fixture.plan("cleanup-pending.json").unwrap();
    let plan: Value = serde_json::from_slice(&fs::read(plan).unwrap()).unwrap();
    let plan = &plan["plan"];
    assert_eq!(plan["status"], "offline_verified_pending_target_actions");
    assert_eq!(plan["execution_authorized"], false);
    let actions = plan["pending_target_actions"].as_array().unwrap();
    assert_eq!(actions.len(), CLEANUP_JOBS.len());
    assert!(
        actions
            .iter()
            .all(|action| action["api_called"].as_bool() == Some(false))
    );
    assert_eq!(snapshot(&fixture.export), before);
}

#[test]
fn cleanup_accepts_all_triggers_and_both_terminal_outcomes() {
    let mut fixture = Fixture::new();
    cleanup(&mut fixture, 0);
    for trigger in ["scheduled", "manual", "misfire"] {
        for terminal in ["succeeded", "dead"] {
            set_row(
                &mut fixture,
                CONTROL,
                EXECUTION,
                "trigger_kind",
                json!(trigger),
            );
            set_row(&mut fixture, CONTROL, JOB, "payload", payload(101, trigger));
            set_row(&mut fixture, CONTROL, JOB, "status", json!(terminal));
            fixture.refresh();
            fixture
                .plan(&format!("cleanup-{trigger}-{terminal}.json"))
                .unwrap();
        }
    }
}

#[test]
fn cleanup_rejects_wrong_scope_handler_and_parent_ids() {
    let mut fixture = Fixture::new();
    cleanup(&mut fixture, 0);
    reject_changes(
        &mut fixture,
        [
            (SCHEDULE, "tenant_id", json!("clone-source-01"), SCOPE_ERROR),
            (
                EXECUTION,
                "tenant_id",
                json!("clone-source-01"),
                SCOPE_ERROR,
            ),
            (JOB, "tenant_id", json!("system"), SCOPE_ERROR),
            (JOB, "tenant_id", json!("clone-source-01"), SCOPE_ERROR),
            (
                SCHEDULE,
                "handler_key",
                json!("system.unknown_cleanup"),
                SCOPE_ERROR,
            ),
            (
                JOB,
                "job_type",
                json!("system.message.retention"),
                SCOPE_ERROR,
            ),
            (JOB, "job_type", json!("unknown"), SCOPE_ERROR),
            (JOB, "schedule_id", Value::Null, NUMBER_ERROR),
            (
                JOB,
                "schedule_id",
                json!(999),
                "平台维护 schedule、execution 与 parent ID 不一致",
            ),
            (
                EXECUTION,
                "schedule_id",
                Value::Null,
                "复制行 schedule_id 必须为整数标识",
            ),
            (EXECUTION, "schedule_id", json!(999), SCOPE_ERROR),
            (EXECUTION, "background_job_id", json!(999), PARENT_ERROR),
            (SCHEDULE, "id", json!(999), SCOPE_ERROR),
        ],
    );
    set_row(
        &mut fixture,
        CONTROL,
        SCHEDULE,
        "handler_key",
        json!("custom.cleanup"),
    );
    set_row(
        &mut fixture,
        CONTROL,
        JOB,
        "job_type",
        json!("custom.cleanup"),
    );
    set_row(&mut fixture, CONTROL, EXECUTION, "tenant_id", Value::Null);
    assert_rejected(&mut fixture, "unreviewed-null-parent", SCOPE_ERROR);

    let mut retention = Fixture::new();
    cleanup(&mut retention, 2);
    set_row(
        &mut retention,
        CONTROL,
        JOB,
        "job_type",
        json!("system.data.retention.cleanup"),
    );
    assert_rejected(&mut retention, "obsolete-retention-job-type", SCOPE_ERROR);
}

#[test]
fn cleanup_rejects_invalid_sql_and_payload_times() {
    let mut fixture = Fixture::new();
    cleanup(&mut fixture, 0);
    reject_changes(
        &mut fixture,
        [
            (JOB, "scheduled_for", Value::Null, TIME_ERROR),
            (
                JOB,
                "scheduled_for",
                json!("2026-09-01 00:00:00.123457"),
                TIME_MISMATCH,
            ),
            (EXECUTION, "scheduled_for", json!(JSON_TIME), TIME_ERROR),
            (
                JOB,
                "completed_at",
                json!("2026-09-01 00:00:00.123455"),
                TIME_MISMATCH,
            ),
        ],
    );
    let original = fixture.data.clone();
    for (index, time) in [
        SQL_TIME,
        "2026-09-01T00:00:00.123457Z",
        "2026-09-01T00:00:00.1234561Z",
        "2026-09-01T08:00:00.123456+08:00",
        "2026-02-31T00:00:00Z",
    ]
    .into_iter()
    .enumerate()
    {
        let mut payload = payload(101, "scheduled");
        payload["scheduled_for"] = json!(time);
        set_row(&mut fixture, CONTROL, JOB, "payload", payload);
        let reason = if index == 1 {
            PAYLOAD_ERROR
        } else {
            TIME_ERROR
        };
        assert_rejected(&mut fixture, &format!("payload-time-{index}"), reason);
        fixture.data.clone_from(&original);
    }
}

#[test]
fn cleanup_rejects_incomplete_or_mismatched_payload_and_unreviewed_trigger() {
    let mut fixture = Fixture::new();
    cleanup(&mut fixture, 0);
    let mut extra = payload(101, "scheduled");
    extra["unreviewed"] = json!("extra");
    let cases = [
        Value::Null,
        json!([]),
        json!({"schedule_id":"101","trigger_kind":"scheduled"}),
        json!({"schedule_id":101,"trigger_kind":"scheduled","scheduled_for":JSON_TIME}),
        payload(102, "scheduled"),
        payload(101, "manual"),
        extra,
    ];
    reject_changes(
        &mut fixture,
        cases.map(|value| (JOB, "payload", value, PAYLOAD_ERROR)),
    );
    set_row(
        &mut fixture,
        CONTROL,
        EXECUTION,
        "trigger_kind",
        json!("unknown"),
    );
    set_row(
        &mut fixture,
        CONTROL,
        JOB,
        "payload",
        payload(101, "unknown"),
    );
    assert_rejected(&mut fixture, "unreviewed-trigger", PAYLOAD_ERROR);
}

#[test]
fn cleanup_rejects_nonterminal_leased_or_missing_parent_job() {
    let mut fixture = Fixture::new();
    cleanup(&mut fixture, 0);
    reject_changes(
        &mut fixture,
        [
            (
                JOB,
                "status",
                json!("pending"),
                "来源存在未解决的可执行任务",
            ),
            (
                JOB,
                "status",
                json!("running"),
                "来源存在未解决的可执行任务",
            ),
            (JOB, "status", json!("failed"), "来源存在未解决的可执行任务"),
            (JOB, "completed_at", Value::Null, "终态任务缺少完成时间"),
            (JOB, "lease_owner", json!("worker"), "终态任务仍携带租约"),
            (JOB, "lease_until", json!(SQL_TIME), "终态任务仍携带租约"),
            (
                EXECUTION,
                "background_job_id",
                Value::Null,
                "调度触发历史与后台任务关联不一致",
            ),
        ],
    );
    fixture
        .data
        .get_mut(CONTROL)
        .unwrap()
        .retain(|(table, _)| table != JOB);
    assert_rejected(&mut fixture, "missing-parent", PARENT_ERROR);
}

#[test]
fn cleanup_requires_a_positive_sql_i64_identifier() {
    let mut fixture = Fixture::new();
    cleanup(&mut fixture, 0);
    reject_changes(
        &mut fixture,
        [
            (JOB, "schedule_id", json!("101"), NUMBER_ERROR),
            (JOB, "schedule_id", json!(0), RANGE_ERROR),
            (JOB, "schedule_id", json!(-1), RANGE_ERROR),
            (JOB, "schedule_id", json!(1_u64 << 63), RANGE_ERROR),
            (JOB, "schedule_id", json!(1.5), RANGE_ERROR),
        ],
    );
    // SQL 字面量不把布尔值或非有限值读成整数。
    for invalid in ["TRUE", "NaN", "Infinity"] {
        let error = native::sql::literals(invalid).unwrap_err();
        assert!(
            error.to_string().contains("SQL 不是受限字面量"),
            "{invalid}: {error}"
        );
    }
}
