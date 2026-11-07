use super::{documents, model};
use crate::recovery::clone::native::{
    export::transport::{CommandSpec, StdoutTarget, Transport},
    resources::common,
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path, validate_local_test_path},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

pub(super) struct Bound {
    root: PathBuf,
    directory: PathBuf,
    work: PathBuf,
    program: PathBuf,
    args: Vec<String>,
    files: Vec<(PathBuf, Value, (u64, u64))>,
    directories: Vec<(PathBuf, (u64, u64))>,
    input_sha: String,
}

impl Bound {
    pub(super) fn capture(
        plan: &Value,
        root: &Path,
        directory: &Path,
        input: &Path,
    ) -> Result<Self> {
        let db = plan["source"]["databases"]
            .as_array()
            .ok_or("源数据库缺失")?
            .iter()
            .find(|db| db["kind"] == "combined")
            .ok_or("combined目标缺失")?;
        let defaults = model::path(&db["defaults_file"])?;
        crate::recovery::source::evidence::reference_workflow::readonly::defaults::validate(
            &defaults,
        )?;
        let program = model::path(&plan["tools"]["mysql"]["path"])?;
        let mut paths = vec![input.to_owned(), defaults.clone(), program.clone()];
        paths.extend(credential_paths(plan, root, &defaults)?);
        let mut files = Vec::new();
        for path in paths {
            validate_absolute_path(&path, LocalTestPathKind::ExistingFile)?;
            files.push((
                path.clone(),
                common::binding(&path)?,
                common::device_inode_for(&path, &fs::metadata(&path)?)?,
            ));
        }
        if files[1].1["sha256"] != db["defaults_sha256"]
            || files[2].1["sha256"] != plan["tools"]["mysql"]["sha256"]
        {
            return Err("MySQL工具或defaults与计划摘要不符".into());
        }
        let work = model::path(&plan["work_dir"])?;
        let mut directory_paths = vec![root.to_owned(), work.clone(), directory.to_owned()];
        if !plan["dataset"]["configuration"].is_null() {
            directory_paths.push(model::path(&plan["dataset"]["configuration"]["directory"])?);
        }
        let directories = directory_paths
            .into_iter()
            .map(|p| Ok((p.clone(), common::device_inode_for(&p, &fs::metadata(&p)?)?)))
            .collect::<Result<Vec<_>>>()?;
        let result = Self {
            root: root.into(),
            directory: directory.into(),
            work,
            program,
            args: vec![
                format!("--defaults-file={}", defaults.display()),
                format!(
                    "--database={}",
                    db["database"].as_str().ok_or("数据库名称缺失")?
                ),
                "--batch".into(),
                "--raw".into(),
                "--skip-column-names".into(),
                "--binary-mode".into(),
                "--default-character-set=utf8mb4".into(),
            ],
            input_sha: files[0].1["sha256"].as_str().ok_or("输入摘要缺失")?.into(),
            files,
            directories,
        };
        result.verify()?;
        Ok(result)
    }

    pub(super) fn input_sha(&self) -> &str {
        &self.input_sha
    }

    pub(super) fn verify(&self) -> Result<()> {
        validate_local_test_path(
            &self.directory,
            &self.root,
            LocalTestPathKind::ExistingDirectory,
        )?;
        for (path, expected, identity) in &self.files {
            validate_absolute_path(path, LocalTestPathKind::ExistingFile)?;
            if common::binding(path)? != *expected
                || common::device_inode_for(path, &fs::metadata(path)?)? != *identity
            {
                return Err("岗位批次输入、工具、defaults或TLS文件发生漂移".into());
            }
        }
        for (path, identity) in &self.directories {
            validate_absolute_path(path, LocalTestPathKind::ExistingDirectory)?;
            if common::device_inode_for(path, &fs::metadata(path)?)? != *identity {
                return Err("岗位工作目录身份漂移".into());
            }
        }
        match fs::symlink_metadata(self.work.join("unused-login.cnf")) {
            Ok(_) => Err("岗位批次拒绝隐式MySQL login文件（包括链接）".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err("无法确认隐式MySQL login文件缺失".into()),
        }
    }
}

pub(super) struct Client<'a, T> {
    pub(super) bound: Bound,
    pub(super) transport: &'a T,
    pub(super) check: &'a dyn Fn() -> Result<()>,
    pub(super) deadline: Instant,
    pub(super) completed: u64,
    pub(super) possible_write: bool,
}

