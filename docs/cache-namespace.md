# 缓存命名空间一致性协议

RyFrame 的可失效业务缓存采用 CACHE-C 协议。数据库中的
`sys_cache_namespace_version` 是租户与命名空间版本的唯一事实来源；Redis 只是可以
随时丢弃并从数据库恢复的镜像。

## 持久化与事务边界

- 主键为 `(tenant_id, namespace)`，`version` 是从 `0` 开始的非负 `BIGINT`。
- 业务写入、版本递增和 Outbox 事件必须在同一个 MySQL 事务中提交。
- 版本通过数据库行锁和 `version = version + 1` 原子递增，不使用时间戳或 Snowflake，
  因而并发提交不会产生重复版本。
- Redis 未启用时仍递增数据库版本。以后重新启用 Redis 时，不需要读取旧缓存或猜测
  初始值。
- optional 模式关闭 Redis 时仍写入并消费 Outbox，消费动作是无副作用的成功；重新启用
  后由首次读取从数据库恢复镜像，避免积压永远无法投递的事件。
- Outbox 至少一次投递。重复或乱序事件由 Redis Lua 脚本幂等处理。

## Redis 键与原子操作

每个租户命名空间固定使用两个键：

```text
ryframe:tenant-cache:{tenant_id}:config:version
ryframe:tenant-cache:{tenant_id}:config:values
```

花括号中的租户标识是 Redis Cluster hash tag，因此 version String 与 values Hash 始终
位于同一个 hash slot。配置键是 `values` Hash 的 field，不再为每个配置项创建独立
Hash。

版本推进 Lua 遵循以下规则：

1. 只接受规范非负十进制字符串：`0`，或不带前导零的数字序列。
2. 比较时先比较字符串长度，再按字典序比较；禁止 `tonumber`，避免 Lua double 在
   `BIGINT` 超过 `2^53` 后丢失精度。
3. 仅当传入版本严格大于当前版本时，才写 version 并清空 values Hash。
4. 相同版本和更旧版本直接成功返回，不清 Hash，因此重复与乱序 Outbox 投递无副作用。
5. 写入某个 field 前必须原子确认当前 version 与查询使用的 version 完全相同；版本已
   推进时拒绝写入，避免把旧数据库结果写回新命名空间。

values Hash 可以设置 TTL，version key 不设置 TTL。若 version key 丢失而 values 仍在，
读取脚本只报告缺失；Service 从主库读取权威版本并执行版本推进脚本，该脚本会在恢复
version 的同时清除无法证明所属版本的旧 values。

## 参数配置读取

参数配置普通按键读取顺序如下：

1. 一次 Redis Lua 调用同时读取 version 与目标 field。
2. 热命中直接返回，不选择数据库节点，也不执行 SQL。
3. Hash 未命中时固定从主库 `Strong` 回源。副本延迟不能把旧配置重新写入当前版本。
4. version key 丢失时，先从主库读取 `sys_cache_namespace_version` 并恢复 Redis，再执行
   Strong 配置查询。
5. 回源结果仅在 version 未变化时写入 Hash；并发业务写已经推进版本时，本次缓存写被
   Lua 拒绝，但主库查询结果仍可用于当前响应。

认证前公开参数读取始终绕过缓存并使用 `Strong`。缓存故障在 required Redis 模式下
拒绝请求；optional 模式下记录降级并从主库读取。

## 运维检查

- 数据库不得缺少任一现有租户的 `config` 命名空间行；缺少时应用显式报错，不在 Redis
  中创建默认版本。
- 排查缓存时同时检查固定 version 与 values 键；禁止通过 `SCAN` 删除某个版本后缀键。
- 重放 Outbox 前无需排序，但不得手工构造负数、前导零或超出 MySQL `BIGINT` 的版本。
- 新增命名空间时必须同时补充迁移初始化、新租户初始化、业务事务递增和定向并发测试。
