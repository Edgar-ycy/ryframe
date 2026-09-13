use std::{env, process::Command};

const UTF8_MODE: &str = "PYTHONUTF8";
const IO_ENCODING: &str = "PYTHONIOENCODING";

pub(crate) fn python_arguments<'a>(executable: &str, arguments: &[&'a str]) -> Vec<&'a str> {
    if executable != "python" {
        return arguments.to_vec();
    }
    let mut normalized = Vec::with_capacity(arguments.len() + 2);
    normalized.extend(["-X", "utf8"]);
    let mut index = 0;
    while index < arguments.len() {
        let argument = arguments[index];
        if is_execution_target(argument) {
            normalized.extend_from_slice(&arguments[index..]);
            break;
        }
        if argument == "-X" {
            let Some(option) = arguments.get(index + 1).copied() else {
                normalized.push(argument);
                break;
            };
            if !is_utf8_option(option) {
                normalized.extend([argument, option]);
            }
            index += 2;
            continue;
        }
        if matches!(argument, "-W" | "--check-hash-based-pycs") {
            normalized.push(argument);
            if let Some(value) = arguments.get(index + 1).copied() {
                normalized.push(value);
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }
        if argument.strip_prefix("-X").is_some_and(is_utf8_option) {
            index += 1;
            continue;
        }
        normalized.push(argument);
        index += 1;
    }
    normalized
}

pub(crate) fn python_environment<'a>(
    executable: &str,
    environment: &[(&'a str, &'a str)],
) -> Vec<(&'a str, &'a str)> {
    if executable != "python" {
        return environment.to_vec();
    }
    let mut normalized = environment
        .iter()
        .copied()
        .filter(|(name, _)| !is_encoding_variable(name))
        .collect::<Vec<_>>();
    normalized.extend([(UTF8_MODE, "1"), (IO_ENCODING, "utf-8")]);
    normalized
}

pub(super) fn configure_environment(
    command: &mut Command,
    executable: &str,
    environment: &[(&str, &str)],
    removed_environment: &[&str],
) {
    for name in removed_environment {
        command.env_remove(name);
    }
    if executable == "python" {
        for (name, _) in env::vars_os() {
            if name.to_str().is_some_and(is_encoding_variable) {
                command.env_remove(name);
            }
        }
    }
    command.envs(python_environment(executable, environment));
}

fn is_encoding_variable(name: &str) -> bool {
    name.eq_ignore_ascii_case(UTF8_MODE) || name.eq_ignore_ascii_case(IO_ENCODING)
}

fn is_execution_target(argument: &str) -> bool {
    matches!(argument, "-" | "--" | "-c" | "-m") || !argument.starts_with('-')
}

fn is_utf8_option(option: &str) -> bool {
    let name = option.split_once('=').map_or(option, |(name, _)| name);
    name.eq_ignore_ascii_case("utf8")
}