impl<T: Transport> Client<'_, T> {
    pub(super) fn query(&mut self, sql: Vec<u8>, writes: bool) -> Result<Vec<u8>> {
        (self.check)()?;
        self.bound.verify()?;
        let timeout = self
            .deadline
            .checked_duration_since(Instant::now())
            .filter(|v| !v.is_zero())
            .ok_or("岗位批次总900秒预算耗尽")?;
        let mut environment = std::env::vars()
            .filter(|(name, _)| crate::devex::runtime_native::entry::allowed_environment(name))
            .collect::<BTreeMap<_, _>>();
        environment.insert(
            "MYSQL_TEST_LOGIN_FILE".into(),
            self.bound
                .work
                .join("unused-login.cnf")
                .display()
                .to_string(),
        );
        self.possible_write |= writes;
        let result = self.transport.run(CommandSpec {
            program: self.bound.program.clone(),
            args: self.bound.args.clone(),
            cwd: self.bound.root.clone(),
            environment,
            stdin: Some(sql),
            stdout: StdoutTarget::Capture,
            work_dir: self.bound.directory.clone(),
            label: "岗位SQL批次".into(),
            timeout,
        });
        let output = match result {
            Ok(output) => output,
            Err(error) => {
                let code = error
                    .downcast_ref::<std::io::Error>()
                    .and_then(std::io::Error::raw_os_error);
                return self.reject("process", json!({"system_code":code,"exit_code":null,
                    "mysql_error":null,"sqlstate":null,"advice":"核对MySQL工具、系统资源和剩余预算；核对完整前后像，禁止重放"}));
            }
        };
        let stable = (self.check)().and_then(|()| self.bound.verify());
        // 不传播任意stderr，只有摘要、长度和受限错误身份进入失败收据。
        if !output.status.success() {
            let detail = mysql_failure(output.status.code(), &output.stderr);
            return self.reject(if writes { "insert" } else { "maximum" }, detail);
        }
        stable?;
        if Instant::now() >= self.deadline {
            return Err("岗位批次总900秒预算耗尽；保留现场，禁止重放".into());
        }
        Ok(output.stdout)
    }

    pub(super) fn record(&self, stage: &str, detail: Value) -> Result<PathBuf> {
        validate_local_test_path(
            &self.bound.directory,
            &self.bound.root,
            LocalTestPathKind::ExistingDirectory,
        )?;
        for (path, identity) in &self.bound.directories {
            validate_absolute_path(path, LocalTestPathKind::ExistingDirectory)?;
            if common::device_inode_for(path, &fs::metadata(path)?)? != *identity {
                return Err("失败目录身份漂移，不发布收据".into());
            }
        }
        let path = self.bound.directory.join(format!(
            "post-batch-failure-{}.json",
            uuid::Uuid::new_v4().simple()
        ));
        documents::write_new(
            &path,
            &json!({"format_version":1,"kind":"dataset-post-batch-failure","stage":stage,"batch":self.completed+1,
                "completed_batches":self.completed,"possible_write":self.possible_write,"automatic_replay":false,
                "bindings":self.bound.files.iter().take(3).map(|(_,binding,_)|binding).collect::<Vec<_>>(),"detail":detail}),
        )?;
        validate_local_test_path(&path, &self.bound.root, LocalTestPathKind::ExistingFile)?;
        Ok(path)
    }

    fn reject(&self, stage: &str, detail: Value) -> Result<Vec<u8>> {
        let receipt = match self.record(stage, detail.clone()) {
            Ok(path) => format!("失败收据={}", path.display()),
            Err(_) => "不能确认失败收据发布；保留输入与原现场，禁止重放".into(),
        };
        Err(format!("岗位SQL失败：阶段={stage}；exit={}；系统码={}；MySQL={}；SQLSTATE={}；已完成批数={}；可能写入={}；unknown={}；{}；{}",
            detail["exit_code"], detail["system_code"], detail["mysql_error"], detail["sqlstate"],
            self.completed, self.possible_write, self.possible_write,
            detail["advice"].as_str().unwrap_or("核对完整前后像，保留现场，禁止重放"), receipt).into())
    }
}

fn mysql_failure(exit: Option<i32>, stderr: &[u8]) -> Value {
    static PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let pattern = PATTERN.get_or_init(|| {
        regex::Regex::new(r"(?m)^ERROR ([0-9]{3,5}) \(([0-9A-Z]{5})\)")
            .expect("固定MySQL错误身份模式有效")
    });
    let text = String::from_utf8_lossy(stderr);
    let fields = pattern.captures(&text);
    let code = fields.as_ref().and_then(|f| f[1].parse::<u32>().ok());
    let state = fields.as_ref().map(|f| f[2].to_owned());
    let advice = match code {
        Some(1045) => {
            "认证拒绝：核对当前绑定MySQL账号、密码配置与TLS；旧预检不证明当前可用，先核对完整像，禁止重放"
        }
        Some(1146) => {
            "表不存在：核对当前目标数据库和正式迁移状态，不直接补表或重放；保留现场并核对完整像"
        }
        Some(1062) => "主键或唯一约束冲突：核对连续ID范围、既有数据及完整前后像，不覆盖记录或重放",
        _ => "核对所示MySQL码、受管配置、服务及完整前后像；保留输入和失败证据，禁止重放",
    };
    json!({"exit_code":exit,"system_code":null,"stderr_bytes":stderr.len(),"stderr_sha256":model::sha(stderr),
        "mysql_error":code,"sqlstate":state,"advice":advice})
}

fn credential_paths(plan: &Value, root: &Path, defaults: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let config = &plan["dataset"]["configuration"];
    if !config.is_null() {
        let directory = model::path(&config["directory"])?;
        for filename in config["files"]
            .as_object()
            .ok_or("配置文件绑定缺失")?
            .keys()
        {
            paths.push(directory.join(filename));
        }
        paths.push(model::path(
            &plan["dataset"]["administrator_credentials"]["path"],
        )?);
    }
    let values = crate::recovery::source::evidence::reference_workflow::readonly::defaults::parse(
        &fs::read_to_string(defaults)?,
    )?;
    for key in ["ssl-ca", "ssl-cert", "ssl-key"] {
        if let Some(path) = values.get(key) {
            let path = PathBuf::from(path);
            paths.push(if path.is_absolute() {
                path
            } else {
                root.join(path)
            });
        }
    }
    Ok(paths)
}
