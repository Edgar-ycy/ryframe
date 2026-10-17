use regex::Regex;

use super::{RunBlock, display_untrusted};

pub(super) fn validate_run_commands(run_blocks: &[RunBlock]) -> Vec<String> {
    let mut errors = Vec::new();
    let mut cargo_cyclonedx = false;
    let mut trivy_cyclonedx = false;
    let mut cyclonedx_reports = 0;
    let mut reproducible_cyclonedx_reports = 0;
    let mut trivy_reports = 0;

    for block in run_blocks {
        for line in logical_command_lines(&block.source) {
            if line.starts_with('#') {
                continue;
            }
            let segments = match shell_command_segments(&line) {
                Ok(segments) => segments,
                Err(error) => {
                    if re(
                        r"\b(?:cargo\s+cyclonedx|trivy\s+image|check_supply_chain\.py|docker\s+(?:run|pull))\b",
                    )
                    .is_match(&line)
                    {
                        errors.push(format!(
                            "{}: {}.run 无法安全解析命令：{error}",
                            block.path.display(),
                            block.location
                        ));
                    }
                    continue;
                }
            };
            for (segment_index, raw_tokens) in segments.iter().enumerate() {
                let tokens = command_tokens(raw_tokens);
                let gate_tokens = if segment_index == 0 {
                    gate_command_tokens(raw_tokens)
                } else {
                    Vec::new()
                };
                if gate_tokens.len() >= 2
                    && gate_tokens[0] == "cargo"
                    && gate_tokens[1] == "cyclonedx"
                {
                    if has_option(&gate_tokens, "--format", "json") {
                        cargo_cyclonedx = true;
                    }
                    continue;
                }
                if gate_tokens.len() >= 2 && gate_tokens[0] == "trivy" && gate_tokens[1] == "image"
                {
                    if has_option(&gate_tokens, "--format", "cyclonedx") {
                        trivy_cyclonedx = true;
                    }
                    continue;
                }
                let report = typed_security_report(&gate_tokens);
                if report.kind.is_some() || !report.errors.is_empty() {
                    let report_has_errors = !report.errors.is_empty();
                    errors.extend(report.errors.into_iter().map(|error| {
                        format!(
                            "{}: {}.run 的 {error}",
                            block.path.display(),
                            block.location
                        )
                    }));
                    if !report_has_errors && report.kind == Some("cyclonedx") {
                        cyclonedx_reports += 1;
                        reproducible_cyclonedx_reports += usize::from(report.reproducible);
                    }
                    if !report_has_errors && report.kind == Some("trivy") {
                        trivy_reports += 1;
                    }
                    continue;
                }
                if gate_tokens.len() >= 2
                    && matches!(
                        gate_tokens[0].as_str(),
                        "python" | "python.exe" | "python3" | "py"
                    )
                    && matches!(
                        gate_tokens[1].as_str(),
                        "tools/python/check_supply_chain.py"
                            | "./tools/python/check_supply_chain.py"
                    )
                    && (has_value_option(&gate_tokens, "--trivy-report")
                        || has_value_option(&gate_tokens, "--cyclonedx"))
                {
                    errors.push(format!(
                        "{}: {}.run 禁止直接调用 check_supply_chain.py 核验报告；必须使用 cargo xtask check ci security report",
                        block.path.display(),
                        block.location
                    ));
                    continue;
                }
                if tokens.len() >= 2
                    && tokens[0] == "docker"
                    && matches!(tokens[1].as_str(), "pull" | "run")
                {
                    match docker_image(&tokens) {
                        Ok(Some(image)) if service_image_ref(&image) => {}
                        Ok(Some(_)) => errors.push(format!(
                            "{}: {}.run 的 docker {} 镜像未固定到 sha256 digest",
                            block.path.display(),
                            block.location,
                            tokens[1],
                        )),
                        Ok(None) => errors.push(format!(
                            "{}: {}.run 的 docker {} 镜像未固定到 sha256 digest",
                            block.path.display(),
                            block.location,
                            tokens[1]
                        )),
                        Err(error) => errors.push(format!(
                            "{}: {}.run 的 docker {} 无法安全识别镜像：{error}",
                            block.path.display(),
                            block.location,
                            tokens[1]
                        )),
                    }
                }
            }
        }
    }

    if !cargo_cyclonedx {
        errors.push("工作流缺少供应链门禁：cargo cyclonedx --format json".into());
    }
    if !trivy_cyclonedx {
        errors.push("工作流缺少供应链门禁：trivy image --format cyclonedx".into());
    }
    if cyclonedx_reports != 2 {
        errors.push(
            "工作流必须恰好两次使用 cargo xtask check ci security report cyclonedx --input <报告>，分别核验 Cargo 与镜像清单"
                .into(),
        );
    }
    if reproducible_cyclonedx_reports != 1 {
        errors.push("工作流必须恰好一次为 Cargo CycloneDX 核验使用 --require-reproducible".into());
    }
    if trivy_reports != 1 {
        errors.push(
            "工作流必须恰好一次使用 cargo xtask check ci security report trivy --input <报告>"
                .into(),
        );
    }
    errors
}

