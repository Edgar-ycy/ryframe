-- RyFrame Database Backup
-- Generated at: 2026-06-04T02:59:59.813907100+00:00

-- Table: sys_user (2 rows)
-- {"id":1,"username":"admin","nickname":"超级管理员","email":"admin@ryframe.com","phone":"13800000000","avatar":null,"status":"1","dept_id":1,"remark":null,"login_ip":null,"login_date":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:21Z"}
-- {"id":2,"username":"user","nickname":"普通用户","email":"user@ryframe.com","phone":"13800000001","avatar":null,"status":"1","dept_id":5,"remark":null,"login_ip":null,"login_date":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:21Z"}

-- Table: sys_role (2 rows)
-- {"id":1,"name":"超级管理员","code":"admin","data_scope":"1","status":"1","sort":1,"remark":"超级管理员，拥有所有权限","del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":2,"name":"普通用户","code":"common","data_scope":"5","status":"1","sort":2,"remark":"普通用户，拥有基础权限","del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}

-- Table: sys_menu (23 rows)
-- {"id":0,"name":"首页","parent_id":null,"menu_type":"C","path":"/dashboard","component":"dashboard/index","query":null,"perms":null,"icon":"HomeFilled","is_frame":false,"is_cache":false,"sort":0,"visible":true,"status":"1","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":1,"name":"系统管理","parent_id":null,"menu_type":"M","path":"/system","component":"Layout","query":null,"perms":null,"icon":"Setting","is_frame":false,"is_cache":false,"sort":1,"visible":true,"status":"1","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":2,"name":"系统监控","parent_id":null,"menu_type":"M","path":"/monitor","component":"Layout","query":null,"perms":null,"icon":"Monitor","is_frame":false,"is_cache":false,"sort":2,"visible":true,"status":"1","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":3,"name":"系统工具","parent_id":null,"menu_type":"M","path":"/tools","component":"Layout","query":null,"perms":null,"icon":"Tools","is_frame":false,"is_cache":false,"sort":3,"visible":true,"status":"1","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":4,"name":"用户管理","parent_id":1,"menu_type":"C","path":"/system/user","component":"system/user/index","query":null,"perms":"system:user:list","icon":"User","is_frame":false,"is_cache":false,"sort":1,"visible":true,"status":"1","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":5,"name":"角色管理","parent_id":1,"menu_type":"C","path":"/system/role","component":"system/role/index","query":null,"perms":"system:role:list","icon":"UserFilled","is_frame":false,"is_cache":false,"sort":2,"visible":true,"status":"1","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":6,"name":"菜单管理","parent_id":1,"menu_type":"C","path":"/system/menu","component":"system/menu/index","query":null,"perms":"system:menu:list","icon":"Grid","is_frame":false,"is_cache":false,"sort":3,"visible":true,"status":"1","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":7,"name":"部门管理","parent_id":1,"menu_type":"C","path":"/system/dept","component":"system/dept/index","query":null,"perms":"system:dept:list","icon":"Menu","is_frame":false,"is_cache":false,"sort":4,"visible":true,"status":"1","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":8,"name":"岗位管理","parent_id":1,"menu_type":"C","path":"/system/post","component":"system/post/index","query":null,"perms":"system:post:list","icon":"Management","is_frame":false,"is_cache":false,"sort":5,"visible":true,"status":"1","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":9,"name":"字典管理","parent_id":1,"menu_type":"C","path":"/system/dict","component":"system/dict/index","query":null,"perms":"system:dict:list","icon":"Collection","is_frame":false,"is_cache":false,"sort":6,"visible":true,"status":"1","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":10,"name":"参数设置","parent_id":1,"menu_type":"C","path":"/system/config","component":"system/config/index","query":null,"perms":"system:config:list","icon":"EditPen","is_frame":false,"is_cache":false,"sort":7,"visible":true,"status":"1","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":11,"name":"通知公告","parent_id":1,"menu_type":"C","path":"/system/notice","component":"system/notice/index","query":null,"perms":"system:notice:list","icon":"Bell","is_frame":false,"is_cache":false,"sort":8,"visible":true,"status":"1","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":12,"name":"操作日志","parent_id":1,"menu_type":"C","path":"/system/operlog","component":"system/operlog/index","query":null,"perms":"system:operlog:list","icon":"Document","is_frame":false,"is_cache":false,"sort":9,"visible":true,"status":"1","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":13,"name":"登录日志","parent_id":1,"menu_type":"C","path":"/system/logininfor","component":"system/logininfor/index","query":null,"perms":"system:logininfor:list","icon":"Notebook","is_frame":false,"is_cache":false,"sort":10,"visible":true,"status":"1","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":14,"name":"定时任务","parent_id":1,"menu_type":"C","path":"/system/job","component":"system/job/index","query":null,"perms":"system:job:list","icon":"Timer","is_frame":false,"is_cache":false,"sort":11,"visible":true,"status":"1","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":15,"name":"在线用户","parent_id":2,"menu_type":"C","path":"/monitor/online","component":"monitor/online/index","query":null,"perms":"monitor:online:list","icon":"Connection","is_frame":false,"is_cache":false,"sort":1,"visible":true,"status":"1","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":16,"name":"服务监控","parent_id":2,"menu_type":"C","path":"/monitor/server","component":"monitor/server/index","query":null,"perms":"monitor:server:list","icon":"DataAnalysis","is_frame":false,"is_cache":false,"sort":2,"visible":true,"status":"1","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":17,"name":"代码生成","parent_id":3,"menu_type":"C","path":"/tools/gen","component":"tools/gen/index","query":null,"perms":"tools:gen:list","icon":"MagicStick","is_frame":false,"is_cache":false,"sort":1,"visible":true,"status":"1","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":18,"name":"用户查询","parent_id":4,"menu_type":"F","path":null,"component":null,"query":null,"perms":"system:user:list","icon":null,"is_frame":false,"is_cache":false,"sort":1,"visible":true,"status":"1","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":19,"name":"用户新增","parent_id":4,"menu_type":"F","path":null,"component":null,"query":null,"perms":"system:user:add","icon":null,"is_frame":false,"is_cache":false,"sort":2,"visible":true,"status":"1","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":20,"name":"用户修改","parent_id":4,"menu_type":"F","path":null,"component":null,"query":null,"perms":"system:user:edit","icon":null,"is_frame":false,"is_cache":false,"sort":3,"visible":true,"status":"1","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":21,"name":"用户删除","parent_id":4,"menu_type":"F","path":null,"component":null,"query":null,"perms":"system:user:remove","icon":null,"is_frame":false,"is_cache":false,"sort":4,"visible":true,"status":"1","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":22,"name":"用户导出","parent_id":4,"menu_type":"F","path":null,"component":null,"query":null,"perms":"system:user:export","icon":null,"is_frame":false,"is_cache":false,"sort":5,"visible":true,"status":"1","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}

-- Table: sys_dept (6 rows)
-- {"id":1,"name":"RyFrame 科技","parent_id":null,"ancestors":"0","sort":1,"status":"1","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":2,"name":"研发部","parent_id":1,"ancestors":"0,1","sort":1,"status":"1","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":3,"name":"产品部","parent_id":1,"ancestors":"0,1","sort":2,"status":"1","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":4,"name":"运维部","parent_id":1,"ancestors":"0,1","sort":3,"status":"1","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":5,"name":"后端组","parent_id":2,"ancestors":"0,1,2","sort":1,"status":"1","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":6,"name":"前端组","parent_id":2,"ancestors":"0,1,2","sort":2,"status":"1","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}

-- Table: sys_post (4 rows)
-- {"id":1,"name":"董事长","code":"ceo","sort":1,"status":"1","remark":"公司最高管理者","del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":2,"name":"技术总监","code":"cto","sort":2,"status":"1","remark":"技术部门负责人","del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":3,"name":"项目经理","code":"pm","sort":3,"status":"1","remark":"项目经理","del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":4,"name":"普通员工","code":"user","sort":4,"status":"1","remark":"普通员工","del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}

-- Table: sys_dict_type (9 rows)
-- {"id":1,"name":"用户性别","code":"sys_user_sex","status":"1","remark":"用户性别列表","del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":2,"name":"菜单状态","code":"sys_show_hide","status":"1","remark":"菜单状态列表","del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":3,"name":"系统开关","code":"sys_normal_disable","status":"1","remark":"系统正常停用状态","del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":4,"name":"任务状态","code":"sys_job_status","status":"1","remark":"定时任务状态","del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":5,"name":"系统是否","code":"sys_yes_no","status":"1","remark":"系统是否列表","del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":6,"name":"通知类型","code":"sys_notice_type","status":"1","remark":"通知类型列表","del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":7,"name":"通知状态","code":"sys_notice_status","status":"1","remark":"通知状态列表","del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":8,"name":"操作类型","code":"sys_oper_type","status":"1","remark":"操作日志类型","del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":9,"name":"登录状态","code":"sys_common_status","status":"1","remark":"登录状态列表","del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}

-- Table: sys_dict_data (24 rows)
-- {"id":1,"type_code":"sys_user_sex","label":"男","value":"0","sort":1,"status":"1","css_class":"","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":2,"type_code":"sys_user_sex","label":"女","value":"1","sort":2,"status":"1","css_class":"","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":3,"type_code":"sys_user_sex","label":"未知","value":"2","sort":3,"status":"1","css_class":"","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":4,"type_code":"sys_show_hide","label":"显示","value":"1","sort":1,"status":"1","css_class":"primary","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":5,"type_code":"sys_show_hide","label":"隐藏","value":"0","sort":2,"status":"1","css_class":"danger","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":6,"type_code":"sys_normal_disable","label":"正常","value":"1","sort":1,"status":"1","css_class":"primary","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":7,"type_code":"sys_normal_disable","label":"停用","value":"0","sort":2,"status":"1","css_class":"danger","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":8,"type_code":"sys_job_status","label":"正常","value":"1","sort":1,"status":"1","css_class":"primary","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":9,"type_code":"sys_job_status","label":"暂停","value":"0","sort":2,"status":"1","css_class":"danger","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":10,"type_code":"sys_yes_no","label":"是","value":"Y","sort":1,"status":"1","css_class":"primary","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":11,"type_code":"sys_yes_no","label":"否","value":"N","sort":2,"status":"1","css_class":"danger","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":12,"type_code":"sys_notice_type","label":"通知","value":"1","sort":1,"status":"1","css_class":"primary","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":13,"type_code":"sys_notice_type","label":"公告","value":"2","sort":2,"status":"1","css_class":"success","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":14,"type_code":"sys_notice_status","label":"正常","value":"1","sort":1,"status":"1","css_class":"primary","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":15,"type_code":"sys_notice_status","label":"关闭","value":"0","sort":2,"status":"1","css_class":"danger","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":16,"type_code":"sys_oper_type","label":"其它","value":"0","sort":1,"status":"1","css_class":"","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":17,"type_code":"sys_oper_type","label":"新增","value":"1","sort":2,"status":"1","css_class":"primary","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":18,"type_code":"sys_oper_type","label":"修改","value":"2","sort":3,"status":"1","css_class":"primary","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":19,"type_code":"sys_oper_type","label":"删除","value":"3","sort":4,"status":"1","css_class":"danger","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":20,"type_code":"sys_oper_type","label":"授权","value":"4","sort":5,"status":"1","css_class":"primary","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":21,"type_code":"sys_oper_type","label":"导出","value":"5","sort":6,"status":"1","css_class":"warning","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":22,"type_code":"sys_oper_type","label":"导入","value":"6","sort":7,"status":"1","css_class":"warning","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":23,"type_code":"sys_common_status","label":"成功","value":"1","sort":1,"status":"1","css_class":"primary","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":24,"type_code":"sys_common_status","label":"失败","value":"0","sort":2,"status":"1","css_class":"danger","remark":null,"del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}

-- Table: sys_config (5 rows)
-- {"id":1,"name":"主框架页-默认皮肤样式","key":"sys.index.skinName","value":"skin-blue","remark":"蓝色 skin-blue、绿色 skin-green、紫色 skin-purple、红色 skin-red、黄色 skin-yellow","del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":2,"name":"用户管理-账号初始密码","key":"sys.user.initPassword","value":"123456","remark":"初始化密码","del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":3,"name":"主框架页-侧边栏主题","key":"sys.index.sideTheme","value":"theme-dark","remark":"dark主题theme-dark，light主题theme-light","del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":4,"name":"账号自助-验证码开关","key":"sys.account.captchaEnabled","value":"true","remark":"是否开启验证码功能（true开启，false关闭）","del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}
-- {"id":5,"name":"账号自助-是否开启注册","key":"sys.account.registerUser","value":"false","remark":"是否开启注册功能（true开启，false关闭）","del_flag":"0","created_at":"2026-06-03T17:26:20Z","updated_at":"2026-06-03T17:26:20Z"}

-- Table: sys_notice (0 rows)

-- Table: sys_job (4 rows)
-- {"id":45496945761849344,"name":"clean_oper_log","group_name":"system","cron_expr":"0 0 2 * * *","misfire_policy":"1","concurrent":"0","status":"1","remark":"每天 02:00 清理 30 天前的操作日志","create_time":"2026-06-03T17:35:17Z","update_time":"2026-06-03T17:35:17Z"}
-- {"id":45496945803792384,"name":"clean_login_info","group_name":"system","cron_expr":"0 0 3 * * *","misfire_policy":"1","concurrent":"0","status":"1","remark":"每天 03:00 清理 90 天前的登录日志","create_time":"2026-06-03T17:35:17Z","update_time":"2026-06-03T17:35:17Z"}
-- {"id":45496945833152512,"name":"clean_temp_files","group_name":"system","cron_expr":"0 0 4 * * 7","misfire_policy":"1","concurrent":"0","status":"1","remark":"每周日 04:00 清理 7 天前的临时文件","create_time":"2026-06-03T17:35:17Z","update_time":"2026-06-03T17:35:17Z"}
-- {"id":45496945862512640,"name":"database_backup","group_name":"system","cron_expr":"0 0 3 * * *","misfire_policy":"1","concurrent":"0","status":"1","remark":"每天 03:00 备份数据库，保留最近 7 天的备份文件","create_time":"2026-06-03T17:35:17Z","update_time":"2026-06-03T17:35:17Z"}

-- Total: 79 rows exported
