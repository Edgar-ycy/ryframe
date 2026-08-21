//! 用户资料、查询、写入和批量导入端口。

mod import;
mod profile;
mod query;
mod write;

pub use import::{
    NewImportedUser, NewUserImportJob, NewUserImportRow, UserImportAuthorizationSnapshot,
    UserImportDepartmentRecord, UserImportJobRecord, UserImportPersistencePort,
    UserImportReadFilter, UserImportRowRecord, UserImportSourceRecord, UserImportSourceState,
    UserImportTransaction,
};
pub use profile::{
    ProfileAvatarFile, ProfileAvatarState, ProfilePersistencePort, ProfileRecord,
    ProfileTransaction, ProfileUserState,
};
pub use query::{
    USER_QUERY_STATUS_NORMAL, UserQueryDetailRecord, UserQueryFilter, UserQueryReadPort,
    UserQueryRecord, UserQueryRoleRecord,
};
pub use write::{
    ManageableUserState, NewUserRecord, USER_STATUS_DISABLED, USER_STATUS_MUST_RESET_PASSWORD,
    USER_STATUS_NORMAL, USER_STATUS_PENDING_ACTIVATION, UpdateUserRecord, UserAssignmentRole,
    UserAssignmentState, UserWritePersistencePort, UserWriteRecord, UserWriteTransaction,
};
