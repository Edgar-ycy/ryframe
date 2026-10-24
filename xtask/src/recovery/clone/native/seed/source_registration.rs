//! 来源登记的完整历史和受控文件发布；由同一原生闭环共享证据。
#[path = "source_registration/anchors.rs"]
pub(crate) mod anchors;
#[path = "source_registration/close.rs"]
pub(crate) mod close;
#[path = "source_registration/copied.rs"]
pub(crate) mod copied;
#[path = "source_registration/identity.rs"]
pub(crate) mod identity;

#[path = "source_registration/lineage.rs"]
pub(crate) mod lineage;
#[path = "source_registration/materialize.rs"]
pub(crate) mod materialize;
#[path = "source_registration/publication.rs"]
pub(crate) mod publication;
#[path = "source_registration/quiet.rs"]
pub(crate) mod quiet;
#[path = "source_registration/registry.rs"]
pub(crate) mod registry;
#[path = "source_registration/request.rs"]
pub(crate) mod request;
#[path = "source_registration/runner.rs"]
pub(crate) mod runner;
#[path = "source_registration/storage.rs"]
pub(crate) mod storage;
