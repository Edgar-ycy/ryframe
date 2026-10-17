use std::path::PathBuf;

use crate::check::language::{LanguageOptions, Percent, resolve_output};

use super::{CliError, set_once, take_unique_option};

pub(super) fn parse_language(args: &[String]) -> Result<LanguageOptions, CliError> {
    let mut args = args.to_vec();
    let output = take_unique_option(&mut args, "--output")?.map(PathBuf::from);
    let minimum = take_unique_option(&mut args, "--min-rust-percent")?
        .map(|value| Percent::parse(&value).map_err(|error| CliError::new(error.to_string())))
        .transpose()?;
    let mut options = LanguageOptions {
        output,
        min_rust_percent: minimum,
        ..LanguageOptions::default()
    };
    for argument in args {
        match argument.as_str() {
            "--write" => set_once(&mut options.write, "--write")?,
            "--plan" => set_once(&mut options.plan, "--plan")?,
            _ => return Err(CliError::new(format!("未知 language 参数：{argument}"))),
        }
    }
    resolve_output(&crate::workspace::root_dir(), &options)
        .map_err(|error| CliError::new(error.to_string()))?;
    Ok(options)
}
