use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use ryframe_application::{
    TenantContext,
    ports::export::{PostExportReadFilter, PostExportReadPort, PostExportRow},
    system::PostExportService,
    with_tenant_context,
};
use ryframe_kernel::{ActorContext, AppResult, DataScope, ExportCursorWindow};

#[derive(Clone, Debug, Eq, PartialEq)]
struct ReadCall {
    tenant_id: String,
    name: Option<String>,
    code: Option<String>,
    status: Option<String>,
    window: ExportCursorWindow,
}

struct FakePostExportRead {
    calls: Mutex<Vec<ReadCall>>,
    rows: Vec<PostExportRow>,
}

#[async_trait]
impl PostExportReadPort for FakePostExportRead {
    async fn find_batch(
        &self,
        tenant_id: &str,
        filter: PostExportReadFilter<'_>,
        window: ExportCursorWindow,
    ) -> AppResult<Vec<PostExportRow>> {
        self.calls
            .lock()
            .expect("岗位导出调用记录锁应可用")
            .push(ReadCall {
                tenant_id: tenant_id.to_owned(),
                name: filter.name.map(str::to_owned),
                code: filter.code.map(str::to_owned),
                status: filter.status.map(str::to_owned),
                window,
            });
        Ok(self.rows.clone())
    }
}

fn actor() -> ActorContext {
    ActorContext {
        user_id: 7,
        tenant_id: "tenant-a".into(),
        username: "tester".into(),
        dept_id: None,
        dept_path: None,
        data_scope: DataScope::All,
        custom_dept_ids: Vec::new(),
        include_self: true,
        is_super_admin: false,
    }
}

fn row() -> PostExportRow {
    PostExportRow {
        id: "9007199254740993".into(),
        name: "平台岗位".into(),
        code: "platform".into(),
        sort: 1,
        status: "1".into(),
        remark: Some("导出测试".into()),
        created_at: Utc
            .with_ymd_and_hms(2026, 8, 23, 0, 0, 0)
            .single()
            .expect("测试时间应有效"),
    }
}

#[tokio::test]
async fn service_forwards_tenant_filters_and_cursor_without_losing_string_id() {
    let expected = row();
    let read = Arc::new(FakePostExportRead {
        calls: Mutex::new(Vec::new()),
        rows: vec![expected.clone()],
    });
    let service = PostExportService::new(read.clone());
    let window = ExportCursorWindow::new(Some(10), 99, 20);

    let rows = service
        .find_batch(&actor(), Some("平台"), Some("platform"), Some("1"), window)
        .await
        .expect("岗位导出批次应读取成功");

    assert_eq!(rows, [expected]);
    assert_eq!(
        *read.calls.lock().expect("岗位导出调用记录锁应可用"),
        [ReadCall {
            tenant_id: "tenant-a".into(),
            name: Some("平台".into()),
            code: Some("platform".into()),
            status: Some("1".into()),
            window,
        }]
    );
}

#[tokio::test]
async fn service_rejects_request_tenant_mismatch_before_reading() {
    let read = Arc::new(FakePostExportRead {
        calls: Mutex::new(Vec::new()),
        rows: Vec::new(),
    });
    let service = PostExportService::new(read.clone());
    let result = with_tenant_context(
        TenantContext {
            tenant_id: "tenant-b".into(),
            is_admin: false,
        },
        service.find_batch(
            &actor(),
            None,
            None,
            None,
            ExportCursorWindow::new(None, 99, 20),
        ),
    )
    .await;

    assert!(result.is_err());
    assert!(
        read.calls
            .lock()
            .expect("岗位导出调用记录锁应可用")
            .is_empty()
    );
}
