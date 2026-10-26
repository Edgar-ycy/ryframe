//! 使用收据明确选择的环境读取公开配置，不读取宿主 APP_ENV。
use super::{
    fail,
    files::{self, StableFile},
    model,
};
use crate::Result;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

enum Observation {
    Present(StableFile),
    Absent(PathBuf, bool),
}
pub(crate) struct Inputs {
    pub(crate) configuration: Value,
    pub(crate) identity: Value,
    observed: Vec<Observation>,
}
impl Inputs {
    pub(crate) fn facts(&self) -> Result<Value> {
        let observed = self
            .observed
            .iter()
            .map(|item| match item {
                Observation::Present(file) => Ok(json!({"present":true,"file":file.facts()?})),
                Observation::Absent(path, managed) => {
                    Ok(json!({"present":false,"path":path,"managed":managed}))
                }
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(json!({"configuration":self.configuration,"identity":self.identity,"observed":observed}))
    }
    pub(crate) fn verify(&self) -> Result<()> {
        for item in &self.observed {
            match item {
                Observation::Present(file) => file.verify()?,
                Observation::Absent(path, managed) => {
                    if files::ordinary(path, true, *managed)?.is_some() {
                        return Err(fail("可选文件在核验期间出现"));
                    }
                }
            }
        }
        Ok(())
    }
}
fn read(
    path: &Path,
    optional: bool,
    managed: bool,
    observed: &mut Vec<Observation>,
) -> Result<Option<(Vec<u8>, Value)>> {
    let Some(path) = files::ordinary(path, optional, managed)? else {
        observed.push(Observation::Absent(path.to_owned(), managed));
        return Ok(None);
    };
    let mut file = StableFile::open(&path)?;
    let bytes = file.read()?;
    let descriptor = files::descriptor(Path::new(""), &path, &file.held.sha256)?;
    observed.push(Observation::Present(file));
    Ok(Some((bytes, descriptor)))
}
fn bound_read(
    root: &Path,
    path: &Path,
    optional: bool,
    managed: bool,
    observed: &mut Vec<Observation>,
) -> Result<Option<(Vec<u8>, Value)>> {
    read(path, optional, managed, observed)?
        .map(|(raw, mut binding)| {
            binding["path"] = json!(files::name(root, path)?);
            Ok((raw, binding))
        })
        .transpose()
}
fn known(value: &toml::Value, fields: &[&str]) -> Result<()> {
    let table = value
        .as_table()
        .ok_or_else(|| fail("TOML 字段必须是公开 table"))?;
    if table.keys().any(|key| !fields.contains(&key.as_str())) {
        return Err(fail("TOML 包含未知或非公开字段；删除凭据和工具私有字段"));
    }
    Ok(())
}
fn origin(value: &toml::Value, empty: bool) -> Result<String> {
    let text = value.as_str().ok_or_else(|| fail("origin 必须是字符串"))?;
    crate::frontend_origin::normalize_origin(text, empty)
        .map_err(|_| fail("HTTP(S) origin 无效；检查地址、身份信息、路径、查询及片段"))
}
fn validate(value: &toml::Value, complete: bool) -> Result<()> {
    known(value, &["api_origin", "server"])?;
    if let Some(api) = value.get("api_origin") {
        origin(api, true)?;
    }
    if let Some(server) = value.get("server") {
        known(server, &["host", "port", "proxy_target"])?;
        if let Some(host) = server.get("host") {
            let host = host
                .as_str()
                .ok_or_else(|| fail("server.host 必须为字符串"))?;
            if host.trim_matches(js_space).is_empty() || host.chars().any(char::is_control) {
                return Err(fail("server.host 必须非空且不含 Cc 控制字符"));
            }
        }
        if let Some(port) = server.get("port") {
            let port = port
                .as_integer()
                .map(|n| n as f64)
                .or_else(|| port.as_float())
                .filter(|n| n.is_finite() && n.fract() == 0.0 && (1.0..=65535.0).contains(n));
            if port.is_none() {
                return Err(fail(
                    "server.port 必须为 1..65535 的整数值数值，不能是 bool",
                ));
            }
        }
        if let Some(proxy) = server.get("proxy_target") {
            origin(proxy, false)?;
        }
    }
    if complete
        && (value.get("api_origin").is_none()
            || ["host", "port", "proxy_target"]
                .iter()
                .any(|key| value.get("server").and_then(|v| v.get(*key)).is_none()))
    {
        return Err(fail(
            "缺少 api_origin 或 server.host/port/proxy_target；补齐所选配置",
        ));
    }
    Ok(())
}
fn js_space(c: char) -> bool {
    matches!(c,'\u{0009}'..='\u{000d}'|'\u{0020}'|'\u{00a0}'|'\u{1680}'|'\u{2000}'..='\u{200a}'|'\u{2028}'|'\u{2029}'|'\u{202f}'|'\u{205f}'|'\u{3000}'|'\u{feff}')
}
fn tool_directory(root: &Path, observed: &mut Vec<Observation>) -> Result<(PathBuf, Vec<Value>)> {
    let mut directory = "config".to_owned();
    let mut tools = Vec::new();
    for relative in model::TOOLS {
        if let Some((raw, binding)) = bound_read(root, &root.join(relative), true, true, observed)?
        {
            let value = files::parse(&raw)?;
            if value.as_object().unwrap().keys().any(|key| {
                ![
                    "application_config_dir",
                    "api_contract",
                    "real_browser",
                    "browser_smoke",
                ]
                .contains(&key.as_str())
            }) {
                return Err(fail(
                    "tools JSON 包含未知 root 字段；只接受当前四个工具配置节",
                ));
            }
            if let Some(value) = value.get("application_config_dir") {
                directory = model::text(value)?.to_owned();
                if directory.is_empty() || directory.contains(['\0', '\r', '\n']) {
                    return Err(fail("application_config_dir 路径无效"));
                }
            }
            tools.push(binding);
        }
    }
    Ok((files::directory(root, Path::new(&directory))?, tools))
}
fn application(
    root: &Path,
    directory: &Path,
    environment: &str,
    observed: &mut Vec<Observation>,
) -> Result<Vec<Value>> {
    let mut merged = toml::Table::new();
    let mut bindings = Vec::new();
    for (index, name) in ["app.toml".into(), format!("app.{environment}.toml")]
        .iter()
        .enumerate()
    {
        if let Some((raw, binding)) =
            bound_read(root, &directory.join(name), index == 0, false, observed)?
        {
            let text = std::str::from_utf8(&raw).map_err(|_| fail("TOML UTF8 无效"))?;
            let value: toml::Value = toml::from_str(text)
                .map_err(|_| fail("TOML 语法、重复键或嵌套无效；检查公共和所选环境文件"))?;
            validate(&value, false)?;
            for (key, value) in value.as_table().unwrap() {
                if key == "server" {
                    let table = merged
                        .entry(key.clone())
                        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
                        .as_table_mut()
                        .ok_or_else(|| fail("server table 无效"))?;
                    table.extend(value.as_table().unwrap().clone());
                } else {
                    merged.insert(key.clone(), value.clone());
                }
            }
            bindings.push(binding);
        }
    }
    let value = toml::Value::Table(merged);
    validate(&value, true)?;
    if !origin(&value["api_origin"], true)?.is_empty() {
        return Err(fail("真实构建要求 api_origin 为空；改为浏览器同源"));
    }
    Ok(bindings)
}
fn identity(root: &Path, observed: &mut Vec<Observation>) -> Result<Value> {
    let mut identity = json!({"frontend_commit":"development","source_file":null});
    if let Some((raw, binding)) =
        bound_read(root, &root.join(model::IDENTITY), true, true, observed)?
    {
        let value = files::parse(&raw)?;
        model::exact(&value, &["format_version", "frontend_commit"])?;
        if model::number(&value["format_version"], 1) != Some(1)
            || value["frontend_commit"] != "development"
                && !model::hex(&value["frontend_commit"], 40)
        {
            return Err(fail("受管身份版本或提交无效"));
        }
        identity = json!({"frontend_commit":value["frontend_commit"],"source_file":binding});
    }
    let (raw, _) = bound_read(
        root,
        &root.join("dist/build-identity.json"),
        false,
        false,
        observed,
    )?
    .ok_or_else(|| fail("缺少产物身份"))?;
    let value = files::parse(&raw)?;
    model::exact(&value, &["frontend_commit"])?;
    if value["frontend_commit"] != identity["frontend_commit"] {
        return Err(fail("产物身份不同于受管构建身份"));
    }
    Ok(identity)
}
pub(crate) fn capture(root: &Path, environment: &str) -> Result<Inputs> {
    if !matches!(environment, "dev" | "test" | "prod") {
        return Err(fail("APP_ENV 只能明确选择 dev/test/prod"));
    }
    let root = files::directory(root, root)?;
    let mut observed = Vec::new();
    let (directory, tools) = tool_directory(&root, &mut observed)?;
    let files = application(&root, &directory, environment, &mut observed)?;
    let identity = identity(&root, &mut observed)?;
    let inputs = Inputs {
        configuration: json!({"environment":environment,"directory":files::name(&root,&directory)?,"files":files,"tools":tools}),
        identity,
        observed,
    };
    inputs.verify()?;
    Ok(inputs)
}
pub(crate) fn verify(root: &Path, build: &Value, expected: Option<&str>) -> Result<Inputs> {
    model::context(build)?;
    let inputs = capture(root, model::text(&build["configuration"]["environment"])?)?;
    if inputs.configuration != build["configuration"]
        || inputs.identity != build["identity"]
        || expected.is_some_and(|head| inputs.identity["frontend_commit"] != head)
    {
        return Err(fail(
            "配置、工具或身份字节绑定与构建不同；使用当前输入重建 v3 收据",
        ));
    }
    inputs.verify()?;
    Ok(inputs)
}
