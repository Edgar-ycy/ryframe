# v0.5 发布与回滚指南

> 最后核对：2026-07-18

RyFrame 后端与 `ryframe-vue3` 位于独立仓库，但 v0.5 起使用相同的 SemVer 和 Git tag 协同发布。后端 API、前端生成类型和部署配置不提供跨版本兼容，禁止单独切换其中一端。

## 1. RC 准入

创建 stable tag 前必须先发布同版本候选，例如 `v0.5.0-rc.1`，并在与生产一致的 HTTPS、同站子域、Nginx、MySQL 8.4、Redis 和对象存储环境持续运行至少 48 小时。观察窗口至少包含：

- 登录成功率、CSRF 拒绝、refresh `409`/`401` 和重放撤销。
- Redis 错误、降级事件、readiness 失败和限流拒绝。
- 上传 `413`、对象存储错误、前端未捕获异常和动态路由恢复。
- 多标签刷新、页面重载恢复、access 过期登出和强制退出。

前后端在同一组提交上创建同名 RC 标签。先推送前端标签，再推送后端标签；后端工作流会执行与 stable 相同的后端、前端、迁移、恢复、smoke、覆盖率和 bundle 门禁，随后自动发布带完整制品的 prerelease。只有 prerelease 发布完成后，才能把该标签部署到 RC 环境并开始观察：

```bash
git tag -s v0.5.0-rc.1 -m "RyFrame v0.5.0-rc.1"
git push origin v0.5.0-rc.1
```

RC 部署必须登记为 GitHub Deployment。上线时写入 `in_progress`，至少连续运行 48 小时且观察项全部合格后才写入 `success`；`environment_url` 和 `log_url` 必须指向长期保留的监控面板与观察记录。示例（需要具有 deployments 写权限的 `GH_TOKEN`）：

```bash
deployment_id="$(gh api --method POST \
  "repos/OWNER/REPOSITORY/deployments" \
  -f ref=v0.5.0-rc.1 \
  -f environment=release-candidate \
  -f description='Deploy v0.5.0-rc.1 for continuous RC observation' \
  -F auto_merge=false \
  -F transient_environment=true \
  -F production_environment=false \
  --jq .id)"

gh api --method POST \
  "repos/OWNER/REPOSITORY/deployments/${deployment_id}/statuses" \
  -f state=in_progress \
  -f environment=release-candidate \
  -f description='RC observation started' \
  -f environment_url='https://rc.example.com' \
  -f log_url='https://monitoring.example.com/rc/v0.5.0-rc.1' \
  -F auto_inactive=false
```

观察期间一旦出现中断或未通过项，必须立即写入 `failure`、`error` 或 `inactive`，修复后从新的 `in_progress` 重新计算 48 小时。观察合格后由发布负责人写入终态：

```bash
gh api --method POST \
  "repos/OWNER/REPOSITORY/deployments/${deployment_id}/statuses" \
  -f state=success \
  -f environment=release-candidate \
  -f description='Continuous 48h RC observation approved' \
  -f environment_url='https://rc.example.com' \
  -f log_url='https://monitoring.example.com/rc/v0.5.0-rc.1' \
  -F auto_inactive=false
```

仓库必须预先创建 `stable-release` GitHub Environment，配置至少一名 required reviewer、启用 `Prevent self-review`，并关闭管理员绕过保护规则。stable 工作流会通过官方 Environment API 校验 required reviewer 和防止自审配置，并在全部自动门禁通过后停在该 Environment 等待人工审批；审批人、审批或管理员绕过动作及时间保存在 GitHub Actions/Deployments 审计记录中。管理员绕过开关本身不在 Environment REST 响应中，发布负责人必须在仓库设置中复核并以审计日志作为配置证据。仓库变量 `RYFRAME_PRODUCTION_API_BASE_URL` 必须设置为生产 API 的绝对 HTTPS `/api/v1` 地址，前端发布门禁会用它构建产物并拒绝相对地址。

stable 工作流会从 GitHub Releases 选择同版本最新发布的 prerelease，同时校验该 RC 的发布时间已超过 48 小时、同一 Deployment 的 `in_progress` 到 `success` 连续至少 48 小时、Deployment 精确绑定 RC 标签与提交，并要求 stable 的后端提交和前端提交都与该 RC 完全相同。仅有 `published_at` 不再构成观察证明。RC 后若修改任何代码，或观察过程被中断，必须发布/部署新的 RC 并重新开始 48 小时窗口。

## 2. 上线前准备

