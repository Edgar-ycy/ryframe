mod common;

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{
        HeaderMap, Method, StatusCode,
        header::{CONNECTION, CONTENT_TYPE, TRANSFER_ENCODING},
    },
    response::Response,
};
use chrono::{DateTime, Utc};
use ryframe_config::JobConfig;
use ryframe_db::{
    DatabaseCluster,
    entities::{background_job, export_job, role, sys_file, user, user_role},
};
use ryframe_excel::ExcelImporter;
use ryframe_kernel::{ActorContext, DataScope, ErrorCode};
use ryframe_service::{
    ExportJobHandler, JobQueue, JobRunResult, JobWorker,
    system::{EXPORT_BUCKET, ExportService, RequestExportCommand, UserExportFilters, UserService},
};
use ryframe_storage::{ObjectStorage, S3Config, S3ObjectStorage};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter,
    Schema, Set,
};
use serde::Deserialize;
use tokio::{
    sync::{Notify, oneshot},
    task::{JoinError, JoinHandle},
};

const SEEDED_USER_COUNT: u64 = 100_000;
const PROXY_BODY_LIMIT: usize = 512 * 1024 * 1024;
const ACCEPTANCE_TIMEOUT: Duration = Duration::from_secs(180);
const WORKER_PHASE_TIMEOUT: Duration = Duration::from_secs(120);
const PUT_PAUSE_TIMEOUT: Duration = Duration::from_secs(150);
const LEASE_RECOVERY_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Deserialize)]
struct ImportedUserRow {
    #[serde(rename = "用户 ID")]
    user_id: String,
    #[serde(rename = "用户名")]
    username: String,
}

#[derive(Default)]
struct ProxyFaults {
    pause_next_object_put: AtomicBool,
    paused_object_put_seen: Notify,
    resume_paused_object_put: Notify,
    fail_next_object_put: AtomicBool,
    fail_next_object_delete: AtomicBool,
    injected_put_failures: AtomicUsize,
    injected_delete_failures: AtomicUsize,
    forwarded_object_puts: AtomicUsize,
}

#[derive(Clone)]
struct ProxyState {
    upstream: Arc<str>,
    client: reqwest::Client,
    faults: Arc<ProxyFaults>,
}

struct ControllableS3Proxy {
    endpoint: String,
    faults: Arc<ProxyFaults>,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

struct AbortTaskOnDrop<T> {
    task: Option<JoinHandle<T>>,
}

impl<T> AbortTaskOnDrop<T> {
    fn new(task: JoinHandle<T>) -> Self {
        Self { task: Some(task) }
    }

    fn task_mut(&mut self) -> &mut JoinHandle<T> {
        self.task.as_mut().expect("后台任务句柄存在")
    }