fn logical_command_lines(source: &str) -> Vec<String> {
    let normalization = Regex::new(r"(?:\\|`|\^)\r?\n[ \t]*").expect("固定命令续行正则");
    normalization
        .replace_all(source, " ")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

fn shell_command_segments(line: &str) -> std::result::Result<Vec<Vec<String>>, String> {
    let mut segments = Vec::new();
    let mut current = Vec::new();
    let mut token = String::new();
    let mut token_started = false;
    let mut quote = None;
    let mut escaped = false;
    let mut characters = line.chars().peekable();
    while let Some(character) = characters.next() {
        if escaped {
            token.push(character);
            token_started = true;
            escaped = false;
            continue;
        }
        if let Some(current_quote) = quote {
            match current_quote {
                '\'' if character == '\'' => quote = None,
                '"' if character == '"' => quote = None,
                '"' if character == '\\' => escaped = true,
                _ => token.push(character),
            }
            continue;
        }
        match character {
            '\\' => {
                escaped = true;
                token_started = true;
            }
            '\'' | '"' => {
                quote = Some(character);
                token_started = true;
            }
            '#' if !token_started => break,
            character if character.is_whitespace() => {
                flush_token(&mut current, &mut token, &mut token_started);
            }
            '(' | ')' | ';' | '&' | '|' => {
                flush_token(&mut current, &mut token, &mut token_started);
                if !current.is_empty() {
                    segments.push(std::mem::take(&mut current));
                }
                while characters
                    .peek()
                    .is_some_and(|value| matches!(value, '(' | ')' | ';' | '&' | '|'))
                {
                    characters.next();
                }
            }
            character => {
                token.push(character);
                token_started = true;
            }
        }
    }
    if quote.is_some() {
        return Err("存在未闭合的引号".into());
    }
    if escaped {
        return Err("命令以未完成的转义结尾".into());
    }
    flush_token(&mut current, &mut token, &mut token_started);
    if !current.is_empty() {
        segments.push(current);
    }
    Ok(segments)
}

fn flush_token(current: &mut Vec<String>, token: &mut String, started: &mut bool) {
    if *started {
        current.push(std::mem::take(token));
        *started = false;
    }
}

fn command_tokens(tokens: &[String]) -> Vec<String> {
    let mut command = tokens.to_vec();
    while let Some(first) = command.first() {
        if matches!(
            first.as_str(),
            "!" | "command" | "do" | "elif" | "if" | "sudo" | "then" | "until" | "while"
        ) || assignment(first)
        {
            command.remove(0);
        } else {
            break;
        }
    }
    command
}

fn gate_command_tokens(tokens: &[String]) -> Vec<String> {
    let mut command = tokens
        .iter()
        .skip_while(|token| assignment(token))
        .cloned()
        .collect::<Vec<_>>();
    if command.first().is_some_and(|token| token == "command") {
        command.remove(0);
    }
    command
}

fn assignment(value: &str) -> bool {
    re(r"^[A-Za-z_][A-Za-z0-9_]*=.*$").is_match(value)
}

fn has_option(tokens: &[String], name: &str, value: &str) -> bool {
    tokens.iter().enumerate().any(|(index, token)| {
        token == &format!("{name}={value}")
            || token == name && tokens.get(index + 1).is_some_and(|next| next == value)
    })
}

fn has_value_option(tokens: &[String], name: &str) -> bool {
    tokens.iter().enumerate().any(|(index, token)| {
        token.starts_with(&format!("{name}=")) && token != &format!("{name}=")
            || token == name
                && tokens
                    .get(index + 1)
                    .is_some_and(|next| !next.starts_with('-'))
    })
}

struct TypedReport<'a> {
    kind: Option<&'a str>,
    reproducible: bool,
    errors: Vec<String>,
}

