use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

// 捕获内容包含真实配置，不派生 Debug，也不把解析器原文或字段值加入诊断。
pub(super) struct CapturedConfiguration {
    source: PathBuf,
    base: Option<CapturedFile>,
    selected: CapturedFile,
    table: toml::Table,
}

#[derive(PartialEq)]
struct CapturedFile {
    bytes: Vec<u8>,
    stamp: Stamp,
}

#[derive(PartialEq)]
struct Stamp {
    length: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
}

pub(super) struct PreparedConfiguration {
    captured: CapturedConfiguration,
    directory: PathBuf,
    base: Option<CapturedFile>,
    selected: Vec<u8>,
}

impl CapturedConfiguration {
    pub(super) fn capture(source: &Path) -> Result<Self, String> {
        check_directory(source, "捕获共享源/检查目录")?;
        let base = capture_optional_file(&source.join("app.toml"), "捕获共享源/app.toml")?;
        let selected = capture_file(&source.join("app.test.toml"), "捕获共享源/app.test.toml")?;
        if let Some(base) = &base {
            parse_table(&base.bytes, &source.join("app.toml"))?;
        }
        let table = parse_table(&selected.bytes, &source.join("app.test.toml"))?;
        // 读取结束后复核完整前像；公共文件的缺失同样属于本轮绑定。
        for (name, expected) in [
            ("app.toml", base.as_ref()),
            ("app.test.toml", Some(&selected)),
        ] {
            verify_captured_file(&source.join(name), expected, "捕获共享源/结束复核")?;
        }
        check_directory(source, "捕获共享源/结束检查目录")?;
        Ok(Self {
            source: source.to_path_buf(),
            base,
            selected,
            table,
        })
    }

    fn verify(&self) -> Result<(), String> {
        check_directory(&self.source, "复核共享源/检查目录")?;
        for (name, expected) in [
            ("app.toml", self.base.as_ref()),
            ("app.test.toml", Some(&self.selected)),
        ] {
            verify_captured_file(&self.source.join(name), expected, "复核共享源文件")?;
        }
        Ok(())
    }

    pub(super) fn prepare(
        self,
        root: &Path,
        schema: &str,
    ) -> Result<PreparedConfiguration, String> {
        self.verify()?;
        check_directory(root, "准备私有配置/检查工作区")?;
        let parent = root.join(".local-tests");
        match fs::create_dir(&parent) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(io_error(
                    "准备私有配置/创建父目录",
                    &parent,
                    &error,
                    "检查工作区父目录和写入权限，保留已有文件，不自动重放",
                ));
            }
        }
        check_directory(&parent, "准备私有配置/检查父目录")?;
        let directory = parent.join("导入 配置");
        // 只接管本次 create_dir 创建的新目录；已有目录一律拒绝，不覆盖外来文件。
        fs::create_dir(&directory).map_err(|error| {
            io_error(
                "准备私有配置/创建独占目录",
                &directory,
                &error,
                "若目标已存在，保留原目录并选择新的独占测试工作区；否则检查写权限，不自动重放",
            )
        })?;
        check_directory(&directory, "准备私有配置/检查新目录")
            .map_err(|error| retained_target(error, &directory))?;
        let selected = self
            .selected_for_schema(schema)
            .map_err(|error| retained_target(error, &directory))?;
        let base = if let Some(base) = &self.base {
            let path = directory.join("app.toml");
            publish_new(&path, &base.bytes)?;
            let published = capture_file(&path, "发布私有配置/复核公共文件")?;
            if published.bytes != base.bytes {
                return Err(retained_target(
                    changed_error("发布私有配置/复核公共文件", &path),
                    &directory,
                ));
            }
            Some(published)
        } else {
            None
        };
        publish_new(&directory.join("app.test.toml"), &selected)?;
        let prepared = PreparedConfiguration {
            captured: self,
            directory,
            base,
            selected,
        };
        prepared
            .verify()
            .map_err(|error| retained_target(error, &prepared.directory))?;
        Ok(prepared)
    }

    fn selected_for_schema(&self, schema: &str) -> Result<Vec<u8>, String> {
        let path = self.source.join("app.test.toml");
        let mut table = self.table.clone();
        let database = table
            .entry("database")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .ok_or_else(|| field_error(&path, "database"))?;
        let primary = database
            .entry("primary")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .ok_or_else(|| field_error(&path, "database.primary"))?;
        primary.insert("database".into(), toml::Value::String(schema.into()));
        toml::to_string(&table)
            .map(String::into_bytes)
            .map_err(|_| {
                diagnostic(
                    "准备私有配置/序列化选中TOML",
                    &path,
                    "已解析的 TOML 表无法重新序列化",
                    "检查配置是否仅使用 TOML 支持的表和值类型；保留本次目标，不自动重放",
                )
            })
    }
}

impl PreparedConfiguration {
    pub(super) fn directory(&self) -> Result<&Path, String> {
        self.verify()?;
        Ok(&self.directory)
    }