    async fn abort_and_join(&mut self) -> Result<T, JoinError> {
        let task = self.task.take().expect("后台任务句柄存在");
        task.abort();
        task.await
    }
}

impl<T> Drop for AbortTaskOnDrop<T> {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl ControllableS3Proxy {
    async fn start(upstream: String) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定对象存储故障代理端口");
        let address = listener.local_addr().expect("读取对象存储故障代理端口");
        let faults = Arc::new(ProxyFaults::default());
        let state = ProxyState {
            upstream: Arc::from(upstream.trim_end_matches('/')),
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(180))
                .build()
                .expect("创建对象存储代理客户端"),
            faults: faults.clone(),
        };
        let router = Router::new().fallback(proxy_s3_request).with_state(state);
        let (shutdown, receiver) = oneshot::channel();
        let task = tokio::spawn(async move {
            let result = axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = receiver.await;
                })
                .await;
            assert!(result.is_ok(), "对象存储故障代理异常退出: {result:?}");
        });

        Self {
            endpoint: format!("http://{address}"),
            faults,
            shutdown: Some(shutdown),
            task: Some(task),
        }
    }

    fn endpoint(&self) -> &str {
        &self.endpoint
    }

    fn pause_next_object_put(&self) {
        assert!(
            !self
                .faults
                .pause_next_object_put
                .swap(true, Ordering::SeqCst),
            "已有一个待暂停的对象 PUT"
        );
    }

    async fn wait_until_object_put_is_paused(&self, worker_task: &mut JoinHandle<JobRunResult>) {
        tokio::select! {
            result = worker_task => {
                panic!("Worker A 在对象 PUT 暂停前提前结束: {result:?}");
            }
            _ = self.faults.paused_object_put_seen.notified() => {}
            _ = tokio::time::sleep(PUT_PAUSE_TIMEOUT) => {
                panic!("等待十万行导出到达对象存储代理超时");
            }
        }
    }

    fn resume_paused_object_put(&self) {
        self.faults.resume_paused_object_put.notify_one();
    }

    fn fail_next_object_put(&self) {
        assert!(
            !self
                .faults
                .fail_next_object_put
                .swap(true, Ordering::SeqCst),
            "已有一个待失败的对象 PUT"
        );
    }

    fn fail_next_object_delete(&self) {
        assert!(
            !self
                .faults
                .fail_next_object_delete
                .swap(true, Ordering::SeqCst),
            "已有一个待失败的对象 DELETE"
        );
    }

    fn injected_put_failures(&self) -> usize {
        self.faults.injected_put_failures.load(Ordering::SeqCst)
    }

    fn injected_delete_failures(&self) -> usize {
        self.faults.injected_delete_failures.load(Ordering::SeqCst)
    }

    fn forwarded_object_puts(&self) -> usize {
        self.faults.forwarded_object_puts.load(Ordering::SeqCst)
    }

    async fn shutdown(mut self) {
        self.faults.resume_paused_object_put.notify_waiters();
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        let mut task = self.task.take().expect("对象存储故障代理任务存在");
        match tokio::time::timeout(Duration::from_secs(5), &mut task).await {
            Ok(result) => result.expect("对象存储故障代理任务异常"),
            Err(_) => {
                task.abort();
                let _ = task.await;
                panic!("关闭对象存储故障代理超时，已强制中止");
            }
        }
    }
}

impl Drop for ControllableS3Proxy {
    fn drop(&mut self) {
        self.faults.resume_paused_object_put.notify_waiters();
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

async fn proxy_s3_request(State(state): State<ProxyState>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let request_body = match to_bytes(body, PROXY_BODY_LIMIT).await {
        Ok(body) => body,
        Err(error) => {
            return proxy_error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("对象存储代理读取请求体失败: {error}"),
            );
        }
    };
    let is_object_path = parts.uri.path().starts_with(&format!("/{EXPORT_BUCKET}/"));
    let is_object_put = parts.method == Method::PUT && is_object_path;
    let is_object_delete = parts.method == Method::DELETE && is_object_path;

    if is_object_put {
        if state
            .faults
            .pause_next_object_put
            .swap(false, Ordering::SeqCst)
        {
            state.faults.paused_object_put_seen.notify_one();
            state.faults.resume_paused_object_put.notified().await;
            state
                .faults
                .injected_put_failures
                .fetch_add(1, Ordering::SeqCst);
            return injected_service_unavailable("已暂停的对象 PUT 由代理返回 503");
        }
        if state
            .faults
            .fail_next_object_put
            .swap(false, Ordering::SeqCst)
        {
            state
                .faults
                .injected_put_failures
                .fetch_add(1, Ordering::SeqCst);
            return injected_service_unavailable("对象 PUT 首次故障注入");
        }
        state
            .faults
            .forwarded_object_puts
            .fetch_add(1, Ordering::SeqCst);
    }
    if is_object_delete
        && state
            .faults
            .fail_next_object_delete
            .swap(false, Ordering::SeqCst)
    {
        state
            .faults
            .injected_delete_failures
            .fetch_add(1, Ordering::SeqCst);
        return injected_service_unavailable("对象 DELETE 首次故障注入");
    }

    let suffix = parts.uri.path_and_query().map_or("/", |path| path.as_str());
    let target = format!("{}{suffix}", state.upstream);
    let headers = forward_headers(&parts.headers);
    let upstream = match state
        .client
        .request(parts.method, target)
        // Host 是 SigV4 签名的一部分，转发时必须保留代理收到的原值。
        .headers(headers)
        .body(request_body)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            return proxy_error_response(
                StatusCode::BAD_GATEWAY,
                format!("对象存储代理转发失败: {error}"),
            );
        }
    };
    let status = upstream.status();
    let headers = forward_headers(upstream.headers());
    let response_body = match upstream.bytes().await {
        Ok(body) => body,
        Err(error) => {
            return proxy_error_response(
                StatusCode::BAD_GATEWAY,
                format!("对象存储代理读取响应失败: {error}"),
            );
        }
    };
    let mut response = Response::builder().status(status);
    if let Some(response_headers) = response.headers_mut() {
        response_headers.extend(headers);
    }
    response
        .body(Body::from(response_body))
        .expect("构造对象存储代理响应")
}

