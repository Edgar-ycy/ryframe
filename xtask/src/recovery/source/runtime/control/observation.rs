//! 未启动恢复与谱系授权只创建本地完整观察证据，不生成产品启动意图或进程。
use super::{configuration, existing, frame::Frame};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::{
        clone::native::{
            export::transport::Transport,
            json as document,
            resources::{common, storage::registered},
            seed::protocol::guards::rebind::live,
        },
        fixture_control,
        source::evidence::{
            binding, environment, images,
            resources::Resources,
            running::{builds, history::lineage::failure::inputs},
        },
    },
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

pub(super) struct Prepared {
    pub facts: Value,
    pub descriptor: Value,
    pub config: PathBuf,
    pub binaries: Value,
}

pub(super) fn prepare(
    backend: &Path,
    run: &Path,
    number: u64,
    descriptor: &Value,
    source: &Value,
    output: &Path,
    config: &Path,
) -> Result<Prepared> {
    validate_local_test_path(output, backend, LocalTestPathKind::NewDirectory)?;
    let request = common::read_bound(backend, descriptor, "来源恢复正式请求")?;
    crate::recovery::clone::native::seed::generation::coordinator::request(&request)?;
    existing::matches_source(&request, source)?;
    builds::verify(backend, &request)?;
    let execution = Path::new(document::text(
        &request["execution_backend"],
        "来源恢复执行根",
    )?);
    if execution
        .parent()
        .is_some_and(|parent| parent.join("fixture.json").exists())
    {
        return Err("来源恢复观察不能混入独立 fixture 运行环境".into());
    }
    let private = common::read_bound(backend, &request["source_environment"], "来源恢复私有环境")?;
    document::exact(&private, &["environment"], "来源恢复私有环境")?;
    let environment = serde_json::from_value(private["environment"].clone())
        .map_err(|_| "来源私有运行环境必须为字符串映射；检查已绑定环境文件，不回显字段值")?;
    environment::configured(&environment)?;
    let mut selected = source["request"]["source"].clone();
    document::object(&selected, "来源恢复物理目标")?;
    selected["runtime_dir"] = json!(output.join("runtime"));
    physical(backend, run, &request, source, &selected, config)?;
    live::quiet(
        backend,
        &json!({"registration":source["registration"],"request":source["request"]}),
    )?;
    Ok(Prepared {
        binaries: inputs::binaries(backend, &request)?,
        descriptor: descriptor.clone(),
        config: config.to_owned(),
        facts: json!({"request":request,"source":source,"selected":selected,"directory":run,"number":number,
            "output":output,"execution":execution,"environment":private}),
    })
}

pub(super) struct Observation<'a, 'b, T> {
    pub frame: &'a Frame<'b>,
    pub prepared: Prepared,
    pub contract: Value,
    environment: BTreeMap<String, String>,
    original_environment: BTreeMap<String, String>,
    transport: &'a T,
}

impl<'a, 'b, T: Transport + Sync> Observation<'a, 'b, T> {
    pub(super) fn create(
        frame: &'a Frame<'b>,
        prepared: Prepared,
        selection: configuration::Selection<'_>,
        transport: &'a T,
    ) -> Result<Self> {
        frame.checkpoint()?;
        let output = Path::new(document::text(
            &prepared.facts["output"],
            "本次恢复观察目录",
        )?);
        validate_local_test_path(output, frame.backend, LocalTestPathKind::NewDirectory)?;
        fs::create_dir(output).map_err(|error| {
            format!("无法创建本次恢复观察目录：{error}；保留已有现场，禁止覆盖或重放")
        })?;
        let contract = configuration::register(
            frame,
            &prepared.facts["request"],
            output,
            &prepared.binaries,
            selection,
        )?;
        let private = serde_json::from_value(prepared.facts["environment"]["environment"].clone())
            .map_err(|_| "来源私有运行环境必须为字符串映射；检查已绑定环境文件，不回显字段值")?;
        let value = Self {
            frame,
            environment: environment::configured(&private)?,
            prepared,
            contract,
            original_environment: std::env::vars().collect(),
            transport,
        };
        value.checkpoint()?;
        Ok(value)
    }

