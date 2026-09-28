pub(crate) const BACKGROUND_JOB_ATTEMPT_DDL: &str = r#"CREATE TABLE IF NOT EXISTS `sys_background_job_attempt` (
    `job_id` BIGINT NOT NULL COMMENT '后台任务ID',
    `sequence` BIGINT NOT NULL COMMENT '该任务的单调领取序号',
    `available_at` DATETIME(6) NOT NULL COMMENT '本次领取的最早可执行时间',
    `started_at` DATETIME(6) NOT NULL COMMENT '领取事务记录的实际开始时间',
    `finished_at` DATETIME(6) DEFAULT NULL COMMENT '处理器完成或失败时间，租约失效时未知',
    `closed_at` DATETIME(6) DEFAULT NULL COMMENT '尝试闭合时间，包含租约回收时间',
    `outcome` VARCHAR(32) NOT NULL COMMENT 'running/succeeded/failed/dead/deferred/lease_expired',
    PRIMARY KEY (`job_id`, `sequence`),
    CONSTRAINT `fk_bg_attempt_job` FOREIGN KEY (`job_id`)
        REFERENCES `sys_background_job` (`id`) ON DELETE CASCADE ON UPDATE RESTRICT,
    CONSTRAINT `ck_bg_attempt_sequence` CHECK (`sequence` > 0),
    CONSTRAINT `ck_bg_attempt_times` CHECK (
        `started_at` >= `available_at`
        AND (`finished_at` IS NULL OR `finished_at` >= `started_at`)
        AND (`closed_at` IS NULL OR `closed_at` >= `started_at`)),
    CONSTRAINT `ck_bg_attempt_outcome` CHECK (
        (`outcome` = 'running' AND `finished_at` IS NULL AND `closed_at` IS NULL)
        OR (`outcome` = 'lease_expired' AND `finished_at` IS NULL AND `closed_at` IS NOT NULL)
        OR (`outcome` IN ('succeeded', 'failed', 'dead', 'deferred')
            AND `finished_at` IS NOT NULL AND `closed_at` IS NOT NULL
            AND `closed_at` = `finished_at`))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='后台任务单次执行事实'"#;
