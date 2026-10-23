use super::{Client, model};
use crate::{
    Result,
    recovery::dataset_prepare::{documents, model::Inputs},
};
use serde_json::{Value, json};
use std::path::Path;

impl Client {
    /// 只有 fresh 准备显式调用；遗留意图即停止，不探测或重放密码写入。
    pub(in crate::recovery::dataset_prepare) async fn activate(
        &mut self,
        inputs: &Inputs,
        index: usize,
        directory: &Path,
    ) -> Result<()> {
        let intent_path = directory.join(format!("activation-{index:02}.intent.json"));
        let confirmed_path = directory.join(format!("activation-{index:02}.confirmed.json"));
        let completed_path = directory.join(format!("activation-{index:02}.completed.json"));
        for path in [&intent_path, &confirmed_path, &completed_path] {
            crate::local_test_path::validate_local_test_path(
                path,
                &inputs.root,
                crate::local_test_path::LocalTestPathKind::OutputFile,
            )?;
        }
        inputs.unchanged()?;
        let initial = inputs
            .initial_password
            .as_deref()
            .ok_or("首次激活缺少初始化密码")?;
        let expected = model::identity(&inputs.plan, index);
        if self.identity["tenant_id"] != expected["tenant_id"]
            || self.identity["username"] != expected["username"]
            || self.identity["password"] != initial
        {
            return Err("首次激活身份或初始化凭据不符".into());
        }
        self.session.login(&mut self.pacer).await?;
        let before = self.context(true).await?;
        let subject = model::positive_id(&before["user"]["id"])?;
        let intent = json!({"format_version":1,"kind":"dataset-password-intent",
            "plan_sha256":model::hash(&inputs.plan)?,"scope_id":inputs.plan["source"]["scope_id"],
            "tenant_id":self.identity["tenant_id"],"username":self.identity["username"],
            "subject_id":subject,"administrator_credentials":inputs.plan["dataset"]["administrator_credentials"],
            "operation":"put_auth_profile_password"});
        inputs.unchanged()?;
        documents::write_new(&intent_path, &intent)?;
        self.request(&json!({"operation":"put_auth_profile_password",
            "body":{"old_password":initial,"new_password":inputs.password}}))
            .await?;
        inputs.unchanged()?;
        if documents::read(&intent_path)? != intent {
            return Err("改密意图发生变化，保留未知写入现场".into());
        }
        let confirmed = json!({"format_version":1,"kind":"dataset-password-confirmed",
            "intent_sha256":model::hash(&intent)?,"subject_id":subject});
        documents::write_new(&confirmed_path, &confirmed)?;
        self.replace_session(&inputs.password)?;
        self.session.login(&mut self.pacer).await?;
        let after = self.context(false).await?;
        if after["user"]["id"] != subject {
            return Err("改密后会话主体变化".into());
        }
        self.authorized(&after)?;
        inputs.unchanged()?;
        if documents::read(&intent_path)? != intent
            || documents::read(&confirmed_path)? != confirmed
        {
            return Err("改密确认收据发生变化，禁止发布激活成功".into());
        }
        documents::write_new(
            &completed_path,
            &json!({"format_version":1,
            "kind":"dataset-password-activated","intent_sha256":model::hash(&intent)?,
            "confirmed_sha256":model::hash(&confirmed)?,"subject_id":subject}),
        )
    }

    pub(super) async fn context(&mut self, required: bool) -> Result<Value> {
        let response = self
            .request(&json!({"operation":"get_auth_context"}))
            .await?;
        let context = &response["data"];
        let id = model::positive_id(&context["user"]["id"])?;
        if id.parse::<i64>()?.to_string() != id {
            return Err("会话主体ID不是规范正整数串".into());
        }
        if context["user"]["tenant_id"] != self.identity["tenant_id"]
            || context["user"]["username"] != self.identity["username"]
            || context["password_change_required"].as_bool() != Some(required)
        {
            return Err("数据集会话身份或首次改密状态不符；已有数据核验不会自动改密".into());
        }
        Ok(context.clone())
    }

    fn replace_session(&mut self, password: &str) -> Result<()> {
        self.identity["password"] = json!(password);
        let config = json!({"bindings":{"api_url":self.api.as_str(),"frontend_url":self.origin},
            "contract":{"timeout_ms":120_000}});
        self.session =
            crate::http_session::Session::new(&config, self.catalog.clone(), &self.identity)?;
        Ok(())
    }

    fn authorized(&self, context: &Value) -> Result<()> {
        let common = ["system:post:add", "system:post:list"];
        let platform = [
            "tenant:add",
            "platform:product-plan:list",
            "platform:product-plan:add",
            "platform:product-plan:publish",
        ];
        let required = common.iter().chain(
            platform
                .iter()
                .filter(|_| self.identity["tenant_id"] == "system"),
        );
        let permissions = context["permissions"]
            .as_array()
            .ok_or("会话权限集合缺失")?;
        if !context["is_super_admin"].is_boolean()
            || permissions.iter().any(|value| !value.is_string())
        {
            return Err("会话权限集合无效".into());
        }
        if context["is_super_admin"] != true
            && !permissions.contains(&json!("*"))
            && !required
                .into_iter()
                .all(|permission| permissions.contains(&json!(permission)))
        {
            return Err("改密后管理员缺少参考数据准备所需权限".into());
        }
        Ok(())
    }
}
