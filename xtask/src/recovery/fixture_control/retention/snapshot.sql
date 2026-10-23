SELECT JSON_OBJECT('migration_id',CAST(m.id AS CHAR),'tenant_id',m.tenant_id,
      'state',m.state,'source',m.source_target_key,'target',m.target_key,
      'source_mode',m.source_target_mode,'target_mode',m.target_target_mode,
      'source_generation',CAST(m.source_generation AS CHAR),'target_generation',CAST(m.target_generation AS CHAR),
      'fingerprint',m.target_schema_fingerprint,'retention_hours',m.retention_hours,
      'retention_seconds',TIMESTAMPDIFF(SECOND,m.succeeded_at,m.retention_until),
      'remaining_seconds',TIMESTAMPDIFF(SECOND,UTC_TIMESTAMP(6),m.retention_until),
      'intent_clear',m.cancel_requested_at IS NULL AND m.finalize_requested_at IS NULL AND m.error_code IS NULL,
      'job_id',CAST(j.id AS CHAR),'job_status',j.status,'job_attempts',j.attempts,'job_tenant_id',j.tenant_id,
      'job_claim_sequence',CAST(j.claim_sequence AS CHAR),
      'attempt_records',COALESCE((SELECT JSON_ARRAYAGG(JSON_OBJECT('job_id',CAST(a.job_id AS CHAR),
          'sequence',CAST(a.sequence AS CHAR),'outcome',a.outcome,'times',JSON_OBJECT('available_at',DATE_FORMAT(a.available_at,'%Y-%m-%dT%H:%i:%s.%fZ'),'started_at',DATE_FORMAT(a.started_at,'%Y-%m-%dT%H:%i:%s.%fZ'),'finished_at',DATE_FORMAT(a.finished_at,'%Y-%m-%dT%H:%i:%s.%fZ'),'closed_at',DATE_FORMAT(a.closed_at,'%Y-%m-%dT%H:%i:%s.%fZ'))))
          FROM `{control}`.sys_background_job_attempt a WHERE a.job_id=j.id),JSON_ARRAY()),
      'job_migration_id',JSON_UNQUOTE(JSON_EXTRACT(j.payload,'$.migration_id')),
      'job_lease_clear',j.lease_owner IS NULL AND j.lease_until IS NULL,
      'item_id',CAST(i.id AS CHAR),'item_state',i.state,'item_source_count',i.source_row_count,
      'item_target_count',i.target_row_count,'source_digest',i.source_digest,'target_digest',i.target_digest,
      'cleanup_state',i.cleanup_state,'cleanup_count',i.cleanup_row_count,
      'placement_matches',p.current_target_key=m.target_key AND p.placement_generation=m.target_generation
          AND p.switch_token=m.switch_token AND p.state='active',
      'target_fence_matches',(SELECT COUNT(*) FROM `{target}`.biz_tenant_fence f WHERE f.tenant_id=m.tenant_id
          AND f.target_key=m.target_key AND f.placement_generation=m.target_generation AND f.switch_token=m.switch_token AND f.state='active'),
      'source_fence_matches',(SELECT COUNT(*) FROM `{source}`.biz_tenant_fence f WHERE f.tenant_id=m.tenant_id
          AND f.target_key=m.source_target_key AND f.placement_generation=m.source_generation AND f.switch_token=m.source_switch_token AND f.state='frozen'),
      'target_slot_matches',(SELECT COUNT(*) FROM `{target}`.biz_tenant_target_slot s WHERE s.slot_id=1 AND s.tenant_id=m.tenant_id
          AND s.placement_generation=m.target_generation AND s.switch_token=m.switch_token),
      'source_slot_matches',(SELECT COUNT(*) FROM `{source}`.biz_tenant_target_slot s WHERE s.slot_id=1 AND s.tenant_id=m.tenant_id
          AND s.placement_generation=m.source_generation AND s.switch_token=m.source_switch_token),
      'source_fences',(SELECT COUNT(*) FROM `{source}`.biz_tenant_fence WHERE tenant_id=m.tenant_id),
      'source_slots',(SELECT COUNT(*) FROM `{source}`.biz_tenant_target_slot WHERE tenant_id=m.tenant_id),
      'source_count',(SELECT COUNT(*) FROM `{source}`.biz_order WHERE tenant_id=m.tenant_id),
      'target_count',(SELECT COUNT(*) FROM `{target}`.biz_order WHERE tenant_id=m.tenant_id),
      'source_total',(SELECT COUNT(*) FROM `{source}`.biz_order),
      'target_total',(SELECT COUNT(*) FROM `{target}`.biz_order),
      'other_migrations',(SELECT COUNT(*) FROM `{control}`.sys_tenant_data_migration x WHERE x.tenant_id=m.tenant_id AND x.id<>m.id),
      'operation_leases',(SELECT COUNT(*) FROM `{control}`.sys_tenant_operation_lease WHERE tenant_id=m.tenant_id),
      'migration_times',JSON_OBJECT('prechecked_at',DATE_FORMAT(m.prechecked_at,'%Y-%m-%dT%H:%i:%s.%fZ'),'queued_at',DATE_FORMAT(m.queued_at,'%Y-%m-%dT%H:%i:%s.%fZ'),'quiesced_at',DATE_FORMAT(m.quiesced_at,'%Y-%m-%dT%H:%i:%s.%fZ'),'frozen_at',DATE_FORMAT(m.frozen_at,'%Y-%m-%dT%H:%i:%s.%fZ'),'copy_started_at',DATE_FORMAT(m.copy_started_at,'%Y-%m-%dT%H:%i:%s.%fZ'),'copy_completed_at',DATE_FORMAT(m.copy_completed_at,'%Y-%m-%dT%H:%i:%s.%fZ'),'verified_at',DATE_FORMAT(m.verified_at,'%Y-%m-%dT%H:%i:%s.%fZ'),'cut_over_at',DATE_FORMAT(m.cut_over_at,'%Y-%m-%dT%H:%i:%s.%fZ'),'activated_at',DATE_FORMAT(m.activated_at,'%Y-%m-%dT%H:%i:%s.%fZ'),'succeeded_at',DATE_FORMAT(m.succeeded_at,'%Y-%m-%dT%H:%i:%s.%fZ'),'retention_until',DATE_FORMAT(m.retention_until,'%Y-%m-%dT%H:%i:%s.%fZ'),'created_at',DATE_FORMAT(m.created_at,'%Y-%m-%dT%H:%i:%s.%fZ'),'updated_at',DATE_FORMAT(m.updated_at,'%Y-%m-%dT%H:%i:%s.%fZ')),'item_times',JSON_OBJECT('copy_started_at',DATE_FORMAT(i.copy_started_at,'%Y-%m-%dT%H:%i:%s.%fZ'),'copied_at',DATE_FORMAT(i.copied_at,'%Y-%m-%dT%H:%i:%s.%fZ'),'verified_at',DATE_FORMAT(i.verified_at,'%Y-%m-%dT%H:%i:%s.%fZ'),'created_at',DATE_FORMAT(i.created_at,'%Y-%m-%dT%H:%i:%s.%fZ'),'updated_at',DATE_FORMAT(i.updated_at,'%Y-%m-%dT%H:%i:%s.%fZ')),'job_times',JSON_OBJECT('available_at',DATE_FORMAT(j.available_at,'%Y-%m-%dT%H:%i:%s.%fZ'),'created_at',DATE_FORMAT(j.created_at,'%Y-%m-%dT%H:%i:%s.%fZ'),'updated_at',DATE_FORMAT(j.updated_at,'%Y-%m-%dT%H:%i:%s.%fZ'),'completed_at',DATE_FORMAT(j.completed_at,'%Y-%m-%dT%H:%i:%s.%fZ')))
      FROM `{control}`.sys_tenant_data_migration m
      JOIN `{control}`.sys_background_job j ON j.id=m.background_job_id AND j.job_type='tenant_data_migration'
      JOIN `{control}`.sys_tenant_data_migration_item i ON i.migration_id=m.id AND i.table_name='biz_order'
      JOIN `{control}`.sys_tenant_data_placement p ON p.tenant_id=m.tenant_id
      WHERE m.id={migration} AND m.tenant_id='{tenant}'
