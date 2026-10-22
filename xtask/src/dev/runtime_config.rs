use std::{
    collections::BTreeMap,
    fmt, fs,
    io::Write,
    path::{Path, PathBuf},
};

use ryframe_config::{AppConfig, Environment};

use crate::Result;

#[path = "runtime_config/paths.rs"]
mod paths;
#[path = "runtime_config/profile.rs"]
mod profile;

const FILE_NAMES: &[&str] = &[
    "app.toml",
    "app.dev.toml",
    "app.test.toml",
    "app.prod.toml",
    "tools.toml",
    "feature-matrix.json",
];

/// 每代独立持有原始配置字节和已校验的开发配置，不重新读取项目环境。
pub(crate) struct RuntimeConfig {
    files: BTreeMap<String, Vec<u8>>,
    app: AppConfig,
}

impl fmt::Debug for RuntimeConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeConfig")
            .field("file_names", &self.files.keys().collect::<Vec<_>>())
            .field("file_count", &self.files.len())
            .finish()
    }
}

#[derive(Clone, Copy)]
pub(crate) struct RuntimeProfile {
    pub(crate) api_port: u16,
    pub(crate) worker_port: u16,
    pub(crate) snowflake_worker_id: u16,
    pub(crate) probe: bool,
    pub(crate) worker: bool,
}

impl RuntimeConfig {
    pub(crate) fn capture(source: &Path) -> Result<Self> {
        let files = read_files(source)?;
        // 即使加载失败也对照第二次读取，避免把变化中的文件集当作稳定输入。
        let loaded = AppConfig::load(source, Environment::Dev);
        if files != read_files(source)? {
            return Err(format!(
                "捕获开发配置 {} 失败：文件集合或字节在加载期间变化；请保存完成后重试",
                source.display()
            )
            .into());
        }
        let app = loaded.map_err(|error| {
            format!(
                "捕获开发配置 {} 的 AppConfig 校验失败：{}",
                source.display(),
                error.message(),
            )
        })?;
        Ok(Self { files, app })
    }

    pub fn app(&self) -> &AppConfig {
        &self.app
    }

    pub fn snapshot_to(&self, target: &Path) -> Result<()> {
        write_new_directory(target, &self.files)
    }

