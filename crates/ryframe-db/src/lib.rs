pub mod connection;
pub mod entities;
pub mod pagination;
pub mod repositories;
pub mod sql_logger;
pub use sql_logger::{DbSpanLayer, SqlLogLayer};
pub mod transaction;

// 便捷导出
pub use entities::{
    config, dept, dict_data, dict_type, login_info, menu, notice, oper_log, permission, post, role,
    role_dept, role_menu, role_permission, sys_file, user, user_role,
};
pub use repositories::{
    ConfigRepository, DeptRepository, DictDataRepository, DictTypeRepository, FileRepository,
    LoginInfoRepository, MenuRepository, NoticeRepository, OperLogRepository, PermissionRepository,
    PostRepository, RoleRepository, UserRepository,
};
