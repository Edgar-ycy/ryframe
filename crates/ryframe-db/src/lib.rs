pub mod connection;
pub mod entities;
pub mod migration;
pub mod pagination;
pub mod repositories;
pub mod transaction;

// 便捷导出
pub use entities::{
    config, dept, dict_data, dict_type, job, job_log, login_info, menu, notice, oper_log,
    permission, post, role, role_dept, role_menu, role_permission, user, user_role,
};
pub use repositories::{
    ConfigRepository, DeptRepository, DictDataRepository, DictTypeRepository, JobLogRepository,
    JobRepository, LoginInfoRepository, MenuRepository, NoticeRepository, OperLogRepository,
    PermissionRepository, PostRepository, RoleRepository, UserRepository,
};