    pub fn semantically_matches(&self, other: &Self) -> Result<bool> {
        if !self.files.keys().eq(other.files.keys()) {
            return Ok(false);
        }
        for (name, bytes) in &self.files {
            let other_bytes = other.files.get(name).ok_or("配置文件集合不一致")?;
            if name.ends_with(".json") {
                let left = parse_json(name, bytes)?;
                let right = parse_json(name, other_bytes)?;
                if left != right {
                    return Ok(false);
                }
            } else if parse_table(name, bytes)? != parse_table(name, other_bytes)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub fn write_profile(
        &self,
        target: &Path,
        profile: RuntimeProfile,
        locales_final: &Path,
    ) -> Result<()> {
        profile::validate(profile, locales_final)?;
        let mut files = self.files.clone();
        let overlay = files.get("app.dev.toml").ok_or("缺少 app.dev.toml")?;
        let mut overlay = parse_table("app.dev.toml", overlay)?;
        profile::apply(&mut overlay, profile, locales_final)?;
        let bytes = toml::to_string(&overlay)
            .map_err(|_| format!("序列化角色配置 app.dev.toml 失败，尚未执行目标 {} 的写入；请检查覆盖字段是否为有效 TOML 值", target.display()))?
            .into_bytes();
        files.insert("app.dev.toml".into(), bytes);
        write_new_directory(target, &files)?;
        let checked = (|| -> Result<()> {
            let loaded = AppConfig::load(target, Environment::Dev);
            if files != read_files(target)? {
                return Err(
                    "角色配置文件集合或字节在校验期间变化；请停止并发修改并重新捕获".into(),
                );
            }
            let app =
                loaded.map_err(|error| format!("角色 AppConfig 加载失败：{}", error.message()))?;
            profile::verify(&app, profile, locales_final)
        })();
        checked.map_err(|error| format!(
            "角色配置最终校验失败：目标 {} 已创建，保留部分现场；阶段原因：{error}；请核对现场并修正对应字段，使用新目标目录重试",
            target.display(),
        ).into())
    }
}

fn read_files(source: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    paths::plain_directory_chain(source)?;
    let mut files = BTreeMap::new();
    let entries = fs::read_dir(source).map_err(|error| {
        paths::io_error(
            "枚举开发配置目录",
            source,
            &error,
            "请提供已有普通目录并检查读取权限",
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            paths::io_error(
                "读取开发配置目录项",
                source,
                &error,
                "请检查目录读取权限及文件系统状态后重试",
            )
        })?;
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(|| {
            format!(
                "读取配置目录 {} 失败：文件名不是 UTF-8；请仅保留已登记的配置文件名",
                source.display()
            )
        })?;
        if !FILE_NAMES.contains(&name) {
            return Err(format!(
                "读取配置目录 {} 失败：包含未登记入口；请移出未知入口，仅保留六个已登记配置文件",
                source.display()
            )
            .into());
        }
        let path = entry.path();
        paths::plain_file(&path)?;
        let bytes = fs::read(&path).map_err(|error| {
            paths::io_error(
                "读取已登记配置文件",
                &path,
                &error,
                "请检查文件读取权限及文件系统状态后重试",
            )
        })?;
        paths::plain_file(&path)?;
        if name.ends_with(".json") {
            parse_json(name, &bytes)?;
        } else {
            parse_table(name, &bytes)?;
        }
        files.insert(name.to_owned(), bytes);
    }
    if !files.contains_key("app.dev.toml") {
        return Err(format!(
            "读取开发配置失败：缺少必需文件 {}；请创建 app.dev.toml 并填写开发配置",
            source.join("app.dev.toml").display()
        )
        .into());
    }
    Ok(files)
}

fn parse_table(name: &str, bytes: &[u8]) -> Result<toml::Table> {
    let text = std::str::from_utf8(bytes).map_err(|error| {
        format!(
            "解析配置文件 {name} 失败：字节位置 {} 存在无效 UTF-8；请使用 UTF-8 编码保存",
            error.valid_up_to(),
        )
    })?;
    toml::from_str(text).map_err(|error: toml::de::Error| {
        let span = error.span().map_or_else(|| "不可用".to_owned(), |span| format!("{}..{}", span.start, span.end));
        format!("解析配置文件 {name} 失败：TOML 语法无效，字节位置 {span}；请检查该位置的引号、类型和重复键").into()
    })
}

fn parse_json(name: &str, bytes: &[u8]) -> Result<serde_json::Value> {
    serde_json::from_slice(bytes).map_err(|error| format!(
        "解析配置文件 {name} 失败：JSON 语法无效，line={}，column={}；请检查该位置的引号、分隔符和 JSON 值类型",
        error.line(), error.column(),
    ).into())
}

fn write_new_directory(target: &Path, files: &BTreeMap<String, Vec<u8>>) -> Result<()> {
    let target = paths::new_directory_path(target)?;
    fs::create_dir(&target).map_err(|error| {
        paths::io_error(
            "独占创建配置目标目录",
            &target,
            &error,
            "请准备可写的普通父目录并指定尚不存在的目标，拒绝覆盖已有路径",
        )
    })?;
    // 只创建本次目录内的新文件；失败时保留现场，不递归删除未知路径。
    let written = (|| -> Result<()> {
        for (name, bytes) in files {
            paths::plain_directory_chain(&target)?;
            let path: PathBuf = target.join(name);
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|error| {
                    paths::io_error(
                        "独占创建配置文件",
                        &path,
                        &error,
                        "请检查目标目录写入权限，拒绝覆盖已有文件",
                    )
                })?;
            file.write_all(bytes).map_err(|error| {
                paths::io_error(
                    "完整写入配置文件",
                    &path,
                    &error,
                    "请检查磁盘空间、写入权限及文件系统状态",
                )
            })?;
        }
        Ok(())
    })();
    written.map_err(|error| format!(
        "写入配置失败：目标 {} 已创建，保留部分现场；阶段原因：{error}；请核对现场后使用新目标目录重试",
        target.display(),
    ).into())
}
