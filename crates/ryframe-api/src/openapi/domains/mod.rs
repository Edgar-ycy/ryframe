use utoipa::openapi::OpenApi;

mod auth;
mod configuration;
mod identity;
mod operations;
mod platform;
mod service;

pub(super) fn merge(document: &mut OpenApi) {
    document.merge(auth::document());
    document.merge(identity::document());
    document.merge(configuration::document());
    document.merge(operations::document());
    document.merge(platform::document());
    document.merge(service::document());
}