1. 确认后端和前端 `main` 均已通过各自 CI，版本、OpenAPI 和生成类型一致。
2. 使用 `deploy.sh backup` 备份 MySQL，并执行 `validate` 与 `rehearse` 临时库恢复演练。
3. 备份旧配置；以 `deploy/redis/redis.conf` 为基线确认生产 Redis 开启 AOF 持久化并使用 `noeviction`，同时配置部署环境专属的 TLS、网络边界和 ACL。
4. 验证 API 与管理端证书、同站子域、可信代理 CIDR、CORS Origin 和 Cookie Secure 属性。
5. 准备蓝绿或双 upstream，两端的新版本在未接流量时先通过 `/livez` 和 `/readyz`。

v0.4 会话没有 `sid`，切换后会主动失效，用户需要重新登录。上线公告必须明确这一点。

## 3. Stable tag

两个仓库必须在最新合格 RC 的原提交上创建相同的 stable 标签，不能包含 RC 观察期间之后的提交。先推送前端标签，确认远端可检出后再推送后端标签；后端标签会立即重新执行联合门禁，例如：

```bash
git tag -s v0.5.0 -m "RyFrame v0.5.0"
git push origin v0.5.0
```

后端 `.github/workflows/release.yml` 是最终发布门禁，依次完成：

1. 标签必须指向 `main` 已包含的提交，版本与全部 workspace crate、后端 OpenAPI、前端 package 和前端 OpenAPI 一致；stable 还必须与最新合格 RC 的前后端提交完全相同。
2. 检出前端同名标签并校验契约完全一致。
3. 在 Docker MySQL、AOF Redis 和 RustFS 上执行源码卫生、格式、Clippy、全量测试、迁移、Seeder、生成 schema 快照校验、应用 smoke、Redis 故障下的 `/livez=200`/`/readyz=503` 与恢复、对象存储、备份恢复以及依赖审计。
4. 执行前端 contract、类型检查、lint、单元测试、覆盖率、E2E 和 bundle budget。
5. stable 在所有自动门禁成功后进入受保护的 `stable-release` Environment，required reviewer 批准后才允许构建和发布；RC 无此人工推广步骤。
6. 所有门禁成功后才构建并发布制品；RC 标记为 prerelease，stable 标记为 latest。

前端门禁阈值固定为：session/auth/HTTP client 的 lines/functions/statements 不低于 90%、branches 不低于 80%；全部手写 TS/Vue 的前三项不低于 60%、branches 不低于 50%。生成文件和声明文件不计入覆盖率。首屏 gzip JS 不超过 350 KiB、CSS 不超过 100 KiB，单个异步原始 JS chunk 不超过 500 KiB。

## 4. 发布物

GitHub Release 必须同时包含：

- `ryframe-vX.Y.Z-linux-amd64.tar.gz`：后端可执行文件、`ryframe-db-reset`、`deploy.sh`、运维指南、静态配置模板、语言包、OpenAPI 和部署基线（含 Redis AOF/noeviction 配置）。
- `ryframe-vue3-vX.Y.Z-dist.tar.gz`：已验证的管理端静态文件。
- `ryframe-vX.Y.Z-linux-amd64.oci.tar`：带 provenance/SBOM attestation 的 OCI 归档。
- `ryframe-vX.Y.Z.cdx.json`：后端 Cargo 与前端 pnpm 依赖的 CycloneDX SBOM。
- `SHA256SUMS`：全部发布物的 SHA-256。

stable 发布还包含 `rc-observation-attestation.json`，保存工作流实际校验的 RC commit、Deployment 状态历史、`stable-release` 审批策略快照和本次 workflow run 链接。它用于事后复核；真正的人工审批记录仍以该链接中的 GitHub Actions Environment deployment 为准。

OCI 归档和注册表不可变标签由同一次 BuildKit 构建导出。RC 只推送 `ghcr.io/edgar-ycy/ryframe:X.Y.Z-rc.N`，绝不更新 `latest`；stable 门禁完成后才推送 `X.Y.Z` 并把同一镜像清单提升为 `latest`。部署前必须先验证 `sha256sum --check SHA256SUMS`，不得使用工作流之外重新构建的同名文件。

## 5. 切换与回滚

按后端和前端同一维护窗口完成蓝绿切换：先让新后端通过 readiness，再原子切换 API upstream 与 SPA 静态目录。切换后立即验证 CSRF、登录、静默恢复、权限、私有头像、普通上传和探针。

数据库迁移只允许加法式、保持旧应用可读；v0.5 Redis 使用独立命名空间，旧应用不会读取新会话键。需要回滚时：

1. 停止新流量，保留故障实例日志和指标。
2. 同时切回旧后端 upstream 和旧前端 dist。
3. 不删除 v0.5 Redis 命名空间，不执行破坏性数据库逆迁移。
4. 必要时从已演练的 MySQL 备份恢复到新库，再经变更审批切换。
5. 回滚后验证旧应用探针和核心业务，并通知用户重新登录。
