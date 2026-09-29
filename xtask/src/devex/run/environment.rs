use std::collections::BTreeMap;

use super::super::{metadata::inherited_environment, model::SuiteDefinition};

pub(super) fn effective_environment(definition: SuiteDefinition) -> BTreeMap<String, String> {
    let mut environment = inherited_environment();
    for key in definition.remove_environment {
        environment.remove(*key);
    }
    environment.extend(
        definition
            .environment
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned())),
    );
    environment
}
