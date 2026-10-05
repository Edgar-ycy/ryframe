pub mod resources;

/// 为离线生成器提供手写资源模型的稳定描述符目录。
pub fn resource_descriptors() -> Vec<ryframe_sdk::ResourceDescriptor> {
    vec![<resources::Order as ryframe_sdk::ResourceModel>::descriptor()]
}

#[cfg(any(feature = "api", feature = "migration"))]
pub mod generated;

#[cfg(any(feature = "api", feature = "migration"))]
pub fn module() -> ryframe_sdk::RyFrameBusinessModule {
    let builder = ryframe_sdk::BusinessModuleBuilder::new("order")
        .resources(&generated::RESOURCES);
    #[cfg(feature = "api")]
    let builder = builder.routes(generated::routes).openapi(generated::openapi);
    #[cfg(feature = "migration")]
    let builder = builder.migrations(generated::migrations::migrations());
    builder.build()
}
