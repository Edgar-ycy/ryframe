use std::{
    cell::RefCell,
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Instant,
};

use crate::Result;

thread_local! {
    static PROCESS_LOG: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

pub(super) fn process_log_active() -> bool {
    PROCESS_LOG.with(|slot| slot.borrow().is_some())
}

pub(crate) fn with_process_log<T>(
    label: &str,
    path: &Path,
    action: impl FnOnce() -> Result<T>,
) -> Result<T> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, [])?;
    println!("→ 并行任务 {label}（日志：{}）", path.display());
    let previous = PROCESS_LOG.with(|slot| slot.replace(Some(path.to_path_buf())));
    let started = Instant::now();
    let result = action();
    PROCESS_LOG.with(|slot| {
        slot.replace(previous);
    });
    match result {
        Ok(value) => {
            println!("✓ 并行任务 {label} {:.1}s", started.elapsed().as_secs_f64());
            Ok(value)
        }
        Err(error) => {
            eprintln!("并行任务 {label} 失败，完整日志如下：");
            if let Ok(log) = fs::read_to_string(path) {
                eprintln!("{log}");
            }
            Err(error)
        }
    }
}

pub(super) fn configure_output(command: &mut Command) -> Result<()> {
    PROCESS_LOG.with(|slot| {
        let Some(path) = slot.borrow().as_ref().cloned() else {
            command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
            return Ok(());
        };
        let stdout = OpenOptions::new().create(true).append(true).open(&path)?;
        let stderr = stdout.try_clone()?;
        command
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        Ok(())
    })
}
