//! 系统管理资源的查询和写入端口。

mod config;
mod dept;
mod dict;
mod login_info;
mod menu;
mod message;
mod notice;
mod oper_log;
mod overview;
mod permission;
mod post;
mod role_read;
mod role_write;

pub use config::{ConfigFilter, ConfigPersistencePort, ConfigRecord, ConfigTransaction};
pub use dept::{
    DeptFilter, DeptReadPort, DeptRecord, DeptTreeRecord, DeptWritePort, DeptWriteTransaction,
};
pub use dict::{
    DictDataRecord, DictPersistencePort, DictTransaction, DictTypeFilter, DictTypeRecord,
};
pub use login_info::{
    LoginInfoFilter, LoginInfoPersistencePort, LoginInfoRecord, LoginInfoTransaction,
};
pub use menu::{
    MenuFilter, MenuReadPort, MenuRecord, MenuTreeRecord, MenuWritePort, MenuWriteTransaction,
};
pub use message::{
    MessageAudienceRecord, MessageAudienceRecordKind, MessageInboxFilter, MessageOutboxRecord,
    MessagePage, MessagePersistencePort, MessageRecipientRecord, MessageRecord, MessageTransaction,
    PublishMessageRecord, PublishedMessageRecord,
};
pub use notice::{NoticeFilter, NoticePersistencePort, NoticeRecord, NoticeTransaction};
pub use oper_log::{OperLogFilter, OperLogPersistencePort, OperLogRecord, OperLogTransaction};
pub use overview::{
    OverviewPersistencePort, OverviewTrendCount, OverviewTrendSeries, ScheduleOverviewStats,
};
pub use permission::{
    PermissionReadPort, PermissionRecord, PermissionWritePort, PermissionWriteTransaction,
};
pub use post::{PostFilter, PostPersistencePort, PostRecord, PostTransaction};
pub use role_read::{RoleFilter, RoleReadPort, RoleRecord};
pub use role_write::{RolePermissionRef, RoleWritePort, RoleWriteTransaction};
