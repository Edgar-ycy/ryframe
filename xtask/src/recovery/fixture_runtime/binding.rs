use super::{
    dependency::Dependencies,
    model::*,
    plan::{self, Context, Paths, SourceGuard, input_guards, output_set, outputs, runtime_args},
    publication,
};
use crate::{Result, recovery::clone::native::resources::common};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf};
pub(super) struct Base {
    boot: Value,
    bootstrap_file: PathBuf,
    execution: PathBuf,
    runtime: PathBuf,
    verified: Value,
    frontend: PathBuf,
    bootstrap: Value,
    review: Value,
    review_file: PathBuf,
    settings: Value,
    api_url: String,
    frontend_url: String,
}
impl Base {
    pub(super) fn load(
        api: &mut impl Dependencies,
        paths: &Paths,
        closed: bool,
        read_only: bool,
    ) -> Result<Self> {
        let boot = api.call(
            "bootstrap",
            json!({"backend":paths.backend,"environment":paths.environment}),
        )?;
        let bootstrap_file = path(&boot["bootstrap"])?;
        let execution = path(&boot["execution"])?;
        let runtime = paths.output.clone();
        directory(&runtime)?;
        check(
            runtime.starts_with(execution.join(".local-tests/reference-fixture"))
                && runtime != execution.join(".local-tests/reference-fixture"),
            "浏览器运行目录越界",
        )?;
        let verified = api.call("verify", runtime_args(paths))?;
        let mut args = runtime_args(paths);
        args["action"] = json!("status");
        let status = api.call(if read_only { "observe" } else { "control" }, args)?;
        let state = if closed { "stopped" } else { "running" };
        check(
            status["processes"] == json!({"api":state,"worker":state}),
            "浏览器核验要求API与Worker处于预期状态",
        )?;
        let build = &verified["build_evidence"];
        let src = &build["source"];
        check(
            src["fixture"] == "business"
                && !src["original"]["frontend"].is_null()
                && !src["generated"]["frontend"].is_null(),
            "浏览器构建缺少完整前端来源",
        )?;
        let frontend = path(&src["roots"]["frontend"])?;
        check(
            frontend.join("package.json").is_file(),
            "浏览器前端工作树无效",
        )?;
        let (bootstrap, _) = document(&bootstrap_file)?;
        let review_file = path(&bootstrap["plan"]["review"]["path"])?;
        check(
            review_file.starts_with(paths.backend.join(".local-tests")),
            "浏览器审阅计划路径越界",
        )?;
        descriptor(&bootstrap["plan"]["review"], &review_file)?;
        let (review, _) = document(&review_file)?;
        let side = text(&bootstrap["plan"]["side"])?;
        check(
            ["seed", "base", "candidate"].contains(&side),
            "浏览器审阅侧无效",
        )?;
        let selected = &review["scopes"][side];
        let settings = api.call(
            "settings",
            json!({"bootstrap":bootstrap_file,"execution":execution}),
        )?;
        let api_url = format!(
            "http://{}:{}",
            text(&settings["app"]["host"])?,
            settings["app"]["port"].as_u64().ok_or("API端口无效")?
        );
        let frontend_url = text(&selected["frontend_url"])?.to_owned();
        super::plan::validate_endpoint(&frontend_url)?;
        check(
            selected["scope_id"] == settings["scope_id"]
                && selected["api_url"] == api_url
                && settings["cors"]["allow_origins"]
                    .as_array()
                    .is_some_and(|a| a.contains(&json!(&frontend_url))),
            "浏览器端点、scope或CORS与已审阅环境不一致",
        )?;
        common::require_closed_port(&frontend_url)?;
        Ok(Self {
            boot,
            bootstrap_file,
            execution,
            runtime,
            verified,
            frontend,
            bootstrap,
            review,
            review_file,
            settings,
            api_url,
            frontend_url,
        })
    }
    pub(super) fn binding(
        &self,
        api: &mut impl Dependencies,
        run_id: &str,
        server: &str,
        fresh: bool,
    ) -> Result<(Value, BTreeMap<String, PathBuf>)> {
        let Self {
            boot,
            bootstrap_file,
            execution,
            runtime,
            verified,
            frontend,
            bootstrap: _,
            review,
            review_file,
            settings,
            api_url,
            frontend_url,
        } = self;
        let src = &verified["build_evidence"]["source"];
        let mysql = path(&review["tools"]["mysql"]["path"])?;
        let mysql = bound(&mysql)?;
        check(
            mysql["sha256"] == review["tools"]["mysql"]["sha256"],
            "浏览器MySQL工具摘要变化",
        )?;
        let selected_tools = api.call("tools", json!({"frontend":frontend}))?;
        let corepack = path(&selected_tools["corepack"])?;
        let cache = path(&selected_tools["corepack_home"])?;
        directory(&cache)?;
        let mut tools = json!({"python":bound(&path(&boot["python"])?)?,"mysql":mysql,"corepack":bound(&corepack)?,"corepack_home":cache});
        if cfg!(windows) {
            tools["node"] = bound(&path(&selected_tools["node"])?)?;
            tools["corepack_entry"] = bound(
                &corepack
                    .parent()
                    .ok_or("Corepack缺少父目录")?
                    .join("node_modules/corepack/dist/corepack.js"),
            )?;
        }
        let limits=api.call("limits",json!({"bootstrap":bootstrap_file,"execution":execution,"app_env":boot["private"]["APP_ENV"]}))?;
        let outputs = outputs(frontend, runtime, run_id, server);
        if fresh && server == "preview" {
            check(
                !frontend.join("dist").try_exists()?,
                "前端已有生产产物，禁止覆盖未知构建",
            )?;
        }
        output_set(runtime, run_id, &outputs, fresh)?;
        let commands = commands(frontend, server)?;
        let response_audit = if server == "preview" {
            json!({"path":outputs["response_audit"],"initial_state":"absent","first_writer":"frontend-preview-static-proxy"})
        } else {
            Value::Null
        };
        let binding = json!({
            "format_version":1,"kind":"reference-fixture-browser-binding","status":"bound",
            "run_id":run_id,"server":server,"scope_id":settings["scope_id"],
            "bootstrap":bound(bootstrap_file)?,
            "environment":bound(&bootstrap_file.parent().ok_or("bootstrap缺少父目录")?.join("environment.json"))?,
            "review":bound(review_file)?,"runtime":verified["runtime"],
            "source_pair":bound(&runtime.join("source-pair.json"))?,
            "backend_build":bound(&runtime.join("backend-build.json"))?,
            "backend":{"path":execution,"source":src["generated"]["backend"]},
            "frontend":{"path":frontend,"source":{"original":src["original"]["frontend"],"generated":src["generated"]["frontend"]}},
            "tools":tools,"endpoints":{"api":api_url,"frontend":frontend_url},
            "identity":{"tenant_id":"system","username":"admin"},
            "rate_limits":{
                "request":{"capacity":limits["capacity"],"window_secs":limits["window_secs"]},
                "login":{"capacity":limits["api_limits"].get("POST /api/v1/auth/login").unwrap_or(&json!(5)),"window_secs":limits["api_window_secs"]}
            },
            "login_budget":{"path":outputs["login_budget"],"initial_state":"absent","first_writer":"frontend-real-browser-login"},
            "response_audit":response_audit,"commands":commands,"remote_writes":0
        });
        Ok((binding, outputs))
    }
    pub(super) fn context(
        self,
        binding: Value,
        outputs: BTreeMap<String, PathBuf>,
    ) -> Result<Context> {
        let Self {
            boot,
            bootstrap_file: _,
            execution,
            runtime,
            verified: _,
            frontend,
            bootstrap,
            review: _,
            review_file: _,
            settings: _,
            api_url: _,
            frontend_url: _,
        } = self;
        let source_guards = vec![
            SourceGuard::capture(&execution, &binding["backend"]["source"])?,
            SourceGuard::capture(&frontend, &binding["frontend"]["source"]["generated"])?,
        ];
        let mut input_guards = input_guards(&binding)?;
        input_guards.push(snapshot(
            &path(&bootstrap["secret_files"]["reset-admin-password.txt"]["path"])?,
            1024 * 1024,
        )?);
        let secrets = plan::secrets(&boot["private"], &bootstrap)?;
        let tool = publication::tool_capture(&frontend)?;
        Ok(Context {
            binding,
            frontend,
            runtime,
            outputs,
            input_guards,
            source_guards,
            tool,
            secrets,
            private: boot["private"].clone(),
        })
    }
}

pub(super) fn commands(frontend: &std::path::Path, server: &str) -> Result<Value> {
    check(matches!(server, "dev" | "preview"), "浏览器模式无效")?;
    let browser = json!([
        "check",
        "--stage",
        "browser",
        "--real",
        "--fixture",
        "business",
        "--server",
        server
    ]);
    let verify = json!([
        "exec",
        "node",
        "scripts/restore-build.mjs",
        "verify",
        "--source-root",
        frontend
    ]);
    Ok(if server == "dev" {
        json!([browser])
    } else {
        json!([["build", "--real"], verify, browser, verify])
    })
}
