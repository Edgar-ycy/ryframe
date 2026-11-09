use std::{fs, io::Write, path::Path, process::Command};

use sea_orm::{ConnectionTrait, DatabaseConnection};

use super::configuration_fixture;

pub(super) const ADMIN_PASSWORD: &str = "ExplicitAdmin123!";

pub(super) struct Export {
    directory: Option<tempfile::TempDir>,
}

impl Export {
    pub(super) fn finish(mut self) -> Result<(), String> {
        let directory = self.directory.take().ok_or("私有 SQL 验收资源已释放")?;
        let path = directory.path().to_owned();
        directory.close().map_err(|_| {
            format!(
                "清理私有 SQL 验收材料失败；请核对目录权限，剩余现场保留于 {}",
                path.display()
            )
        })
    }
}

impl Drop for Export {
    fn drop(&mut self) {
        if let Some(directory) = self.directory.take() {
            let path = directory.keep();
            let _ = writeln!(
                std::io::stderr(),
                "私有 SQL 验收现场保留于 {}",
                path.display()
            );
        }
    }
}

pub(super) async fn import(database: &DatabaseConnection) -> Result<Export, String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .ok_or("无法定位后端工作区；请从正式测试入口运行")?;
    let parent = root.join(".local-tests");
    fs::create_dir_all(&parent).map_err(|_| "无法准备私有 SQL 测试目录；请检查工作区权限")?;
    let directory = tempfile::Builder::new()
        .prefix("account-sql-")
        .tempdir_in(&parent)
        .map_err(|_| "无法创建私有 SQL 测试目录；请检查工作区权限")?;
    let config = directory.path().join("config");
    let sql = directory.path().join("initialization.sql");
    let resources = Export {
        directory: Some(directory),
    };
    configuration_fixture::write_configuration(&config, 8080, 9091, Some(ADMIN_PASSWORD));
    let result = async {
        let output = Command::new(env!("CARGO_BIN_EXE_ryframe-migrate"))
            .current_dir(root)
            .arg("--config-dir")
            .arg(&config)
            .args(["control", "export-sql", "--output"])
            .arg(&sql)
            .arg("--write")
            .env("APP_ENV", "test")
            .output()
            .map_err(|error| {
                format!(
                    "无法启动正式 SQL 导出工具（{:?}）；请检查测试二进制和执行权限",
                    error.kind()
                )
            })?;
        if !output.status.success() {
            return Err(format!(
                "正式 SQL 导出失败（{}）；请核对私有配置与导出目录权限，不自动重放",
                output.status
            ));
        }
        let content = fs::read_to_string(&sql)
            .map_err(|_| "无法读取正式导出的 UTF-8 SQL；请核对私有文件完整性")?;
        database
            .execute_unprepared(&content)
            .await
            .map_err(|_| "正式 SQL 导入失败；请核对保留的私有导出与数据库现场，不自动重放")?;
        ryframe_db::migration::verify(database)
            .await
            .map_err(|_| "SQL 导入后控制库校验失败；请核对保留现场，不自动重放".to_owned())?;
        ryframe_tenant_db::migration::verify_shared_control(database)
            .await
            .map_err(|_| {
                "SQL 导入后 shared-control 校验失败；禁止补 up 掩盖导出缺口，保留现场".to_owned()
            })
    }
    .await;
    result?;
    Ok(resources)
}
