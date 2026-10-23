use super::{
    batch, documents,
    model::{self, Inputs},
    session::{self, Client},
};
use crate::Result;
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

pub(super) async fn run(inputs: &Inputs) -> Result<Value> {
    inputs.unchanged()?;
    let plan = &inputs.plan;
    let directory = model::path(&plan["work_dir"])?.join("dataset");
    crate::local_test_path::validate_local_test_path(
        &directory,
        &inputs.root,
        crate::local_test_path::LocalTestPathKind::NewDirectory,
    )?;
    fs::create_dir(&directory)?;
    let catalog = session::catalog(&inputs.root)?;
    let started = chrono::Utc::now().to_rfc3339();
    let mut clients = create_clients(inputs, &directory, &catalog).await?;
    let failure = AtomicBool::new(false);
    let prepared =
        futures_util::future::join_all(clients.iter_mut().enumerate().map(|(index, client)| {
            let failure = &failure;
            let directory = &directory;
            async move {
                let result = prepare_posts(client, plan, index, directory, failure).await;
                if result.is_err() {
                    failure.store(true, Ordering::SeqCst);
                }
                result
            }
        }))
        .await;
    let mut tenants = Vec::new();
    for result in prepared {
        tenants.push(result?);
    }
    inputs.unchanged()?;
    let batch = batch::run(inputs, &directory)?;
    for sample in batch["samples"].as_array().ok_or("岗位批次样本缺失")? {
        let index = sample["identityIndex"]
            .as_u64()
            .filter(|v| *v < 11)
            .ok_or("岗位批次租户索引无效")? as usize;
        tenants[index]["posts"]
            .as_array_mut()
            .ok_or("岗位样本缺失")?
            .push(json!({"id":sample["id"],"code":sample["code"],"name":sample["name"]}));
    }
    let object_bytes = upload_objects(inputs, &directory, &mut clients, &mut tenants).await?;
    inputs.unchanged()?;
    let settings = &plan["dataset"];
    let result = json!({"format_version":1,"plan_sha256":model::hash(plan)?,"source_scope_id":plan["source"]["scope_id"],"started_at":started,"completed_at":chrono::Utc::now().to_rfc3339(),"records":settings["records"],"object_bytes":object_bytes,"tenants":tenants,"request_interval_ms":settings["request_interval_ms"],"post_concurrency":11,"post_batch":batch["result"]});
    documents::write_new(&directory.join("result.json"), &result)?;
    Ok(result)
}
async fn create_clients(
    inputs: &Inputs,
    directory: &Path,
    catalog: &std::collections::BTreeMap<String, Value>,
) -> Result<Vec<Client>> {
    let plan = &inputs.plan;
    let initial = inputs
        .initial_password
        .as_deref()
        .ok_or("fresh 数据准备缺少初始化密码")?;
    let mut admin = Client::new(plan, "source", 0, catalog, initial)?;
    admin.activate(inputs, 0, directory).await?;
    let version = plan_version(&mut admin, plan).await?;
    let mut clients = vec![admin];
    for index in 1..11 {
        inputs.unchanged()?;
        let body = json!({"tenant_id":model::tenant(plan,index),"name":format!("恢复参考租户{index}"),"admin_username":"owner","admin_password":initial,"data_target_key":plan["dataset"]["tenant_targets"][index-1],"plan_version_id":version["id"],"max_users":20,"max_roles":10,"max_storage_mb":512,"max_requests_per_min":60_000});
        clients[0]
            .request(&json!({"operation":"post_platform_tenants","body":body}))
            .await?;
        record(
            directory,
            &json!({"kind":"tenant","tenant_id":model::tenant(plan,index)}),
        )?;
        let mut client = Client::new(plan, "source", index, catalog, initial)?;
        client.activate(inputs, index, directory).await?;
        clients.push(client);
    }
    Ok(clients)
}
async fn upload_objects(
    inputs: &Inputs,
    directory: &Path,
    clients: &mut [Client],
    tenants: &mut [Value],
) -> Result<u64> {
    let plan = &inputs.plan;
    for client in clients.iter_mut() {
        client
            .request(&json!({"operation":"post_auth_refresh"}))
            .await?;
    }
    let settings = &plan["dataset"];
    let count = settings["object_count"].as_u64().ok_or("对象数缺失")?;
    let bytes = settings["object_bytes"].as_u64().ok_or("对象大小缺失")? as usize;
    let mut sample = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(directory.join("upload.txt"))?;
    for index in 0..count {
        inputs.unchanged()?;
        let content = sample_content(plan["id"].as_str().ok_or("数据集ID缺失")?, index, bytes);
        let sha = model::sha(&content);
        use std::io::{Seek, SeekFrom};
        sample.seek(SeekFrom::Start(0))?;
        sample.set_len(0)?;
        sample.write_all(&content)?;
        sample.sync_all()?;
        let owner = index as usize % 11;
        let response = clients[owner]
            .upload(
                format!("{}-{index}.txt", plan["id"].as_str().unwrap_or("")),
                content,
            )
            .await?;
        let files = response["data"]
            .as_array()
            .filter(|v| v.len() == 1)
            .ok_or("上传没有返回唯一文件登记")?;
        let mut file = files[0].clone();
        if file["file_path"].as_str().is_none_or(str::is_empty)
            || file["file_id"].as_str().is_none_or(str::is_empty)
        {
            return Err("上传未返回文件登记".into());
        }
        file["bytes"] = json!(bytes);
        file["sha256"] = json!(sha);
        let mut entry = file.clone();
        entry["kind"] = json!("file");
        entry["tenant_id"] = json!(model::tenant(plan, owner));
        record(directory, &entry)?;
        tenants[owner]["files"]
            .as_array_mut()
            .ok_or("对象样本缺失")?
            .push(file);
    }
    Ok(count * bytes as u64)
}
pub(super) async fn plan_version(admin: &mut Client, plan: &Value) -> Result<Value> {
    let capabilities = admin
        .request(&json!({"operation":"get_platform_capabilities"}))
        .await?;
    if !capabilities["data"].is_array() {
        return Err("能力目录响应必须是数组".into());
    }
    let product=admin.request(&json!({"operation":"post_platform_product_plans","body":{"key":plan["id"],"name":format!("恢复参考{}",plan["id"].as_str().unwrap_or(""))}})).await?;
    let version=admin.request(&json!({"operation":"post_platform_product_plans_by_plan_id_versions","path":{"plan_id":product["data"]["id"]},"body":{"name":"恢复参考版本","capabilities":[]}})).await?;
    admin.request(&json!({"operation":"post_platform_product_plans_by_plan_id_versions_by_version_publish","path":{"plan_id":product["data"]["id"],"version":version["data"]["version"]}})).await?;
    Ok(version["data"].clone())
}
async fn prepare_posts(
    client: &mut Client,
    plan: &Value,
    index: usize,
    directory: &Path,
    failure: &AtomicBool,
) -> Result<Value> {
    let identity = model::identity(plan, index);
    let api = plan["dataset"]["api_validation_posts"]
        .as_u64()
        .ok_or("API记录数缺失")?;
    client
        .request(&json!({"operation":"post_auth_refresh"}))
        .await?;
    let posts = seed_posts(
        client,
        plan["id"].as_str().ok_or("数据集ID缺失")?,
        &model::tenant(plan, index),
        api,
        directory,
        failure,
    )
    .await?;
    let count = model::count(
        plan["dataset"]["records"].as_u64().ok_or("记录数缺失")?,
        index,
    );
    Ok(
        json!({"tenant_id":identity["tenant_id"],"username":identity["username"],"records":count,"api_records":api,"batch_records":count-api,"posts":posts,"files":[]}),
    )
}
pub(super) async fn seed_posts(
    client: &mut Client,
    id: &str,
    tenant: &str,
    count: u64,
    directory: &Path,
    failure: &AtomicBool,
) -> Result<Vec<Value>> {
    let mut samples = Vec::new();
    for index in 0..count {
        if failure.load(Ordering::SeqCst) {
            return Err("其他参考租户准备失败，停止新增请求".into());
        }
        if index > 0 && index % 100 == 0 {
            client
                .request(&json!({"operation":"post_auth_refresh"}))
                .await?;
        }
        let code = format!("{id}-{index}");
        let response=client.request(&json!({"operation":"post_system_posts","body":{"name":format!("恢复样本{index}"),"code":code,"sort":index%1000}})).await?;
        let post = &response["data"];
        model::positive_id(&post["id"])?;
        if post["code"] != code {
            return Err("岗位创建结果没有对应当前数据集".into());
        }
        record(
            directory,
            &json!({"kind":"post","tenant_id":tenant,"id":post["id"],"code":code}),
        )?;
        if index < 2 || index == count - 1 {
            samples.push(json!({"id":post["id"],"code":code,"name":post["name"]}));
        }
    }
    let page = client
        .request(
            &json!({"operation":"get_system_posts","query":{"code":id,"page":1,"page_size":1}}),
        )
        .await?;
    if page["data"]["total"] != count {
        return Err("岗位数量与本次精确编码前缀不一致".into());
    }
    Ok(samples)
}
fn record(directory: &Path, value: &Value) -> Result<()> {
    let mut stream = OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join("created.ndjson"))?;
    serde_json::to_writer(&mut stream, value)?;
    stream.write_all(b"\n")?;
    stream.sync_all()?;
    Ok(())
}
pub(super) fn sample_content(id: &str, index: u64, bytes: usize) -> Vec<u8> {
    let mut output = Vec::with_capacity(bytes);
    let mut block = 0;
    while output.len() < bytes {
        let line = format!(
            "{}\n",
            model::sha(format!("{id}:{index}:{block}").as_bytes())
        );
        output.extend_from_slice(&line.as_bytes()[..(bytes - output.len()).min(line.len())]);
        block += 1;
    }
    output
}
