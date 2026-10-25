//! 五桶完整空前像和原生签名读收据；凭据只从已核验文件配置取得。
use super::super::super::{
    digest,
    export::transport::Transport,
    input::BUCKETS,
    json as document,
    s3::{Configuration, Operation, Request, app_configuration},
};
use super::super::{input::request_configuration, storage::StorageInputs};
use crate::Result;
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub(super) fn absence(inputs: &StorageInputs<'_>, transport: &impl Transport) -> Result<Value> {
    let mut result = serde_json::Map::new();
    for bucket in BUCKETS.into_iter().collect::<BTreeSet<_>>() {
        observe(
            inputs,
            transport,
            bucket,
            Operation::HeadBucket,
            "bucket-readiness",
            None,
        )?;
        let version = observe(
            inputs,
            transport,
            bucket,
            Operation::Versioning,
            "bucket-versioning",
            None,
        )?;
        if !version["Status"].is_null() {
            return Err("fresh 初始化不允许已启用版本的对象桶".into());
        }
        let scope = document::text(&inputs.request["target"]["scope_id"], "对象 scope")?;
        let prefix = format!("{scope}/");
        let page = observe(
            inputs,
            transport,
            bucket,
            Operation::List {
                prefix: prefix.clone(),
                continuation: None,
            },
            "scoped-list",
            Some(prefix),
        )?;
        // 空范围必须由服务明确完成；截断页不是空范围证明，也不能自动重试。
        if page["IsTruncated"] != false || page.get("Contents").is_some_and(|v| v != &json!([])) {
            return Err("目标 scoped 对象范围非空或分页未明确完成，保留现场".into());
        }
        result.insert(bucket.into(), json!({"keys":[]}));
    }
    Ok(Value::Object(result))
}

fn observe(
    inputs: &StorageInputs<'_>,
    transport: &impl Transport,
    bucket: &str,
    operation: Operation,
    stage: &str,
    prefix: Option<String>,
) -> Result<Value> {
    let root = inputs.execution_backend()?;
    let (directory, files) =
        request_configuration::fresh_files(&root, inputs.environment, inputs.request)?;
    let binding = app_configuration::configuration_value(&root, &directory, "test")?;
    let mut snapshots = Vec::new();
    let table = app_configuration::load(&root, &binding, &mut snapshots)?;
    let private = serde_json::to_value(table.get("object_storage").ok_or("绑定文件缺少对象配置")?)?;
    let expected = &inputs.request["target"]["s3"];
    for key in ["endpoint", "region", "access_key", "secret_key"] {
        if private[key] != expected[key] {
            return Err("对象文件配置不同于固定请求".into());
        }
    }
    let configuration = Configuration::read(
        &json!({"endpoint":private["endpoint"],"region":private["region"],
        "access_key":private["access_key"],"secret_key":private["secret_key"]}),
    )?;
    let operation_name = match &operation {
        Operation::HeadBucket => "head-bucket",
        Operation::Versioning => "get-bucket-versioning",
        Operation::List { .. } => "list-objects-v2",
        _ => return Err("prepare 拒绝非只读对象操作".into()),
    };
    let path = inputs.observation_dir.join(format!(
        "{stage}-{}.command.json",
        uuid::Uuid::new_v4().simple()
    ));
    let result = transport.s3(Request {
        configuration,
        bucket: bucket.into(),
        operation,
    });
    let error = result.as_ref().err().map(|_| "NativeS3Error");
    document::write_new(
        &path,
        &json!({"transport":"rust-s3-sigv4","operation":operation_name,"bucket":bucket,
        "returncode":if error.is_none() {Some(0)} else {None},"error_type":error,"response":result.as_ref().ok(),
        "key":null,"destination":null,"remote_writes":0,"scope_id":inputs.request["target"]["scope_id"],
        "endpoint":expected["endpoint"],"configuration_sha256":digest::plan_hash(expected)?,"prefix":prefix,"continuation":null}),
    )?;
    app_configuration::unchanged(&snapshots)?;
    if request_configuration::fresh_files(&root, inputs.environment, inputs.request)?
        != (directory, files)
    {
        return Err("对象观察期间绑定文件配置变化".into());
    }
    result.map_err(|_| "原生对象只读观察失败；检查绑定文件、权限与服务，保留现场显式续作".into())
}
