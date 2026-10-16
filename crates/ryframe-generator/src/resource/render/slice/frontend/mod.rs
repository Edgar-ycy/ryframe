mod modules;
mod presentation;
mod resource;
mod split;
mod split_form;
mod split_presentation;
mod transport;
mod values;

use super::super::super::{FieldIr, ResourceIr, ValueType, WidgetIr};
use super::super::{html_header, slash_header};

pub(super) use transport::{api, page, registration};

pub(super) fn fields(resource: &ResourceIr) -> Vec<(String, String)> {
    let content = presentation::fields(resource);
    if content.lines().count() <= 300 {
        vec![("fields.ts".into(), content)]
    } else {
        split::render(resource)
    }
}

fn has_date_column(resource: &ResourceIr) -> bool {
    resource.fields.iter().any(|field| {
        field.usage.list && field.enum_values.is_empty() && field.value_type == ValueType::DateTime
    })
}

#[cfg(test)]
mod tests;
