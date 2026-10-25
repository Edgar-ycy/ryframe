use super::{Result, binding::Inputs, native, text};
use native::{
    export::transport::{CommandOutput, CommandSpec, Transport},
    target_verify::database::evidence::{self, stdout::VerifiedStdout},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Duration};

pub(super) struct Collector<'a, T> {
    pub(super) inputs: &'a Inputs,
    pub(super) transport: &'a T,
}

impl<T: Transport> Collector<'_, T> {
    pub(super) fn query(&self, db: &Value, check: &str) -> Result<String> {
        let mut receipts = Vec::new();
        let first = self.attempt(db, check, &mut receipts)?;
        if !first.stdout.is_empty() || !first.stderr.is_empty() {
            return response(&first);
        }
        let second = self.attempt(db, check, &mut receipts);
        // 第二次失败也保留关系；attempt 在错误前已发布该次收据。
        let receipt = self.inputs.work.join(format!(
            "mysql-{check}-retry-{}.json",
            uuid::Uuid::new_v4().simple()
        ));
        if receipts.len() == 2 {
            native::json::write_new(
                &receipt,
                &json!({"format_version":1,
                    "kind":"mysql-verification-retry","check":check,"target_key":db["key"],
                    "reason":"first_returncode_zero_stdout_stderr_exactly_empty",
                    "first":receipts[0],"second":receipts[1]}),
            )?;
        }
        let output = second?;
        if receipts.len() != 2 {
            return Err("MySQL 重试缺少两次原始输出收据".into());
        }
        if output.stdout.is_empty() {
            return Err("MySQL 固定核验第二次仍为空；保留两次输出，禁止继续重试".into());
        }
        response(&output)
    }

    fn attempt(&self, db: &Value, check: &str, receipts: &mut Vec<Value>) -> Result<CommandOutput> {
        let sql = evidence::verification_sql(check)?;
        self.inputs.unchanged()?;
        let name = format!("mysql-{check}-{}", uuid::Uuid::new_v4().simple());
        let mut stdout = VerifiedStdout::create(&self.inputs.work.join(format!("{name}.stdout")))?;
        let mut args = arguments(db)?;
        args.extend(["--execute".into(), sql.into()]);
        let result = self.transport.run(self.spec(args, None, stdout.target()?)?);
        let snapshot = stdout.snapshot();
        let mut receipt = evidence::mysql_receipt(db, check, &stdout, &result, &snapshot);
        if result.as_ref().is_ok_and(|output| output.status.success())
            && snapshot
                .as_ref()
                .is_ok_and(|snapshot| std::str::from_utf8(&snapshot.raw).is_err())
        {
            receipt["error_type"] = json!("UnicodeDecodeError");
        }
        let receipt_path = self.inputs.work.join(format!("{name}.json"));
        native::json::write_new(&receipt_path, &receipt)?;
        let binding = native::resources::common::binding(&receipt_path)?;
        receipts.push(binding);
        self.inputs.unchanged()?;
        let mut output = result.map_err(|_| "MySQL 只读观察执行失败；保留部分原始输出和收据")?;
        output.stdout = snapshot?.raw;
        if !output.status.success() {
            return Err("MySQL 只读观察返回非零状态；保留原始输出和收据，不自动重试".into());
        }
        Ok(output)
    }

    pub(super) fn count(&self, db: &Value) -> Result<String> {
        self.inputs.unchanged()?;
        let result = self.transport.run(self.spec(
            arguments(db)?,
            Some(b"SELECT COUNT(*) FROM sys_tenant WHERE tenant_id <> 'system';".to_vec()),
            native::export::transport::StdoutTarget::Capture,
        )?);
        self.inputs.unchanged()?;
        let output = result.map_err(|_| "源租户数量只读查询失败；禁止重试")?;
        if !output.status.success() {
            return Err("源租户数量只读查询返回非零状态".into());
        }
        response(&output)
    }

    fn spec(
        &self,
        args: Vec<String>,
        stdin: Option<Vec<u8>>,
        stdout: native::export::transport::StdoutTarget,
    ) -> Result<CommandSpec> {
        let mut environment = std::env::vars().collect::<BTreeMap<_, _>>();
        environment.remove("MYSQL_PWD");
        environment.insert(
            "MYSQL_TEST_LOGIN_FILE".into(),
            self.inputs
                .work
                .join("unused-login.cnf")
                .display()
                .to_string(),
        );
        Ok(CommandSpec {
            program: self.inputs.program()?,
            args,
            cwd: self.inputs.backend.clone(),
            environment,
            stdin,
            stdout,
            work_dir: self.inputs.work.clone(),
            label: "参考数据库只读核验".into(),
            timeout: Duration::from_secs(1800),
        })
    }
}

fn arguments(db: &Value) -> Result<Vec<String>> {
    Ok(vec![
        format!("--defaults-file={}", text(&db["defaults_file"])?),
        format!("--database={}", text(&db["database"])?),
        "--batch".into(),
        "--raw".into(),
        "--skip-column-names".into(),
        "--binary-mode".into(),
        "--default-character-set=utf8mb4".into(),
    ])
}

fn response(output: &CommandOutput) -> Result<String> {
    Ok(std::str::from_utf8(&output.stdout)
        .map_err(|_| "MySQL 核验输出不是 UTF-8")?
        .trim()
        .to_owned())
}
