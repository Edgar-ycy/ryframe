use super::{adoption::*, *};
use ryframe_sdk::{
    BusinessAdoptionState, apply_business_adoption, inspect_business_adoption,
    preview_business_adoption,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_commit_response_requires_authoritative_inspection_and_no_replay() {
    mysql::run_mysql_test("adoptunknown", |database| async move {
        mysql::execute(&database, DDL).await?;
        mysql::execute(&database, "INSERT INTO biz_sql_example(code) VALUES('keep-unknown-result')").await?;
        empty_ledger(&database).await?;
        let initial = migration(false);
        let plan = preview_business_adoption(&database, BusinessMigrationScope::Control, initial.module(), &[&initial]).await.map_err(message)?;
        let rows = business_rows(&database).await?;
        let configured = mysql::MySqlTestConfiguration::load()
            .unwrap_or_else(|error| panic!("{error}"));
        let mut config = configured.connection(&plan.manifest.target.database, None)
            .unwrap_or_else(|error| panic!("{error}"));
        let mut gate = response_gate::ResponseGate::start(config.host.clone(), config.port).await.map_err(message)?;
        config.host = gate.address.ip().to_string();
        config.port = gate.address.port();
        config.max_connections = 1;
        config.min_connections = 1;
        let proxy = match ryframe_db::connection::connect(&config).await {
            Ok(database) => database,
            Err(error) => { gate.stop().await?; return Err(message(error)); }
        };
        gate.arm();
        let proxy_apply = proxy.clone();
        let saved = plan.clone();
        let mut applying = tokio::spawn(async move {
            apply_business_adoption(&proxy_apply, &saved, &[&migration(false)]).await
        });
        let observed = tokio::time::timeout(std::time::Duration::from_secs(20), async {
            loop {
                tokio::select! {
                    event = gate.responses.recv() => {
                        let permit = event.ok_or("响应 gate 提前结束")?;
                        let row = database.query_one_raw(Statement::from_sql_and_values(DbBackend::MySql,
                            "SELECT COUNT(*) AS value FROM ryframe_business_migration WHERE origin='adopted' AND adoption_hash=?", [plan.fingerprint.clone().into()]))
                            .await.map_err(mysql::db_error)?.ok_or("无法读取权威账本")?;
                        let committed: i64 = row.try_get("", "value").map_err(mysql::db_error)?;
                        if committed == 1 { let _ = permit.send(false); return Ok(()); }
                        if committed != 0 { return Err("权威后像不是完整的单条接管结果".into()); }
                        permit.send(true).map_err(|_| "转发通道提前关闭")?;
                    }
                }
            }
        }).await.map_err(|_| "等待真实 COMMIT 响应 gate 超时".to_owned()).and_then(|value| value);
        let stopped = gate.stop().await;
        // 每条失败路径都结束任务和 TCP 连接，不能遗留事务影响精确清理。
        let completion = if observed.is_ok() {
            tokio::time::timeout(std::time::Duration::from_secs(10), &mut applying).await
                .map_err(|_| "提交响应丢失后客户端未结束".to_owned())
        } else {
            applying.abort();
            let _ = applying.await;
            proxy.close().await.map_err(mysql::db_error)?;
            stopped?;
            return observed;
        };
        if completion.is_err() { applying.abort(); let _ = applying.await; }
        proxy.close().await.map_err(mysql::db_error)?;
        stopped?;
        observed?;
        let failure = completion?.map_err(message)?.unwrap_err().to_string();
        assert!(failure.contains("提交结果未知"), "{failure}");
        assert_eq!(inspect_business_adoption(&database, &plan, &[&initial]).await.map_err(message)?, BusinessAdoptionState::Applied);
        let before = inventory(&database).await?;
        let time = timestamps(&database).await?;
        let outcome = apply_business_adoption(&database, &plan, &[&initial]).await.map_err(message)?;
        assert!(outcome.already_applied && outcome.registered == 0);
        assert_eq!(inventory(&database).await?, before);
        assert_eq!(timestamps(&database).await?, time);
        assert_eq!(business_rows(&database).await?, rows);
        Ok(())
    }).await;
}
