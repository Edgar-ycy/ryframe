use crate::{Result, process::run_owned, workspace::root_dir};

pub(crate) fn run(arguments: &[String]) -> Result<()> {
    let mut command = Vec::with_capacity(arguments.len() + 1);
    command.push("scripts/restore_reference.py".to_owned());
    command.extend(arguments.iter().cloned());
    run_owned(&root_dir(), "python", &command)
}