fn forward_headers(source: &HeaderMap) -> HeaderMap {
    source
        .iter()
        .filter(|(name, _)| *name != CONNECTION && *name != TRANSFER_ENCODING)
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

fn injected_service_unavailable(message: &str) -> Response {
    proxy_error_response(StatusCode::SERVICE_UNAVAILABLE, message.to_owned())
}

fn proxy_error_response(status: StatusCode, message: String) -> Response {
    Response::builder()
        .status(status)
        .header(CONNECTION, "close")
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(message))
        .expect("构造对象存储代理错误响应")
}

fn actor() -> ActorContext {
    ActorContext {
        user_id: 1,
        tenant_id: "system".into(),
        username: "admin".into(),
        dept_id: None,
        dept_path: None,
        data_scope: DataScope::All,
        custom_dept_ids: Vec::new(),
        include_self: true,
        is_super_admin: true,
    }
}

fn acceptance_job_config() -> JobConfig {
    JobConfig {
        poll_interval_ms: 50,
        lease_seconds: 30,
        heartbeat_seconds: 5,
        default_max_attempts: 5,
        export_max_rows: SEEDED_USER_COUNT as usize,
        concurrency: 1,
        ..JobConfig::default()
    }
}

fn rustfs_endpoint() -> String {
    let endpoint = std::env::var("RYFRAME_TEST_RUSTFS_ENDPOINT").unwrap_or_else(|_| {
        let port = std::env::var("RYFRAME_TEST_RUSTFS_PORT")
            .map_or_else(
                |_| Ok(19_000),
                |value| {
                    value
                        .parse::<u16>()
                        .ok()
                        .filter(|port| *port > 0)
                        .ok_or(value)
                },
            )
            .unwrap_or_else(|value| panic!("RYFRAME_TEST_RUSTFS_PORT 无效: {value}"));
        format!("http://127.0.0.1:{port}")
    });
    validate_rustfs_endpoint(&endpoint).unwrap_or_else(|error| panic!("{error}"));
    endpoint
}

fn validate_rustfs_endpoint(endpoint: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(endpoint)
        .map_err(|error| format!("RYFRAME_TEST_RUSTFS_ENDPOINT 无效: {error}"))?;
    let port = url
        .port()
        .filter(|port| *port > 0)
        .ok_or_else(|| "RYFRAME_TEST_RUSTFS_ENDPOINT 必须显式包含有效端口".to_owned())?;
    let expected = format!("http://127.0.0.1:{port}");
    if endpoint != expected {
        return Err(format!(
            "RYFRAME_TEST_RUSTFS_ENDPOINT 必须严格使用 {expected} 形式，不允许外部地址、localhost、HTTPS、路径或查询参数"
        ));
    }
    Ok(())
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_owned())
}

#[test]
fn rustfs_endpoint_requires_exact_loopback_http_origin() {
    assert!(validate_rustfs_endpoint("http://127.0.0.1:19000").is_ok());
    for endpoint in [
        "https://127.0.0.1:19000",
        "http://localhost:19000",
        "http://192.168.1.10:19000",
        "http://127.0.0.1:19000/",
        "http://127.0.0.1",
        "http://127.0.0.1:19000/path",
        "http://127.0.0.1:19000?target=external",
    ] {
        assert!(
            validate_rustfs_endpoint(endpoint).is_err(),
            "应拒绝非精确回环地址: {endpoint}"
        );
    }
}

