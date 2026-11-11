#[cfg(windows)]
use crate::Result;
#[cfg(windows)]
use std::path::Path;
#[cfg(windows)]
use winsafe::{HPROCESS, STARTUPINFO, co, guard::CloseHandlePiGuard, prelude::kernel_Hprocess};

/// 从已约束环境的跳板启动持久进程，继承当前环境与工作目录，不继承句柄。
#[cfg(windows)]
pub(crate) fn spawn_detached(executable: &Path, args: &[&str]) -> Result<u32> {
    Ok(spawn_detached_with_handle(executable, args)?.dwProcessId)
}

#[cfg(windows)]
pub(crate) fn spawn_detached_with_handle(
    executable: &Path,
    args: &[&str],
) -> Result<CloseHandlePiGuard> {
    let executable = executable.to_str().ok_or("持久进程程序路径不是 UTF-8")?;
    let line = command_line(executable, args);
    let mut startup = STARTUPINFO::default();
    let base = co::CREATE::NO_WINDOW | co::CREATE::UNICODE_ENVIRONMENT;
    // winsafe 0.0.19 的自定义环境缓冲区生命周期不可靠，沿用跳板继承入口。
    let create = |flags, startup: &mut STARTUPINFO| {
        HPROCESS::CreateProcess(
            Some(executable),
            Some(&line),
            None,
            None,
            false,
            flags,
            None,
            None,
            startup,
        )
    };
    let child = match create(base | co::CREATE::BREAKAWAY_FROM_JOB, &mut startup) {
        Ok(child) => child,
        Err(co::ERROR::ACCESS_DENIED) => create(base, &mut startup)?,
        Err(error) => return Err(error.into()),
    };
    Ok(child)
}

fn command_line(executable: &str, args: &[&str]) -> String {
    let mut line = String::new();
    quote_arg(&mut line, executable);
    for arg in args {
        line.push(' ');
        quote_arg(&mut line, arg);
    }
    line
}

fn quote_arg(line: &mut String, value: &str) {
    line.push('"');
    let mut slashes = 0;
    for character in value.chars() {
        if character == '\\' {
            slashes += 1;
            continue;
        }
        let count = if character == '"' {
            slashes * 2 + 1
        } else {
            slashes
        };
        line.extend(std::iter::repeat_n('\\', count));
        slashes = 0;
        line.push(character);
    }
    line.extend(std::iter::repeat_n('\\', slashes * 2));
    line.push('"');
}

#[cfg(test)]
mod tests {
    use super::command_line;

    #[test]
    fn command_line_preserves_empty_whitespace_backslashes_and_quotes() {
        let executable = r"D:\tool dir\xtask.exe";
        assert_eq!(command_line(executable, &[]), r#""D:\tool dir\xtask.exe""#);
        assert_eq!(
            command_line(
                executable,
                &[
                    "",
                    "two words",
                    "one\ttwo",
                    r"some\path\",
                    r#"slash\"quote"#
                ],
            ),
            concat!(
                r#""D:\tool dir\xtask.exe" "" "two words" "#,
                "\"one\ttwo\" ",
                r#""some\path\\" "slash\\\"quote""#,
            ),
        );
    }
}