    pub(super) fn output(&self) -> Result<&Path> {
        Ok(Path::new(document::text(
            &self.prepared.facts["output"],
            "恢复观察目录",
        )?))
    }

    pub(super) fn checkpoint(&self) -> Result<()> {
        self.frame.checkpoint()?;
        let facts = &self.prepared.facts;
        let request = &facts["request"];
        if common::read_bound(
            self.frame.backend,
            &self.prepared.descriptor,
            "恢复显式请求",
        )? != *request
            || std::env::vars().collect::<BTreeMap<_, _>>() != self.original_environment
        {
            return Err("恢复观察期间正式请求或控制环境变化".into());
        }
        builds::verify(self.frame.backend, request)?;
        let execution = Path::new(document::text(&facts["execution"], "恢复执行根")?);
        let runtime = self.output()?.join("runtime");
        let internal = self.output()?.join("request.json");
        if self.frame.mode == "source-generation-start"
            && internal.try_exists()?
            && document::read(&internal)? != *request
        {
            return Err("START内部请求与原冻结请求不同；保留现场，不重放".into());
        }
        let intent = self.output()?.join("intent.json");
        if self.frame.mode == "source-generation-start" && intent.try_exists()? {
            let intent = document::read(&intent)?;
            super::super::running::roles::directory(&runtime, &intent["operations"])?;
        } else {
            inputs::exact_directory(
                self.frame.backend,
                &runtime,
                &["binaries.json", "runtime.json"],
                &[],
            )?;
        }
        if fixture_control::verify_full_stack_runtime(execution, &runtime)? != self.contract
            || document::read(&runtime.join("binaries.json"))? != self.prepared.binaries
        {
            return Err("恢复观察的完整 runtime 或实际产品/维护产物变化".into());
        }
        let config = Path::new(document::text(
            &self.contract["api_config_dir"],
            "恢复绑定 API 配置",
        )?);
        physical(
            self.frame.backend,
            self.frame.run,
            request,
            &facts["source"],
            &facts["selected"],
            config,
        )
    }

    pub(super) fn quiet(&self) -> Result<()> {
        self.checkpoint()?;
        let source = &self.prepared.facts["source"];
        live::quiet(
            self.frame.backend,
            &json!({"registration":source["registration"],"request":source["request"]}),
        )?;
        self.checkpoint()
    }

    pub(super) fn capture(&self, phase: &str) -> Result<(Value, Value)> {
        if !matches!(phase, "before" | "after")
            && !(phase == "running" && self.frame.mode == "source-generation-start")
        {
            return Err("恢复只读观察阶段无效".into());
        }
        let facts = &self.prepared.facts;
        let output = self.output()?.join(phase);
        let checkpoint = || self.checkpoint();
        let resources = Resources {
            backend: self.frame.backend,
            execution: Path::new(document::text(&facts["execution"], "恢复执行根")?),
            request: &facts["source"]["seed_target"],
            environment: &self.environment,
            output: &output,
            transport: self.transport,
            checkpoint: &checkpoint,
        };
        let context = images::capture::context(&resources, facts)?;
        let binding =
            images::capture::run(&resources, &context, &facts["source"], &self.frame.scope)?;
        let image = images::verify(
            self.frame.backend,
            &binding,
            &facts["selected"],
            &facts["source"]["request"],
            &facts["request"]["source_registration"],
        )?;
        self.checkpoint()?;
        Ok((binding, image))
    }
}

fn physical(
    backend: &Path,
    run: &Path,
    request: &Value,
    source: &Value,
    selected: &Value,
    config: &Path,
) -> Result<()> {
    if serde_json::to_value(registered::current(
        backend,
        run,
        crate::cli::CloneSide::Target,
    )?)? != request["current_storage"]
    {
        return Err("来源恢复观察的实际存储代次不同于固定请求".into());
    }
    let execution = Path::new(document::text(&request["execution_backend"], "恢复执行根")?);
    let actual = binding::physical(
        &json!({"backend":execution,"evidence_root":backend,"source":selected,
        "app_env":"test","config_dir":config}),
    )?;
    if actual != source["generation"]["physical_binding"] {
        return Err("来源恢复的实际配置改变了 C52 完整物理来源".into());
    }
    inputs::api_address(execution, &json!(config), &selected["api_url"])
}
