use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use redis::{AsyncCommands, RedisResult};
use ryframe_adapters::RedisClient;
use tokio::sync::{Barrier, Notify, mpsc};

use super::{ENABLE_ENV, finish_with_cleanup, integration_enabled, redis_config, skip_message};

#[tokio::test]
async fn successful_transactions_reuse_one_connection_for_4000_operations() {
    if !enabled() {
        return;
    }
    let (client, owner) = connect_owned("tx-reuse").await;
    let mut ordinary = client.conn().clone();
    let ordinary_id: i64 = redis::cmd("CLIENT")
        .arg("ID")
        .query_async(&mut ordinary)
        .await
        .expect("普通连接 ID");
    let first_id = transaction_id(&client).await.expect("首次事务 ID");
    assert_ne!(ordinary_id, first_id, "普通命令不得共享 WATCH 连接");
    let before = connections_received(&client).await;
    for _ in 0..4_000 {
        assert_eq!(
            transaction_id(&client).await.expect("复用事务连接"),
            first_id,
            "成功事务之间不得重新建立连接"
        );
    }
    let after = connections_received(&client).await;
    // INFO 是整个实例的计数，其他隔离 namespace 可并行连接；本测试以稳定 CLIENT ID 断言复用。
    eprintln!("4000 次事务使用同一连接 {first_id}，实例总连接计数 {before} → {after}");
    finish_owned(Ok(()), &client, &owner, &[]).await;
}

