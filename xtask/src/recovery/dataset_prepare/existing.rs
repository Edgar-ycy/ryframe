use super::{
    batch, documents,
    model::{self, Inputs},
    session::{self, Client},
};
use crate::{Result, check::consumer_context::python};
use serde_json::{Value, json};
use std::time::Duration;

pub(super) fn ownership(inputs: &Inputs) -> Result<()> {
    let request = json!({"format_version":1,"kind":"ryframe-xtask-recovery-reference","request":{"backend_dir":inputs.root,"format_version":1,"operation":"check-existing","plan":inputs.plan_path,"side":inputs.side,"write":false}});
    let program = python::selected_program(&inputs.root)?;
    let command = python::command(
        &inputs.root,
        &program,
        &[
            "-X",
            "utf8",
            "-B",
            "tools/python/restore_reference.py",
            "--stdin",
        ],
    );
    let output = python::output(
        &inputs.root,
        batch::scrub(command),
        Some(&serde_json::to_vec(&request)?),
        Duration::from_secs(60),
    )?;
    if !output.status.success() {
        return Err("已有数据ownership或运行进程核验失败；没有业务写入，请检查参考阶段收据".into());
    }
    let result =
        documents::parse_fragment(std::str::from_utf8(&output.stdout)?, "ownership核验结果")?;
    if result["plan_sha256"] != model::hash(&inputs.plan)?
        || result["side"] != inputs.side
        || result["scope_id"] != inputs.plan[&inputs.side]["scope_id"]
    {
        return Err("参考计划或核验侧在ownership检查期间变化".into());
    }
    inputs.unchanged()
}
pub(super) async fn run(inputs: &Inputs) -> Result<Value> {
    let dataset = inputs.dataset.as_ref().ok_or("已有数据收据缺失")?;
    let catalog = session::catalog(&inputs.root)?;
    if catalog
        .get("get_system_posts_by_id")
        .is_none_or(|v| v["method"] != "GET")
    {
        return Err("旧岗位验收必须使用只读查询契约".into());
    }
    let mut posts = 0u64;
    let mut files = 0u64;
    for identity in dataset["tenants"].as_array().ok_or("租户样本缺失")? {
        inputs.unchanged()?;
        let index = (0..11)
            .find(|i| identity["tenant_id"] == model::tenant(&inputs.plan, *i))
            .ok_or("租户归属不符")?;
        let mut client = Client::new(
            &inputs.plan,
            &inputs.side,
            index,
            &catalog,
            &inputs.password,
        )?;
        client.login().await?;
        let result = verify_identity(&mut client, identity).await;
        let logout = client
            .request(&json!({"operation":"post_auth_logout"}))
            .await;
        let counts = match (result, logout) {
            (Ok(counts), Ok(_)) => counts,
            (Err(_), Err(_)) => return Err("已有数据验证及注销均失败；不发布验证成功".into()),
            (Err(error), _) | (_, Err(error)) => return Err(error),
        };
        posts += counts.0;
        files += counts.1;
    }
    Ok(
        json!({"format_version":1,"status":"existing_data_verified","side":inputs.side,"scope_id":inputs.plan[&inputs.side]["scope_id"],"plan_sha256":model::hash(&inputs.plan)?,"source_scope_id":dataset["source_scope_id"],"dataset_sha256":inputs.dataset_sha()?,"actions":{"business":"read_only","objects":"read_only","session":"login_logout"},"restore_success":false,"tenants":11,"posts":posts,"files":files}),
    )
}
async fn verify_identity(client: &mut Client, identity: &Value) -> Result<(u64, u64)> {
    let posts = identity["posts"].as_array().ok_or("岗位样本缺失")?;
    for post in posts {
        let response = client
            .request(&json!({"operation":"get_system_posts_by_id","path":{"id":post["id"]}}))
            .await?;
        if response["data"]["code"] != post["code"] || response["data"]["name"] != post["name"] {
            return Err("已有岗位内容不一致".into());
        }
    }
    let files = identity["files"].as_array().ok_or("对象样本缺失")?;
    for file in files {
        client.download(file).await?;
    }
    Ok((posts.len() as u64, files.len() as u64))
}