fn typed_security_report(tokens: &[String]) -> TypedReport<'_> {
    const PREFIX: [&str; 6] = ["cargo", "xtask", "check", "ci", "security", "report"];
    if tokens
        .iter()
        .take(PREFIX.len())
        .map(String::as_str)
        .ne(PREFIX)
    {
        return TypedReport {
            kind: None,
            reproducible: false,
            errors: Vec::new(),
        };
    }
    let mut errors = Vec::new();
    let Some(kind) = tokens.get(PREFIX.len()).map(String::as_str) else {
        return TypedReport {
            kind: None,
            reproducible: false,
            errors: vec!["security report 缺少报告类型".into()],
        };
    };
    if !matches!(kind, "cyclonedx" | "trivy") {
        return TypedReport {
            kind: None,
            reproducible: false,
            errors: vec![format!("security report 类型无效：{kind}")],
        };
    }
    let mut input_seen = false;
    let mut reproducible = false;
    let mut index = PREFIX.len() + 1;
    while index < tokens.len() {
        match tokens[index].as_str() {
            "--input" => {
                if input_seen {
                    errors.push("security report --input 不能重复".into());
                }
                if tokens
                    .get(index + 1)
                    .is_none_or(|value| value.starts_with('-'))
                {
                    errors.push("security report --input 缺少取值".into());
                    index += 1;
                } else {
                    input_seen = true;
                    index += 2;
                }
            }
            "--require-reproducible" => {
                if kind != "cyclonedx" {
                    errors.push("security report trivy 不支持 --require-reproducible".into());
                } else if reproducible {
                    errors.push("security report --require-reproducible 不能重复".into());
                }
                reproducible = true;
                index += 1;
            }
            option => {
                errors.push(format!("security report 包含未知参数：{option}"));
                index += 1;
            }
        }
    }
    if !input_seen {
        errors.push("security report 缺少 --input <报告>".into());
    }
    TypedReport {
        kind: Some(kind),
        reproducible,
        errors,
    }
}

fn docker_image(tokens: &[String]) -> std::result::Result<Option<String>, String> {
    let operation = tokens.get(1).map(String::as_str).unwrap_or("");
    let (flags, values) = if operation == "run" {
        (DOCKER_RUN_FLAGS, DOCKER_RUN_VALUES)
    } else {
        (DOCKER_PULL_FLAGS, DOCKER_PULL_VALUES)
    };
    let mut index = 2;
    while index < tokens.len() {
        let token = &tokens[index];
        if token == "--" {
            index += 1;
            break;
        }
        if flags.contains(&token.as_str()) {
            index += 1;
            continue;
        }
        if values.contains(&token.as_str()) {
            if index + 1 >= tokens.len() {
                return Err(format!("选项 {token} 缺少值"));
            }
            index += 2;
            continue;
        }
        if token.starts_with("--") && token.contains('=') {
            index += 1;
            continue;
        }
        if token.starts_with('-') {
            return Err(format!("无法识别选项 {}", display_untrusted(token)));
        }
        return Ok(Some(token.clone()));
    }
    Ok(tokens.get(index).cloned())
}

fn service_image_ref(value: &str) -> bool {
    re(r"^[A-Za-z0-9][A-Za-z0-9._:/-]*@sha256:[0-9a-f]{64}$").is_match(value)
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("固定供应链命令正则")
}

const DOCKER_RUN_FLAGS: &[&str] = &[
    "--detach",
    "--init",
    "--interactive",
    "--privileged",
    "--read-only",
    "--rm",
    "--sig-proxy",
    "--tty",
    "-d",
    "-i",
    "-t",
];
const DOCKER_RUN_VALUES: &[&str] = &[
    "--add-host",
    "--cap-add",
    "--cap-drop",
    "--cpus",
    "--entrypoint",
    "--env",
    "--env-file",
    "--hostname",
    "--label",
    "--memory",
    "--mount",
    "--name",
    "--network",
    "--platform",
    "--publish",
    "--pull",
    "--restart",
    "--security-opt",
    "--user",
    "--volume",
    "--workdir",
    "-e",
    "-h",
    "-l",
    "-m",
    "-p",
    "-u",
    "-v",
    "-w",
];
const DOCKER_PULL_FLAGS: &[&str] = &["--all-tags", "--quiet", "-a", "-q"];
const DOCKER_PULL_VALUES: &[&str] = &["--platform"];
