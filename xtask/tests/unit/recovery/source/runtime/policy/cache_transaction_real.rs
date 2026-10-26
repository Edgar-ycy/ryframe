//! 真实 Redis 的生产 execute 验收；测试注入不属于正式 source 发布链。
use super::{cache_io, cache_transaction, operations_tests::cache_fixture};
use crate::{
    Result,
    ci::integration_configuration::{capture::Captured, files, owned::OwnedConfiguration},
    recovery::clone::native::{
        json as document,
        resources::common,
        target_verify::{input::request_configuration, redis::Client, storage::StorageInputs},
    },
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

struct Fixture {
    backend: PathBuf,
    directory: PathBuf,
    captured: Captured,
    owned: OwnedConfiguration,
    request: Value,
    environment: BTreeMap<String, String>,
    namespace: String,
    owner_key: String,
    owner_value: String,
    comparison: cache_io::Comparison,
}

impl Fixture {
    fn open(case: &str) -> Result<Self> {
        let backend = crate::workspace::root_dir();
        let source = backend.join(".local-tests/service-tests/source-watch/config");
        if !files::directory_available(&source)? {
            return Err("真实 Redis 未验收：缺少 service-tests/source-watch/config；请由控制器准备隔离文件配置后显式运行".into());
        }
        let captured = Captured::load(&source, false)?;
        let redis = captured
            .app
            .redis
            .as_ref()
            .ok_or("真实 Redis 未验收：文件配置缺少 redis；请配置隔离服务")?;
        if !captured.redis_endpoint_explicit
            || redis.mode != ryframe_config::RedisMode::Required
            || redis.host != "127.0.0.1"
            || redis.database != 0
            || redis.tls
        {
            return Err("真实 Redis 未验收：生产 Client 要求显式 required、127.0.0.1、database=0、tls=false；请核对隔离文件配置，不采用覆盖".into());
        }
        let port = redis.port;
        let scope = format!("watch-{}", uuid::Uuid::new_v4().simple());
        common::scope_id(&scope)?;
        let parent = backend.join(".local-tests/service-tests/runtime");
        files::create_parents(&parent)?;
        // 先保留容器，后续任何失败都不触发递归清理或资源重放。
        let directory = tempfile::Builder::new()
            .prefix("source-watch-")
            .tempdir_in(parent)?
            .keep();
        eprintln!("真实 Redis {case} 验收现场：{}", directory.display());
        let mut selected = captured.selected.clone();
        selected.insert("scope_id".into(), toml::Value::String(scope.clone()));
        let owned = OwnedConfiguration::create(
            &directory.join("config"),
            &captured.derived_files(&selected)?,
        )?;
        let environment = BTreeMap::from([("APP_ENV".into(), "test".into())]);
        let mut request = json!({"config_dir":owned.directory,"target":{"scope_id":scope},"storage":{"redis":{"port":port}}});
        let (path, images) = request_configuration::fresh_files(&backend, &environment, &request)?;
        request["configuration_sha256"] = json!(request_configuration::configuration_identity(
            &path, &images
        )?);
        let namespace = format!("ryframe:{{{scope}}}:");
        let comparison = comparison(&namespace)?;
        let fixture = Self {
            backend,
            directory,
            captured,
            owned,
            request,
            environment,
            owner_key: format!("{namespace}.ryframe-owner"),
            owner_value: format!("source-watch:v1:{scope}"),
            namespace,
            comparison,
        };
        common::write_new(
            &fixture.directory.join("ownership.json"),
            &json!({"case":case,
            "namespace":fixture.namespace,"owner_key":fixture.owner_key,"owner_value":fixture.owner_value,
            "configuration_sha256":fixture.request["configuration_sha256"]}),
        )?;
        Ok(fixture)
    }

    fn connect(&self) -> Result<Client> {
        self.verify_configuration()?;
        let inputs = StorageInputs {
            backend: &self.backend,
            request: &self.request,
            environment: &self.environment,
            observation_dir: &self.directory,
        };
        Client::authenticated(&inputs, &self.request["storage"]["redis"]).map_err(|error| {
            format!("真实 Redis 未验收：{error}；请核对隔离服务及已绑定文件，保留现场").into()
        })
    }

    fn verify_configuration(&self) -> Result<()> {
        self.captured.verify()?;
        self.owned.verify()?;
        Ok(())
    }

    fn populate(&self, client: &mut Client) -> Result<()> {
        self.verify_configuration()?;
        if client.transaction_command(&["SET", &self.owner_key, &self.owner_value, "NX"])?
            != json!("OK")
        {
            return Err("真实 Redis scope 已有 owner；保留现场，禁止接管".into());
        }
        self.require_owner(client)?;
        if names(client, &self.namespace)? != BTreeSet::from([self.owner_key.clone()]) {
            return Err("真实 Redis scope 含外来键；保留现场，不写入测试数据".into());
        }
        for (keys, values) in self
            .comparison
            .keys
            .as_chunks::<3>()
            .0
            .iter()
            .zip(self.comparison.values.as_chunks::<5>().0.iter())
        {
            for (key, value) in keys.iter().zip(values).take(2) {
                if client.transaction_command(&["SET", key, value, "NX"])? != json!("OK") {
                    return Err("真实 Redis 前像键出现竞争；保留现场，不接管".into());
                }
            }
            if client.transaction_command(&["HSET", &keys[2], &values[3], &values[4]])? != json!(1)
            {
                return Err("真实 Redis 快照键出现竞争；保留现场".into());
            }
        }
        self.require_owner(client)?;
        Ok(())
    }

    fn require_owner(&self, client: &mut Client) -> Result<()> {
        self.verify_configuration()?;
        if client.transaction_command(&["GET", &self.owner_key])? != json!(self.owner_value) {
            return Err("真实 Redis owner 已变化；禁止继续或清理".into());
        }
        Ok(())
    }

    fn finish(self, mut client: Client, expected: &[Value]) -> Result<()> {
        self.require_owner(&mut client)?;
        let known = self
            .comparison
            .keys
            .iter()
            .cloned()
            .chain([self.owner_key.clone()])
            .collect::<BTreeSet<_>>();
        if !names(&mut client, &self.namespace)?.is_subset(&known) {
            return Err("真实 Redis 清理前出现未知键；保留现场，不删除".into());
        }
        // 同一原子命令复核归属并精确删除已知键；失败不按前缀清理。
        let mut watch = vec!["WATCH", self.owner_key.as_str()];
        watch.extend(self.comparison.keys.iter().map(String::as_str));
        if client.transaction_command(&watch)? != json!("OK") {
            return Err("清理 WATCH 未确认，保留现场".into());
        }
        self.require_owner(&mut client)?;
        if image(&mut client, &self.comparison)? != expected {
            return Err("清理前完整后像已变化，保留现场，不删除".into());
        }
        if client.transaction_command(&["MULTI"])? != json!("OK") {
            return Err("清理 MULTI 未确认，保留现场".into());
        }
        let mut unlink = vec!["UNLINK"];
        unlink.extend(self.comparison.keys.iter().map(String::as_str));
        unlink.push(&self.owner_key);
        if client.transaction_command(&unlink)? != json!("QUEUED") {
            return Err("精确清理未排队，保留现场".into());
        }
        let result = client.transaction_command(&["EXEC"])?;
        if result
            .as_array()
            .is_none_or(|rows| rows.len() != 1 || rows[0].as_u64().is_none())
            || !names(&mut client, &self.namespace)?.is_empty()
        {
            return Err("精确清理结果未知或竞争；保留现场，不重试".into());
        }
        drop(client);
        self.verify_configuration()?;
        self.owned.remove()?;
        Ok(())
    }
}

fn names(client: &mut Client, namespace: &str) -> Result<BTreeSet<String>> {
    let pattern = format!("{namespace}*");
    let mut found = BTreeSet::new();
    let mut cursors = BTreeSet::new();
    let mut cursor = "0".to_owned();
    for _ in 0..10000 {
        let value =
            client.transaction_command(&["SCAN", &cursor, "MATCH", &pattern, "COUNT", "1000"])?;
        let rows = value
            .as_array()
            .filter(|rows| rows.len() == 2)
            .ok_or("真实 Redis SCAN shape 无效")?;
        let next = rows[0].as_str().ok_or("真实 Redis SCAN cursor 无效")?;
        for key in rows[1].as_array().ok_or("真实 Redis SCAN keys 无效")? {
            let key = key.as_str().ok_or("真实 Redis SCAN key 无效")?;
            if !key.starts_with(namespace) || !found.insert(key.to_owned()) {
                return Err("真实 Redis SCAN 越界或重复，保留现场".into());
            }
        }
        if next == "0" {
            return Ok(found);
        }
        if !cursors.insert(next.to_owned()) {
            return Err("真实 Redis SCAN cursor 重复，保留现场".into());
        }
        cursor = next.to_owned();
    }
    Err("真实 Redis SCAN 超限，保留现场".into())
}

fn comparison(namespace: &str) -> Result<cache_io::Comparison> {
    let (example, data) = cache_fixture();
    let plan = super::super::super::model::cache_plan(
        &json!({"namespace":namespace,"subjects":example["subjects"]}),
    )?;
    let template = document::parse_fragment(
        data["raw"][1]["payload"]
            .as_str()
            .ok_or("夹具 payload 缺失")?,
        "固定非敏感快照",
    )?;
    let mut raw = Vec::new();
    for subject in plan.as_array().ok_or("夹具 plan 缺失")? {
        let mut payload = template.clone();
        payload["principal"]["actor"]["tenant_id"] = subject["tenant_id"].clone();
        payload["principal"]["actor"]["user_id"] = subject["user_id"].clone();
        raw.push(
            json!({"types":{"tenant_epoch":"string","user_version":"string","snapshot":"hash"},
            "tenant_epoch":"2","user_version":"1","fields":["2:1"],"payload":payload.to_string()}),
        );
    }
    let observed = cache_io::observation(&json!({"plan":plan,"raw":raw}))?;
    cache_io::comparison(
        &json!({"observation":{"namespace":namespace,"subjects":example["subjects"],
        "plan":plan,"before":observed["before"]},"comparisons":observed["comparisons"]}),
    )
}

fn image(client: &mut Client, plan: &cache_io::Comparison) -> Result<Vec<Value>> {
    plan.keys
        .iter()
        .map(|key| {
            let kind = client.transaction_command(&["TYPE", key])?;
            let body = match kind.as_str() {
                Some("string") => client.transaction_command(&["GET", key])?,
                Some("hash") => client.transaction_command(&["HGETALL", key])?,
                Some("none") => Value::Null,
                _ => return Err("真实 Redis 前后像出现未知类型，保留现场".into()),
            };
            Ok(json!({"key":key,"type":kind,"body":body}))
        })
        .collect()
}

fn expire(client: &mut Client, key: &str) -> Result<()> {
    if client.transaction_command(&["PEXPIRE", key, "50"])? != json!(1) {
        return Err("TTL 注入未命中原键，真实场景未验收".into());
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if client.transaction_command(&["TYPE", key])? == json!("none") {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(5));
    }
    Err("TTL 未实际到期，真实场景未验收".into())
}

fn exercise(case: &str) -> Result<()> {
    let fixture = Fixture::open(case)?;
    let mut writer = fixture.connect()?;
    let mut observer = fixture.connect()?;
    fixture.populate(&mut observer)?;
    let before = image(&mut observer, &fixture.comparison)?;
    common::write_new(&fixture.directory.join("before.json"), &json!(before))?;
    let mut execs = 0;
    let mut unlinks = 0;
    let result = cache_transaction::execute(&fixture.comparison, &mut |parts| {
        fixture.verify_configuration()?;
        if parts[0] == "UNLINK" {
            unlinks += 1;
        }
        if parts == ["EXEC"] {
            execs += 1;
            fixture.require_owner(&mut observer)?;
            match case {
                "aba" => {
                    let key = &fixture.comparison.keys[0];
                    if observer.transaction_command(&["SET", key, "3"])? != json!("OK")
                        || observer.transaction_command(&[
                            "SET",
                            key,
                            &fixture.comparison.values[0],
                        ])? != json!("OK")
                    {
                        return Err("ABA 注入失败，真实场景未验收".into());
                    }
                    if image(&mut observer, &fixture.comparison)? != before {
                        return Err("ABA 未恢复完整原像，真实场景未验收".into());
                    }
                }
                "ttl" => expire(&mut observer, &fixture.comparison.keys[2])?,
                // 明确额外排队命令以丢弃回复；不声称正式 source 固定命令中包含此注入。
                "lost-reply" => {
                    if writer.transaction_command(&["CLIENT", "REPLY", "OFF"])? != json!("QUEUED") {
                        return Err("CLIENT REPLY OFF 未排队，真实场景未验收".into());
                    }
                }
                _ => {}
            }
        }
        writer.transaction_command(parts)
    });
    drop(writer);
    fixture.require_owner(&mut observer)?;
    let after = image(&mut observer, &fixture.comparison)?;
    common::write_new(&fixture.directory.join("after.json"), &json!(after))?;
    common::write_new(
        &fixture.directory.join("result.json"),
        &json!({"outcome":result,"execs":execs,"queued_unlinks":unlinks,
        "test_only_reply_off":case=="lost-reply"}),
    )?;
    if execs != 1 || unlinks != 1 || result["automatic_retry_permitted"] != false {
        return Err("真实 execute 未保持单次 EXEC/UNLINK，保留现场".into());
    }
    verify(case, &before, &after, &result)?;
    fixture.finish(observer, &after)
}

fn verify(case: &str, before: &[Value], after: &[Value], result: &Value) -> Result<()> {
    let absent = after
        .iter()
        .all(|row| row["type"] == "none" && row["body"].is_null());
    let valid = match case {
        "success" => {
            result["status"] == "confirmed"
                && result["deleted"] == 33
                && result["expired"] == json!([])
                && result["remote_write_may_have_occurred"] == true
                && absent
        }
        "lost-reply" => {
            result["status"] == "unknown_exec_result"
                && result["deleted"].is_null()
                && result["remote_write_may_have_occurred"] == true
                && absent
        }
        "aba" => {
            result["status"] == "watch_conflict"
                && result["remote_write_may_have_occurred"] == false
                && result["deleted"].is_null()
                && before == after
        }
        "ttl" => {
            let mut expected = before.to_vec();
            expected[2]["type"] = json!("none");
            expected[2]["body"] = Value::Null;
            result["status"] == "watch_conflict"
                && result["remote_write_may_have_occurred"] == false
                && result["deleted"].is_null()
                && expected == after
        }
        _ => false,
    };
    if !valid {
        return Err("真实 Redis 分类或完整前后像不符，保留现场；不重试、不推断回滚".into());
    }
    Ok(())
}

#[test]
#[ignore = "需要主控独占真实隔离 Redis 文件配置；不运行即未验收"]
fn real_source_cache_exec_confirms_complete_33_key_cleanup() {
    exercise("success").expect("真实 Redis 33 键清理尚未验收");
}

#[test]
#[ignore = "需要主控独占真实隔离 Redis 文件配置；不运行即未验收"]
fn real_source_cache_aba_aborts_once_without_unlink() {
    exercise("aba").expect("真实 Redis ABA 冲突尚未验收");
}

#[test]
#[ignore = "需要主控独占真实隔离 Redis 文件配置；不运行即未验收"]
fn real_source_cache_expiration_aborts_once_without_unlink() {
    exercise("ttl").expect("真实 Redis WATCH 到期冲突尚未验收");
}

#[test]
#[ignore = "需要主控独占真实隔离 Redis 和 CLIENT REPLY 权限；不运行即未验收"]
fn real_source_cache_lost_reply_preserves_unknown_after_actual_deletion() {
    exercise("lost-reply").expect("真实 Redis EXEC 丢回复尚未验收");
}