#[tokio::test]
async fn idle_reused_connection_is_reopened_before_the_operation() {
    if !enabled() {
        return;
    }
    let (client, owner) = connect_owned("tx-idle-disconnect").await;
    let original = transaction_id(&client).await.expect("首次事务 ID");
    let mut ordinary = client.conn().clone();
    let killed: i64 = redis::cmd("CLIENT")
        .arg("KILL")
        .arg("ID")
        .arg(original)
        .query_async(&mut ordinary)
        .await
        .expect("只终止已归还的事务连接");
    assert_eq!(killed, 1);

    let replacement = transaction_id(&client)
        .await
        .expect("复用连接失效时应在事务操作前透明重建");
    assert_ne!(replacement, original);
    finish_owned(Ok(()), &client, &owner, &[]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn configured_pool_allows_two_transactions_to_enter_concurrently() {
    if !enabled() {
        return;
    }
    let mut config = redis_config("tx-pool-concurrency");
    config.max_pool_size = 2;
    let owner = config.scope_id().ownership_marker("redis");
    let client = RedisClient::connect(&config)
        .await
        .expect("连接隔离协议环境");
    client
        .ensure_scope_ownership(&owner)
        .await
        .expect("登记本次协议 namespace 所有权");

    let (started, mut observed) = mpsc::channel(2);
    let first_release = Arc::new(Notify::new());
    let second_release = Arc::new(Notify::new());
    let first = held_transaction(
        client.clone(),
        "protocol:pool-first",
        started.clone(),
        first_release.clone(),
    );
    let second = held_transaction(
        client.clone(),
        "protocol:pool-second",
        started,
        second_release.clone(),
    );
    let entered = tokio::time::timeout(Duration::from_secs(2), async {
        let first = observed.recv().await.expect("第一个事务进入闭包");
        let second = observed.recv().await.expect("第二个事务进入闭包");
        (first, second)
    })
    .await;
    first_release.notify_one();
    second_release.notify_one();

    let result = async {
        let first_result = first
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        let second_result = second
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        let (first_id, second_id) =
            entered.map_err(|_| "两个事务未能在同一时间进入闭包".to_owned())?;
        if first_id == second_id {
            return Err("并发事务错误复用了同一 Redis 连接".to_owned());
        }
        if first_result == second_result
            || ![first_id, second_id].contains(&first_result)
            || ![first_id, second_id].contains(&second_result)
        {
            return Err("事务执行期间连接身份发生变化".to_owned());
        }
        Ok(())
    }
    .await;
    finish_owned(result, &client, &owner, &[]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pool_limit_blocks_extra_transaction_and_cancel_releases_its_permit() {
    if !enabled() {
        return;
    }
    let mut config = redis_config("tx-pool-limit");
    config.max_pool_size = 1;
    let owner = config.scope_id().ownership_marker("redis");
    let client = RedisClient::connect(&config)
        .await
        .expect("连接隔离协议环境");
    client
        .ensure_scope_ownership(&owner)
        .await
        .expect("登记本次协议 namespace 所有权");

    let (first_started, mut first_observed) = mpsc::channel(1);
    let first = held_transaction(
        client.clone(),
        "protocol:pool-held",
        first_started,
        Arc::new(Notify::new()),
    );
    let first_id = first_observed.recv().await.expect("首个事务进入闭包");

    let (second_started, mut second_observed) = mpsc::channel(1);
    let second_release = Arc::new(Notify::new());
    let second = held_transaction(
        client.clone(),
        "protocol:pool-waiting",
        second_started,
        second_release.clone(),
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(100), second_observed.recv())
            .await
            .is_err(),
        "容量为一时第二个事务不得提前进入闭包"
    );

    first.abort();
    assert!(first.await.expect_err("首个事务已取消").is_cancelled());
    let second_id = tokio::time::timeout(Duration::from_secs(2), second_observed.recv())
        .await
        .expect("取消后必须及时释放事务池许可")
        .expect("第二个事务进入闭包");
    assert_ne!(second_id, first_id, "取消的连接不得被归还事务池");
    second_release.notify_one();
    let result = second
        .await
        .map_err(|error| error.to_string())
        .and_then(|result| result.map_err(|error| error.to_string()))
        .and_then(|completed| {
            (completed == second_id)
                .then_some(())
                .ok_or_else(|| "第二个事务的连接身份发生变化".to_owned())
        });
    finish_owned(result, &client, &owner, &[]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_transactions_keep_watch_read_and_exec_exclusive() {
    if !enabled() {
        return;
    }
    let (client, owner) = connect_owned("tx-exclusive").await;
    let key = "protocol:transaction-counter";
    client.set(key, "0").await.expect("创建本次精确计数键");
    let barrier = Arc::new(Barrier::new(8));
    let mut workers = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let client = client.clone();
        let barrier = barrier.clone();
        workers.spawn(async move {
            barrier.wait().await;
            for _ in 0..50 {
                increment_watched(&client, key).await?;
            }
            Ok::<_, redis::RedisError>(())
        });
    }
    let result = async {
        while let Some(worker) = workers.join_next().await {
            worker
                .map_err(|error| error.to_string())?
                .map_err(|error| error.to_string())?;
        }
        let value = client.get(key).await.map_err(|error| error.to_string())?;
        if value.as_deref() != Some("400") {
            return Err(format!("并发 WATCH 事务丢失更新: {value:?}"));
        }
        Ok(())
    }
    .await;
    finish_owned(result, &client, &owner, &[key]).await;
}

#[tokio::test]
async fn disconnected_transaction_fails_and_next_call_uses_new_connection() {
    if !enabled() {
        return;
    }
    let (client, owner) = connect_owned("tx-disconnect").await;
    let (started, mut observed) = mpsc::channel(1);
    let release = Arc::new(Notify::new());
    let running = held_transaction(
        client.clone(),
        "protocol:disconnect",
        started,
        release.clone(),
    );
    let original = observed.recv().await.expect("观察本次事务的真实连接 ID");
    let mut ordinary = client.conn().clone();
    let killed: i64 = redis::cmd("CLIENT")
        .arg("KILL")
        .arg("ID")
        .arg(original)
        .query_async(&mut ordinary)
        .await
        .expect("只终止本测试刚观察到的事务连接");
    assert_eq!(killed, 1);
    release.notify_one();
    assert!(
        running.await.expect("事务任务完成").is_err(),
        "断线当次必须失败"
    );
    assert_ne!(
        transaction_id(&client)
            .await
            .expect("下一次调用重新建立连接"),
        original
    );
    finish_owned(Ok(()), &client, &owner, &[]).await;
}

#[tokio::test]
async fn committed_transaction_survives_cleanup_failure_and_discards_connection() {
    if !enabled() {
        return;
    }
    let (client, owner) = connect_owned("tx-cleanup-failure").await;
    let key = "protocol:cleanup-after-commit";
    let physical_key = client.scoped_key(key);
    let (committed, mut observed) = mpsc::channel(1);
    let release = Arc::new(Notify::new());
    let transaction_release = release.clone();
    let transaction_client = client.clone();
    let running = tokio::spawn(async move {
        transaction_client
            .transaction(&[key], async move |mut connection, mut pipeline| {
                let physical_key = physical_key.clone();
                let committed = committed.clone();
                let release = transaction_release.clone();
                let id: i64 = redis::cmd("CLIENT")
                    .arg("ID")
                    .query_async(&mut connection)
                    .await?;
                let value: Option<(String,)> = pipeline
                    .set(&physical_key, "committed")
                    .ignore()
                    .get(&physical_key)
                    .query_async(&mut connection)
                    .await?;
                if value.is_some() {
                    committed.send(id).await.expect("测试观察者仍在等待");
                    release.notified().await;
                }
                Ok(value.map(|(value,)| (id, value)))
            })
            .await
    });
    let original = observed.recv().await.expect("EXEC 已成功返回");
    let mut ordinary = client.conn().clone();
    let killed: i64 = redis::cmd("CLIENT")
        .arg("KILL")
        .arg("ID")
        .arg(original)
        .query_async(&mut ordinary)
        .await
        .expect("只终止已提交事务的连接");
    assert_eq!(killed, 1);
    release.notify_one();

    let result = async {
        let (returned_id, value) = running
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        if returned_id != original || value != "committed" {
            return Err(format!(
                "已提交事务结果丢失: id={returned_id}, value={value:?}"
            ));
        }
        let stored = client.get(key).await.map_err(|error| error.to_string())?;
        if stored.as_deref() != Some("committed") {
            return Err(format!("EXEC 成功后的写入未保留: {stored:?}"));
        }
        let next = transaction_id(&client)
            .await
            .map_err(|error| error.to_string())?;
        if next == original {
            return Err("清理失败的事务连接被错误复用".to_owned());
        }
        Ok(())
    }
    .await;
    finish_owned(result, &client, &owner, &[key]).await;
}

#[tokio::test]
async fn committed_transaction_survives_cleanup_timeout_and_discards_connection() {
    if !enabled() {
        return;
    }
    let (client, owner) = connect_owned("tx-cleanup-timeout").await;
    let key = "protocol:cleanup-timeout";
    let blocker_key = "protocol:cleanup-blocker";
    let physical_key = client.scoped_key(key);
    let physical_blocker_key = client.scoped_key(blocker_key);
    let (committed, mut observed) = mpsc::channel(1);
    let release = Arc::new(Notify::new());
    let transaction_release = release.clone();
    let transaction_client = client.clone();
    let running = tokio::spawn(async move {
        transaction_client
            .transaction(&[key], async move |mut connection, mut pipeline| {
                let physical_key = physical_key.clone();
                let physical_blocker_key = physical_blocker_key.clone();
                let committed = committed.clone();
                let release = transaction_release.clone();
                let id: i64 = redis::cmd("CLIENT")
                    .arg("ID")
                    .query_async(&mut connection)
                    .await?;
                let value: Option<(String,)> = pipeline
                    .set(&physical_key, "committed")
                    .ignore()
                    .get(&physical_key)
                    .query_async(&mut connection)
                    .await?;
                if value.is_some() {
                    let mut command = redis::cmd("BLPOP");
                    command.arg(&physical_blocker_key).arg(10);
                    let mut blocker =
                        Box::pin(command.query_async::<Option<(String, String)>>(&mut connection));
                    committed.send(id).await.expect("测试观察者仍在等待");
                    tokio::select! {
                        result = &mut blocker => {
                            panic!("用于阻塞 UNWATCH 的 BLPOP 意外提前完成: {result:?}");
                        }
                        () = release.notified() => {}
                    }
                    drop(blocker);
                }
                Ok(value.map(|(value,)| (id, value)))
            })
            .await
    });
    let original = observed.recv().await.expect("EXEC 已成功返回");
    if !wait_for_client_command(&client, original, "blpop").await {
        release.notify_one();
        running.abort();
        panic!("未观察到事务连接上的阻塞命令");
    }
    release.notify_one();

    let result = async {
        let (returned_id, value) = running
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        if returned_id != original || value != "committed" {
            return Err(format!(
                "清理超时后已提交事务结果丢失: id={returned_id}, value={value:?}"
            ));
        }
        let stored = client.get(key).await.map_err(|error| error.to_string())?;
        if stored.as_deref() != Some("committed") {
            return Err(format!("清理超时后写入未保留: {stored:?}"));
        }
        let next = transaction_id(&client)
            .await
            .map_err(|error| error.to_string())?;
        if next == original {
            return Err("清理超时的事务连接被错误复用".to_owned());
        }
        Ok(())
    }
    .await;
    finish_owned(result, &client, &owner, &[key, blocker_key]).await;
}

#[tokio::test]
async fn executed_transaction_with_lost_response_is_not_replayed() {
    if !enabled() {
        return;
    }
    let mut config = redis_config("tx-unknown-result");
    config.timeout_secs = 1;
    let owner = config.scope_id().ownership_marker("redis");
    let client = RedisClient::connect(&config)
        .await
        .expect("连接隔离协议环境");
    client
        .ensure_scope_ownership(&owner)
        .await
        .expect("登记本次协议 namespace 所有权");
    let key = "protocol:unknown-result";
    let physical_key = client.scoped_key(key);
    let calls = Arc::new(AtomicUsize::new(0));
    let operation_calls = calls.clone();
    let result: RedisResult<()> = client
        .transaction(&[key], async move |mut connection, mut pipeline| {
            let physical_key = physical_key.clone();
            let calls = operation_calls.clone();
            calls.fetch_add(1, Ordering::SeqCst);
            let committed: Option<()> = pipeline
                .cmd("CLIENT")
                .arg("REPLY")
                .arg("OFF")
                .ignore()
                .set(&physical_key, "committed")
                .query_async(&mut connection)
                .await?;
            Ok(committed)
        })
        .await;

    let verification = async {
        if result.is_ok() {
            return Err("丢失 EXEC 响应的事务不得报告成功".to_owned());
        }
        if calls.load(Ordering::SeqCst) != 1 {
            return Err("未知提交结果错误重放了事务闭包".to_owned());
        }
        let stored = client.get(key).await.map_err(|error| error.to_string())?;
        if stored.as_deref() != Some("committed") {
            return Err(format!("服务端未保留已经执行的事务: {stored:?}"));
        }
        Ok(())
    }
    .await;
    finish_owned(verification, &client, &owner, &[key]).await;
}

#[tokio::test]
async fn cancelled_transaction_discards_its_connection_and_watch_state() {
    if !enabled() {
        return;
    }
    let (client, owner) = connect_owned("tx-cancel").await;
    let stale_key = "protocol:cancelled-watch";
    let next_key = "protocol:next-watch";
    let (started, mut observed) = mpsc::channel(1);
    let running = held_transaction(client.clone(), stale_key, started, Arc::new(Notify::new()));
    let original = observed.recv().await.expect("取消前已完成 WATCH");
    running.abort();
    assert!(running.await.expect_err("事务任务被取消").is_cancelled());

    let (started, mut observed) = mpsc::channel(1);
    let release = Arc::new(Notify::new());
    let next = held_transaction(client.clone(), next_key, started, release.clone());
    let new_id = observed.recv().await.expect("新事务已完成 WATCH");
    let result = async {
        if original == new_id {
            return Err("取消的事务连接被错误归还".to_owned());
        }
        client
            .set(stale_key, "changed")
            .await
            .map_err(|error| error.to_string())?;
        release.notify_one();
        let completed = next
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        if completed != new_id {
            return Err("新事务继承旧 WATCH 或连接代次变化".to_owned());
        }
        Ok(())
    }
    .await;
    finish_owned(result, &client, &owner, &[stale_key]).await;
}

async fn connect_owned(name: &str) -> (RedisClient, String) {
    let config = redis_config(name);
    let owner = config.scope_id().ownership_marker("redis");
    let client = RedisClient::connect(&config)
        .await
        .expect("连接隔离协议环境");
    client
        .ensure_scope_ownership(&owner)
        .await
        .expect("登记本次协议 namespace 所有权");
    (client, owner)
}

async fn finish_owned(
    result: Result<(), String>,
    client: &RedisClient,
    owner: &str,
    keys: &[&str],
) {
    client
        .verify_scope_ownership(owner)
        .await
        .expect("精确清理前重新校验所有权");
    let mut keys: Vec<_> = keys.iter().map(|key| (*key).to_owned()).collect();
    keys.push(client.ownership_marker_key());
    finish_with_cleanup(result, &[(client, keys)]).await;
}

fn enabled() -> bool {
    if integration_enabled(ENABLE_ENV) {
        true
    } else {
        skip_message();
        false
    }
}

async fn transaction_id(client: &RedisClient) -> RedisResult<i64> {
    client
        .transaction(
            &["protocol:connection-id"],
            async move |mut connection, mut pipeline| {
                let value: Option<(i64,)> = pipeline
                    .cmd("CLIENT")
                    .arg("ID")
                    .query_async(&mut connection)
                    .await?;
                Ok(value.map(|(id,)| id))
            },
        )
        .await
}

async fn connections_received(client: &RedisClient) -> u64 {
    let mut ordinary = client.conn().clone();
    let info: String = redis::cmd("INFO")
        .arg("stats")
        .query_async(&mut ordinary)
        .await
        .expect("只读服务端连接计数");
    info.lines()
        .find_map(|line| line.strip_prefix("total_connections_received:"))
        .expect("服务端公开连接计数")
        .trim()
        .parse()
        .expect("有效计数")
}

async fn wait_for_client_command(client: &RedisClient, id: i64, command: &str) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    let id_field = format!("id={id}");
    let command_field = format!("cmd={command}");
    loop {
        let mut ordinary = client.conn().clone();
        let clients: String = redis::cmd("CLIENT")
            .arg("LIST")
            .query_async(&mut ordinary)
            .await
            .expect("读取 Redis 客户端状态");
        if clients.lines().any(|line| {
            let fields = line.split_ascii_whitespace().collect::<Vec<_>>();
            fields.contains(&id_field.as_str()) && fields.contains(&command_field.as_str())
        }) {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn increment_watched(client: &RedisClient, key: &str) -> RedisResult<i64> {
    let physical_key = client.scoped_key(key);
    client
        .transaction(&[key], async move |mut connection, mut pipeline| {
            let physical_key = physical_key.clone();
            let before: i64 = connection.get(&physical_key).await?;
            tokio::task::yield_now().await;
            let result: Option<(i64,)> = pipeline
                .set(&physical_key, before + 1)
                .ignore()
                .get(&physical_key)
                .query_async(&mut connection)
                .await?;
            Ok(result.map(|(value,)| value))
        })
        .await
}

fn held_transaction(
    client: RedisClient,
    key: &'static str,
    started: mpsc::Sender<i64>,
    release: Arc<Notify>,
) -> tokio::task::JoinHandle<RedisResult<i64>> {
    tokio::spawn(async move {
        client
            .transaction(&[key], async move |mut connection, mut pipeline| {
                let started = started.clone();
                let release = release.clone();
                let id = redis::cmd("CLIENT")
                    .arg("ID")
                    .query_async(&mut connection)
                    .await?;
                started.send(id).await.expect("测试观察者仍在等待");
                release.notified().await;
                let value: Option<(i64,)> = pipeline
                    .cmd("CLIENT")
                    .arg("ID")
                    .query_async(&mut connection)
                    .await?;
                Ok(value.map(|(id,)| id))
            })
            .await
    })
}