    pub(super) fn verify(&self) -> Result<(), String> {
        self.captured.verify()?;
        check_directory(&self.directory, "复核私有配置/检查目录")?;
        verify_captured_file(
            &self.directory.join("app.toml"),
            self.base.as_ref(),
            "复核私有配置文件",
        )?;
        let path = self.directory.join("app.test.toml");
        if capture_file(&path, "复核私有配置文件")?.bytes != self.selected {
            return Err(diagnostic(
                "复核私有配置文件",
                &path,
                "文件字节与本次已发布配置不一致",
                "停止导入并核对修改来源；目标文件保留，不重建、不覆盖、不自动重放",
            ));
        }
        Ok(())
    }
}

fn parse_table(bytes: &[u8], path: &Path) -> Result<toml::Table, String> {
    let text = std::str::from_utf8(bytes).map_err(|error| {
        diagnostic(
            "解析共享配置/UTF8",
            path,
            &format!(
                "UTF-8 {}；byte={}；error_len={:?}",
                if error.error_len().is_some() {
                    "字节序列非法"
                } else {
                    "末尾字节序列不完整"
                },
                error.valid_up_to(),
                error.error_len()
            ),
            "在编辑器中将该文件保存为 UTF-8，按 byte 位置修复无效编码，不把原始内容写入日志",
        )
    })?;
    toml::from_str(text).map_err(|error: toml::de::Error| {
        let Some(span) = error.span() else {
            return diagnostic(
                "解析共享配置/TOML",
                path,
                &format!("TOML {}；byte/span=不可用", toml_error_kind(&error)),
                "解析器未提供位置；在编辑器中检查 TOML 语法及表结构，不输出原文或字段值",
            );
        };
        let offset = span.start.min(bytes.len());
        let prefix = &bytes[..offset];
        let line = prefix.iter().filter(|byte| **byte == b'\n').count() + 1;
        let column = prefix
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(offset + 1, |last| offset - last);
        diagnostic(
            "解析共享配置/TOML",
            path,
            &format!(
                "TOML {}；byte/span={}..{}；行={line}；列={column}",
                toml_error_kind(&error),
                span.start,
                span.end
            ),
            "在编辑器中检查该位置的引号、括号、重复键和值类型；不要输出解析原文或字段值",
        )
    })
}

fn toml_error_kind(error: &toml::de::Error) -> &'static str {
    // 只按解析器原因分类；绝不拼接可能含键名、值或原始输入的消息。
    let reason = error.message();
    if reason.contains("duplicate") {
        "重复键或重复表声明"
    } else if reason.contains("unterminated") || reason.contains("unclosed") {
        "字符串或括号未闭合"
    } else if reason.contains("expected") {
        "语法元素缺失或分隔符位置错误"
    } else if reason.contains("out of range") || reason.contains("invalid number") {
        "数字字面量无效或越界"
    } else if reason.contains("escape") {
        "字符串转义无效"
    } else {
        "语法或表结构无效"
    }
}

fn verify_captured_file(
    path: &Path,
    expected: Option<&CapturedFile>,
    stage: &str,
) -> Result<(), String> {
    if capture_optional_file(path, stage)?.as_ref() != expected {
        return Err(changed_error(stage, path));
    }
    Ok(())
}

fn capture_optional_file(path: &Path, stage: &str) -> Result<Option<CapturedFile>, String> {
    let parent = path.parent().ok_or_else(|| changed_error(stage, path))?;
    check_directory(parent, stage)?;
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            check_directory(parent, stage)?;
            Ok(None)
        }
        Err(error) => Err(io_error(
            stage,
            path,
            &error,
            "检查文件元数据及读取权限；不切换来源、不自动重放",
        )),
        Ok(_) => capture_file(path, stage).map(Some),
    }
}

