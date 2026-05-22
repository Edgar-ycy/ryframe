use async_trait::async_trait;
use ryframe_common::AppResult;

use crate::task_manager::{ScheduledTask, TaskContext};

/// 清理临时文件任务
///
/// 每周日 04:00 清理 ./tmp/ 目录下 7 天前的临时文件
pub struct CleanTempFilesTask;

#[async_trait]
impl ScheduledTask for CleanTempFilesTask {
    fn name(&self) -> &str {
        "clean_temp_files"
    }

    fn cron(&self) -> &str {
        "0 0 4 * * 7"
    }

    fn description(&self) -> &str {
        "每周日 04:00 清理 7 天前的临时文件"
    }

    async fn execute(&self, _ctx: &TaskContext) -> AppResult<String> {
        let tmp_dir = std::path::Path::new("./tmp");
        if !tmp_dir.exists() {
            return Ok("临时文件目录不存在，无需清理".into());
        }

        let cutoff = chrono::Utc::now() - chrono::Duration::days(7);
        let mut deleted_count = 0u64;
        let mut total_size = 0u64;

        if let Ok(entries) = std::fs::read_dir(tmp_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file()
                    && let Ok(metadata) = path.metadata()
                        && let Ok(modified) = metadata.modified() {
                            let modified_time = {
                                let duration = modified
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default();
                                chrono::DateTime::from_timestamp(
                                    duration.as_secs() as i64,
                                    duration.subsec_nanos(),
                                )
                            };
                            if let Some(mt) = modified_time
                                && mt < cutoff {
                                    total_size += metadata.len();
                                    if std::fs::remove_file(&path).is_ok() {
                                        deleted_count += 1;
                                    }
                                }
                        }
            }
        }

        Ok(format!(
            "清理临时文件完成, 删除 {} 个文件, 释放 {} 字节",
            deleted_count, total_size
        ))
    }
}