async fn create_job_tables(database: &sea_orm::DatabaseConnection) {
    let schema = Schema::new(sea_orm::DatabaseBackend::MySql);
    database
        .execute(&schema.create_table_from_entity(background_job::Entity))
        .await
        .expect("创建后台任务表");
    database
        .execute(&schema.create_table_from_entity(export_job::Entity))
        .await
        .expect("创建导出任务表");
}

async fn seed_exactly_one_hundred_thousand_users(database: &sea_orm::DatabaseConnection) {
    let now = Utc::now();
    user::Entity::insert(user::ActiveModel {
        id: Set(1),
        tenant_id: Set("system".into()),
        username: Set("admin".into()),
        password_hash: Set("not-used-in-export-acceptance".into()),
        nickname: Set("管理员".into()),
        email: Set("admin@example.com".into()),
        phone: Set("13800000000".into()),
        avatar: Set(None),
        avatar_file_id: Set(None),
        preferred_locale: Set(None),
        status: Set(user::Model::STATUS_NORMAL.into()),
        authorization_version: Set(1),
        dept_id: Set(None),
        remark: Set(None),
        login_ip: Set(None),
        login_date: Set(None),
        del_flag: Set(user::Model::DEL_FLAG_NORMAL.into()),
        created_at: Set(now),
        updated_at: Set(now),
    })
    .exec(database)
    .await
    .expect("写入导出申请人");
    role::Entity::insert(role::ActiveModel {
        id: Set(1),
        tenant_id: Set("system".into()),
        name: Set("超级管理员".into()),
        code: Set("super_admin".into()),
        is_super: Set(1),
        data_scope: Set(role::Model::DATA_SCOPE_ALL.into()),
        status: Set(role::Model::STATUS_NORMAL.into()),
        sort: Set(0),
        remark: Set(None),
        del_flag: Set(role::Model::DEL_FLAG_NORMAL.into()),
        created_at: Set(now),
        updated_at: Set(now),
    })
    .exec(database)
    .await
    .expect("写入超级管理员角色");
    user_role::Entity::insert(user_role::ActiveModel {
        tenant_id: Set("system".into()),
        user_id: Set(1),
        role_id: Set(1),
    })
    .exec(database)
    .await
    .expect("关联导出申请人与超级管理员角色");

    for first_id in (2_i64..=SEEDED_USER_COUNT as i64).step_by(1_000) {
        let last_id = (first_id + 999).min(SEEDED_USER_COUNT as i64);
        let batch = (first_id..=last_id)
            .map(|id| user::ActiveModel {
                id: Set(id),
                tenant_id: Set("system".into()),
                username: Set(format!("export-user-{id:06}")),
                password_hash: Set("not-used-in-export-acceptance".into()),
                nickname: Set(format!("导出用户{id:06}")),
                email: Set(format!("export-user-{id:06}@example.com")),
                phone: Set(format!("139{id:08}")),
                avatar: Set(None),
                avatar_file_id: Set(None),
                preferred_locale: Set(None),
                status: Set(user::Model::STATUS_NORMAL.into()),
                authorization_version: Set(1),
                dept_id: Set(None),
                remark: Set(None),
                login_ip: Set(None),
                login_date: Set(None),
                del_flag: Set(user::Model::DEL_FLAG_NORMAL.into()),
                created_at: Set(now),
                updated_at: Set(now),
            })
            .collect::<Vec<_>>();
        user::Entity::insert_many(batch)
            .exec(database)
            .await
            .expect("分批写入十万行用户种子");
    }

    let count = user::Entity::find()
        .count(database)
        .await
        .expect("统计用户种子");
    assert_eq!(count, SEEDED_USER_COUNT, "用户种子总数必须精确");
}