fn capture_file(path: &Path, stage: &str) -> Result<CapturedFile, String> {
    let parent = path.parent().ok_or_else(|| {
        diagnostic(
            stage,
            path,
            "文件缺少父目录",
            "提供共享目录中 app.toml 或 app.test.toml 的绝对路径",
        )
    })?;
    check_directory(parent, stage)?;
    let before = fs::symlink_metadata(path).map_err(|error| {
        io_error(
            &format!("{stage}/读取前元数据"),
            path,
            &error,
            "检查该文件是否存在及读取权限，保留捕获来源，不自动切换配置",
        )
    })?;
    if !ordinary(&before) || !before.is_file() {
        return Err(diagnostic(
            stage,
            path,
            "预期普通无链接文件，实际为非普通文件、链接或重解析点",
            "提供真实普通文件，移除配置路径中的链接或重解析跳转；不要读取其他源文件",
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT：不跟随末层重解析点。
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0o400000); // O_NOFOLLOW：不跟随末层符号链接。
    }
    let mut file = options.open(path).map_err(|error| {
        io_error(
            &format!("{stage}/打开文件"),
            path,
            &error,
            "检查读取权限、占用状态及链接替换；不要放宽无链接限制",
        )
    })?;
    let held = file.metadata().map_err(|error| {
        io_error(
            &format!("{stage}/句柄元数据"),
            path,
            &error,
            "停止本次捕获，检查文件句柄状态及读取权限",
        )
    })?;
    if !ordinary(&held) || !held.is_file() || stamp(&held) != stamp(&before) {
        return Err(changed_error(stage, path));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|error| {
        io_error(
            &format!("{stage}/读取字节"),
            path,
            &error,
            "检查文件可读性和设备状态；本次未取得完整配置，不自动重试",
        )
    })?;
    let after = fs::symlink_metadata(path).map_err(|error| {
        io_error(
            &format!("{stage}/读取后元数据"),
            path,
            &error,
            "核对源文件是否在读取期间被删除或替换，保留现场",
        )
    })?;
    let held_after = file.metadata().map_err(|error| {
        io_error(
            &format!("{stage}/读取后句柄元数据"),
            path,
            &error,
            "核对文件句柄及设备状态，保留现场，不自动重试",
        )
    })?;
    if !ordinary(&after)
        || !after.is_file()
        || stamp(&after) != stamp(&before)
        || stamp(&held_after) != stamp(&before)
        || bytes.len() as u64 != before.len()
    {
        return Err(changed_error(stage, path));
    }
    check_directory(parent, stage)?;
    Ok(CapturedFile {
        bytes,
        stamp: stamp(&before),
    })
}

fn check_directory(directory: &Path, stage: &str) -> Result<(), String> {
    if !directory.is_absolute() {
        return Err(diagnostic(
            stage,
            directory,
            "目录不是绝对路径",
            "提供明确的绝对共享源或独占工作区目录",
        ));
    }
    for path in directory.ancestors().collect::<Vec<_>>().into_iter().rev() {
        let metadata = fs::symlink_metadata(path).map_err(|error| {
            io_error(
                stage,
                path,
                &error,
                &format!(
                    "核对目录 {} 的该层是否存在及访问权限；不要改用其他配置来源",
                    directory.display()
                ),
            )
        })?;
        if !ordinary(&metadata) || !metadata.is_dir() {
            return Err(diagnostic(
                stage,
                path,
                "目录层级不是普通无链接目录，或包含链接/重解析点",
                "使用真实目录并检查各层路径；不能通过链接进入共享源或私有目标",
            ));
        }
    }
    Ok(())
}

fn ordinary(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return false;
        }
    }
    true
}

fn stamp(metadata: &fs::Metadata) -> Stamp {
    Stamp {
        length: metadata.len(),
        modified: metadata.modified().ok(),
        created: metadata.created().ok(),
    }
}

fn publish_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file: File = OpenOptions::new().write(true).create_new(true).open(path).map_err(|error| {
        io_error("发布私有配置/创建文件", path, &error,
            "本次目标目录已创建，当前文件创建未获成功确认；已有内容及其他部分文件保留，核对目标与权限，不覆盖、不自动重放")
    })?;
    file.write_all(bytes).map_err(|error| {
        io_error(
            "发布私有配置/写入文件",
            path,
            &error,
            "目标文件已创建，可能只写入部分字节；部分文件保留，核对磁盘空间及写入权限，不自动重放",
        )
    })?;
    file.sync_all().map_err(|error| {
        io_error(
            "发布私有配置/持久化文件",
            path,
            &error,
            "目标文件已创建并写入，持久化尚未确认；文件保留，核对设备状态，不自动重放",
        )
    })
}

fn diagnostic(stage: &str, path: &Path, reason: &str, action: &str) -> String {
    format!(
        "阶段={stage}；路径={}；原因={reason}；处理={action}",
        path.display()
    )
}

fn io_error(stage: &str, path: &Path, error: &io::Error, action: &str) -> String {
    diagnostic(
        stage,
        path,
        &format!(
            "IOkind={:?}；rawcode={:?}",
            error.kind(),
            error.raw_os_error()
        ),
        action,
    )
}

fn changed_error(stage: &str, path: &Path) -> String {
    diagnostic(
        stage,
        path,
        "文件类型、字节或元数据与捕获时不一致",
        "停止导入，核对文件与外层 fixture 的绑定并保留现场；本次不重新捕获、不切换来源、不自动重放",
    )
}

fn field_error(path: &Path, field: &str) -> String {
    diagnostic(
        "准备私有配置/修改schema",
        path,
        &format!("字段 {field} 必须是 TOML 表"),
        "在编辑器中把该字段改为对应的 [database] 或 [database.primary] 表，保留其余连接字段",
    )
}

fn retained_target(error: String, directory: &Path) -> String {
    format!(
        "{error}；本次目标目录已创建={}，已有或部分文件保留；先人工核对，不自动重放",
        directory.display()
    )
}

#[cfg(test)]
#[path = "configuration_tests.rs"]
mod tests;
