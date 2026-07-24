mod spec;
mod table;
mod value;

use ryframe_common::AppResult;

use self::spec::ENV_OVERRIDES;

pub(super) fn apply_env_overrides(table: &mut toml::Table) -> AppResult<()> {
    for spec in ENV_OVERRIDES {
        let Ok(raw_value) = std::env::var(spec.name) else {
            continue;
        };
        let parsed_value = value::parse(spec.name, &raw_value, spec.value_type)?;
        table::insert(table, spec.path, parsed_value);
    }

    Ok(())
}