async fn request_user_export(exports: &ExportService, username: Option<&str>) -> i64 {
    exports
        .request(
            &actor(),
            RequestExportCommand {
                resource: "users".into(),
                permission_code: "system:user:export".into(),
                request_params: serde_json::to_value(UserExportFilters {
                    username: username.map(str::to_owned),
                    phone: None,
                    status: None,
                    dept_id: None,
                })
                .expect("编码用户导出筛选条件"),
            },
        )
        .await
        .expect("创建用户导出任务")
        .id
        .parse()
        .expect("解析导出任务 ID")
}

async fn run_until_claimed(worker: &JobWorker, worker_id: &str) -> JobRunResult {
    tokio::time::timeout(WORKER_PHASE_TIMEOUT, async {
        for _ in 0..100 {
            let result = worker.run_once(worker_id).await.expect("执行后台任务");
            if result != JobRunResult::Idle {
                return result;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("后台任务在轮询期限内未被领取");
    })
    .await
    .expect("后台任务单阶段执行超时")
}

async fn recover_job_after_worker_loss(
    database: &sea_orm::DatabaseConnection,
    queue: &JobQueue,
    job_id: i64,
) -> background_job::Model {
    tokio::time::timeout(LEASE_RECOVERY_TIMEOUT, async {
        loop {
            let job = background_job::Entity::find_by_id(job_id)
                .one(database)
                .await
                .expect("读取待回收后台任务")
                .expect("待回收后台任务存在");
            if job.status == background_job::Model::STATUS_PENDING {
                return job;
            }
            assert_eq!(
                job.status,
                background_job::Model::STATUS_RUNNING,
                "租约回收前任务必须保持 running"
            );
            let lease_until = job.lease_until.expect("running 任务必须持有租约");
            let now = queue.database_now().await.expect("读取数据库时间");
            if lease_until <= now {
                queue
                    .recover_expired_leases()
                    .await
                    .expect("使用生产队列回收 Worker A 的过期租约");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("等待并回收 Worker A 的最终租约超时")
}

async fn wait_until_job_is_ready(
    database: &sea_orm::DatabaseConnection,
    queue: &JobQueue,
    job_id: i64,
) {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let job = background_job::Entity::find_by_id(job_id)
                .one(database)
                .await
                .expect("读取待重试后台任务")
                .expect("待重试后台任务存在");
            if job.status == background_job::Model::STATUS_PENDING
                && job.available_at <= queue.database_now().await.expect("读取数据库时间")
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("等待后台任务重试窗口超时");
}

async fn mark_export_expired(
    database: &sea_orm::DatabaseConnection,
    export_id: i64,
    expires_at: DateTime<Utc>,
) {
    let export = export_job::Entity::find_by_id(export_id)
        .one(database)
        .await
        .expect("读取待过期导出任务")
        .expect("待过期导出任务存在");
    let mut export: export_job::ActiveModel = export.into();
    export.expires_at = Set(Some(expires_at));
    export
        .update(database)
        .await
        .expect("将导出任务设置为已到期");
}

async fn assert_file_state(
    database: &sea_orm::DatabaseConnection,
    file_id: i64,
    expected_del_flag: &str,
) {
    let file = sys_file::Entity::find_by_id(file_id)
        .one(database)
        .await
        .expect("读取导出文件元数据")
        .expect("导出文件元数据存在");
    assert_eq!(file.del_flag, expected_del_flag);
}

#[tokio::test]
#[ignore = "需要隔离 MySQL 与 RustFS"]
async fn export_runtime_acceptance_covers_scale_takeover_storage_recovery_and_cleanup() {
    tokio::time::timeout(ACCEPTANCE_TIMEOUT, run_export_runtime_acceptance())
        .await
        .expect("导出运行时验收总耗时超过三分钟");
}

async fn run_export_runtime_acceptance() {
    // 正常路径逐个删除测试对象；异常退出由 runtime_acceptance.ps1 的 finally
    // 执行 `docker compose down --volumes`，销毁本次唯一 Compose project 的隔离 RustFS 卷。
    let database = common::setup_test_db().await;
    create_job_tables(database.connection()).await;
    seed_exactly_one_hundred_thousand_users(database.connection()).await;

    let proxy = ControllableS3Proxy::start(rustfs_endpoint()).await;
    let storage = Arc::new(
        S3ObjectStorage::new(S3Config {
            endpoint: proxy.endpoint().to_owned(),
            access_key: env_or("RYFRAME_TEST_RUSTFS_ACCESS_KEY", "ryframe-test-access"),
            secret_key: env_or("RYFRAME_TEST_RUSTFS_SECRET_KEY", "ryframe-test-secret-2026"),
            use_ssl: false,
            region: env_or("RYFRAME_TEST_RUSTFS_REGION", "us-east-1"),
        })
        .expect("创建真实 S3ObjectStorage"),
    );
    storage
        .ensure_bucket(EXPORT_BUCKET)
        .await
        .expect("通过代理初始化 RustFS 导出存储桶");

    let job_config = acceptance_job_config();
    let cluster = DatabaseCluster::single(database.connection().clone());
    let users = Arc::new(UserService::new(
        cluster.clone(),
        ryframe_service::AuthorizationCache::disabled(),
    ));
    let exports = Arc::new(ExportService::new(
        cluster.clone(),
        users,
        storage.clone(),
        &job_config,
    ));
    let queue = Arc::new(JobQueue::new(cluster));
    let worker = JobWorker::new(queue.clone(), &job_config)
        .expect("创建导出 Worker")
        .with_handler(Arc::new(ExportJobHandler::new(exports.clone())))
        .expect("注册导出任务处理器");

    // 第一阶段：十万行任务由 Worker A 领取，在对象 PUT 阶段中止，租约过期后由 Worker B 接管。
    let scale_export_id = request_user_export(&exports, None).await;
    let scale_key = format!("system/exports/users-{scale_export_id}.xlsx");
    let scale_export = export_job::Entity::find_by_id(scale_export_id)
        .one(database.connection())
        .await
        .expect("读取十万行导出任务")
        .expect("十万行导出任务存在");
    proxy.pause_next_object_put();
    let worker_a = worker.clone();
    let mut worker_a_task = AbortTaskOnDrop::new(tokio::spawn(async move {
        run_until_claimed(&worker_a, "export-runtime-worker-a").await
    }));
    proxy
        .wait_until_object_put_is_paused(worker_a_task.task_mut())
        .await;

    let claimed = background_job::Entity::find_by_id(scale_export.background_job_id)
        .one(database.connection())
        .await
        .expect("读取 Worker A 已领取任务")
        .expect("Worker A 已领取任务存在");
    assert_eq!(claimed.status, background_job::Model::STATUS_RUNNING);
    assert_eq!(
        claimed.lease_owner.as_deref(),
        Some("export-runtime-worker-a")
    );
    assert_eq!(claimed.attempts, 1);
    assert!(claimed.lease_until.is_some(), "Worker A 持有短租约");

    assert!(
        worker_a_task
            .abort_and_join()
            .await
            .expect_err("Worker A 应在持有租约期间被中止")
            .is_cancelled()
    );
    proxy.resume_paused_object_put();
    let recovered = recover_job_after_worker_loss(
        database.connection(),
        &queue,
        scale_export.background_job_id,
    )
    .await;
    assert_eq!(recovered.status, background_job::Model::STATUS_PENDING);
    assert!(recovered.lease_owner.is_none());
    assert!(recovered.lease_until.is_none());
    assert!(
        !storage
            .exists(EXPORT_BUCKET, &scale_key)
            .await
            .expect("确认 Worker A 未留下对象")
    );

    assert_eq!(
        run_until_claimed(&worker, "export-runtime-worker-b").await,
        JobRunResult::Succeeded
    );
    let completed_scale_job = background_job::Entity::find_by_id(scale_export.background_job_id)
        .one(database.connection())
        .await
        .expect("读取 Worker B 完成的后台任务")
        .expect("Worker B 完成的后台任务存在");
    assert_eq!(
        completed_scale_job.status,
        background_job::Model::STATUS_SUCCEEDED
    );
    assert_eq!(completed_scale_job.attempts, 2);
    assert_eq!(proxy.forwarded_object_puts(), 1, "接管后只应写入一个对象");
    let scale_files = sys_file::Entity::find()
        .filter(sys_file::Column::Id.eq(scale_export_id))
        .all(database.connection())
        .await
        .expect("读取十万行导出文件元数据");
    assert_eq!(scale_files.len(), 1, "接管后只能生成一条结果元数据");

    let scale_location = exports
        .download_location_for_requester(&actor(), scale_export_id)
        .await
        .expect("读取十万行导出下载位置");
    assert_eq!(scale_location.bucket, EXPORT_BUCKET);
    assert_eq!(scale_location.path, scale_key);
    let workbook = storage
        .get(&scale_location.bucket, &scale_location.path)
        .await
        .expect("从真实 RustFS 读回十万行 XLSX");
    let imported = ExcelImporter::read_from_bytes::<ImportedUserRow>(&workbook, None)
        .expect("使用 ExcelImporter 解析十万行 XLSX");
    assert_eq!(imported.len(), SEEDED_USER_COUNT as usize);
    assert_eq!(imported.first().expect("存在首行").user_id, "1");
    assert_eq!(imported.first().expect("存在首行").username, "admin");
    assert_eq!(
        imported.last().expect("存在末行").user_id,
        SEEDED_USER_COUNT.to_string()
    );
    assert_eq!(
        imported.last().expect("存在末行").username,
        "export-user-100000"
    );
    let imported_ids = imported
        .iter()
        .map(|row| row.user_id.parse::<u64>().expect("导入用户 ID 为整数"))
        .collect::<Vec<_>>();
    assert!(
        imported_ids.windows(2).all(|pair| pair[1] == pair[0] + 1),
        "主键游标导出与 XLSX 读回顺序必须严格递增且无遗漏"
    );

    // 第二阶段：小结果首次 PUT 返回 503，后台任务按生产退避重试后恢复。
    let retry_export_id = request_user_export(&exports, Some("admin")).await;
    let retry_export = export_job::Entity::find_by_id(retry_export_id)
        .one(database.connection())
        .await
        .expect("读取 PUT 重试导出任务")
        .expect("PUT 重试导出任务存在");
    let put_failures_before = proxy.injected_put_failures();
    let forwarded_puts_before = proxy.forwarded_object_puts();
    proxy.fail_next_object_put();
    assert_eq!(
        run_until_claimed(&worker, "export-runtime-put-retry").await,
        JobRunResult::Retried
    );
    assert_eq!(proxy.injected_put_failures(), put_failures_before + 1);
    let pending_retry = background_job::Entity::find_by_id(retry_export.background_job_id)
        .one(database.connection())
        .await
        .expect("读取首次 PUT 失败任务")
        .expect("首次 PUT 失败任务存在");
    assert_eq!(pending_retry.status, background_job::Model::STATUS_PENDING);
    assert_eq!(pending_retry.attempts, 1);
    assert!(
        pending_retry
            .last_error
            .as_deref()
            .is_some_and(|message| message.contains("503"))
    );
    let queued_retry = export_job::Entity::find_by_id(retry_export_id)
        .one(database.connection())
        .await
        .expect("读取首次 PUT 失败后的公开导出任务")
        .expect("首次 PUT 失败后的公开导出任务存在");
    assert_eq!(queued_retry.status, export_job::Model::STATUS_QUEUED);

    wait_until_job_is_ready(
        database.connection(),
        &queue,
        retry_export.background_job_id,
    )
    .await;
    assert_eq!(
        run_until_claimed(&worker, "export-runtime-put-recovery").await,
        JobRunResult::Succeeded
    );
    assert_eq!(
        proxy.forwarded_object_puts(),
        forwarded_puts_before + 1,
        "503 后只能成功转发一次重试 PUT"
    );
    let recovered_retry = background_job::Entity::find_by_id(retry_export.background_job_id)
        .one(database.connection())
        .await
        .expect("读取 PUT 恢复任务")
        .expect("PUT 恢复任务存在");
    assert_eq!(
        recovered_retry.status,
        background_job::Model::STATUS_SUCCEEDED
    );
    assert_eq!(recovered_retry.attempts, 2);

    // 第三阶段：首个过期对象 DELETE 返回 503，本轮仍清理后续项，下一轮恢复首项。
    let follower_export_id = request_user_export(&exports, Some("export-user-100000")).await;
    assert!(retry_export_id < follower_export_id);
    assert_eq!(
        run_until_claimed(&worker, "export-runtime-cleanup-follower").await,
        JobRunResult::Succeeded
    );
    let retry_location = exports
        .download_location_for_requester(&actor(), retry_export_id)
        .await
        .expect("读取 PUT 恢复结果位置");
    let follower_location = exports
        .download_location_for_requester(&actor(), follower_export_id)
        .await
        .expect("读取清理后续结果位置");
    let expires_at =
        queue.database_now().await.expect("读取数据库时间") - chrono::Duration::seconds(1);
    mark_export_expired(database.connection(), retry_export_id, expires_at).await;
    mark_export_expired(database.connection(), follower_export_id, expires_at).await;

    let delete_failures_before = proxy.injected_delete_failures();
    proxy.fail_next_object_delete();
    let first_cleanup = exports.cleanup_expired().await;
    assert!(first_cleanup.is_err(), "首个 DELETE 503 必须汇总为本轮错误");
    assert_eq!(proxy.injected_delete_failures(), delete_failures_before + 1);
    let failed_cleanup_export = export_job::Entity::find_by_id(retry_export_id)
        .one(database.connection())
        .await
        .expect("读取首次清理失败任务")
        .expect("首次清理失败任务存在");
    let continued_cleanup_export = export_job::Entity::find_by_id(follower_export_id)
        .one(database.connection())
        .await
        .expect("读取本轮继续清理任务")
        .expect("本轮继续清理任务存在");
    assert_eq!(
        failed_cleanup_export.status,
        export_job::Model::STATUS_SUCCEEDED
    );
    assert_eq!(
        continued_cleanup_export.status,
        export_job::Model::STATUS_EXPIRED
    );
    assert_file_state(
        database.connection(),
        retry_export_id,
        sys_file::Model::DEL_FLAG_NORMAL,
    )
    .await;
    assert_file_state(
        database.connection(),
        follower_export_id,
        sys_file::Model::DEL_FLAG_DELETED,
    )
    .await;
    assert!(
        storage
            .exists(&retry_location.bucket, &retry_location.path)
            .await
            .expect("确认首次 DELETE 失败对象仍存在")
    );
    assert!(
        !storage
            .exists(&follower_location.bucket, &follower_location.path)
            .await
            .expect("确认后续过期对象已删除")
    );

    assert_eq!(exports.cleanup_expired().await.expect("下一轮恢复清理"), 1);
    let recovered_cleanup_export = export_job::Entity::find_by_id(retry_export_id)
        .one(database.connection())
        .await
        .expect("读取恢复清理任务")
        .expect("恢复清理任务存在");
    assert_eq!(
        recovered_cleanup_export.status,
        export_job::Model::STATUS_EXPIRED
    );
    assert_file_state(
        database.connection(),
        retry_export_id,
        sys_file::Model::DEL_FLAG_DELETED,
    )
    .await;
    assert!(
        !storage
            .exists(&retry_location.bucket, &retry_location.path)
            .await
            .expect("确认恢复清理后对象不存在")
    );
    for export_id in [retry_export_id, follower_export_id] {
        let error = exports
            .download_location_for_requester(&actor(), export_id)
            .await
            .expect_err("已过期导出必须拒绝下载");
        assert_eq!(error.error_code(), ErrorCode::Conflict);
    }

    // 清除规模验收对象，避免共享 RustFS 在本地重复运行时残留测试数据。
    let scale_expires_at =
        queue.database_now().await.expect("读取数据库时间") - chrono::Duration::seconds(1);
    mark_export_expired(database.connection(), scale_export_id, scale_expires_at).await;
    assert_eq!(
        exports.cleanup_expired().await.expect("清理规模验收对象"),
        1
    );
    assert!(
        !storage
            .exists(EXPORT_BUCKET, &scale_key)
            .await
            .expect("确认规模验收对象已删除")
    );

    proxy.shutdown().await;
}
