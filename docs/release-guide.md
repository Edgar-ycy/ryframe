# v0.5 发布与回滚指南

> 最后核对：2026-07-26

RyFrame 后端与 `ryframe-vue3` 位于独立仓库，但从 v0.5 起仅使用同名稳定版 SemVer tag 协同发布。后端 API、前端生成类型和部署配置不提供跨版本兼容，禁止单独切换其中一端。

## 1. 创建稳定版 tag

只接受 `vMAJOR.MINOR.PATCH` 格式的 annotated tag，例如 `v0.5.0`；不接受 RC、beta、nightly 或 lightweight tag。tag annotation 必须与该仓库 `CHANGELOG.md` 中对应版本的完整章节完全一致。前端和后端分别执行以下命令，仅替换 `release_tag`；如需 GPG 签名，可将 `-a` 改为 `-s`，但必须保留 `--cleanup=verbatim` 和 `-F`：

```bash
release_tag=v0.5.0
notes_file="$(mktemp)"
trap 'rm -f "$notes_file"' EXIT

awk -v version="$release_tag" '
  BEGIN { heading = "## [" version "]" }
  !found && ($0 == heading || index($0, heading " ") == 1 || index($0, heading "\t") == 1) {
    found=1
  }
  found && emitted && /^## \[/ { exit }
  found { print; emitted=1 }
  END { if (!found) exit 2 }
' CHANGELOG.md > "$notes_file"
grep --extended-regexp --quiet '^-[[:space:]]+[^[:space:]]' "$notes_file"
git tag -a --cleanup=verbatim -F "$notes_file" "$release_tag"
test "$(git cat-file -t "refs/tags/$release_tag")" = tag
git push origin "refs/tags/$release_tag"
```

先在前端仓库创建并推送 tag，确认远端可检出后，再在后端仓库创建同名 tag。后端工作流会校验两仓 annotation、tag object ID、peeled commit、版本与 OpenAPI，再在发布前复核远端 tag 未移动。

## 2. 上线前准备

1. 确认后端和前端 `main` 均已通过各自 CI，版本、OpenAPI 和生成类型一致。
2. 使用 `deploy.sh backup` 备份 MySQL，并执行 `validate` 与 `rehearse` 临时库恢复演练。
3. 备份旧配置；以 `deploy/redis/redis.conf` 为基线确认生产 Redis 开启 AOF 持久化并使用 `noeviction`，同时配置部署环境专属的 TLS、网络边界和 ACL。
4. 按[生产部署基线](production-deployment.md)核对 MySQL、Redis、对象存储 TLS、metrics allowlist 与 Token、API 文档关闭和持久存储；按[容量验收标准](capacity-guide.md)保留当前版本报告。
5. 加载 `deploy/prometheus/ryframe-alerts.yml`，逐条确认查询有数据并完成 Alertmanager 测试通知；值班人熟悉[生产监控与值班手册](operations-runbook.md)。
6. 验证 API 与管理端证书、同站子域、可信代理 CIDR、CORS Origin 和 Cookie Secure 属性。
7. 准备蓝绿或双 upstream，两端的新版本在未接流量时先通过 `/livez` 和 `/readyz`。

v0.4 会话没有 `sid`，切换后会主动失效，用户需要重新登录。上线公告必须明确这一点。

## 3. 联合发布门禁

后端 `.github/workflows/release.yml` 是唯一的联合发布主控。它依次完成：

1. 验证稳定 tag 位于 `main`，且后端与前端均为同名 annotated tag；版本与全部 workspace crate、后端 OpenAPI、前端 `package.json`、前端 OpenAPI 一致。
2. 根据仓库变量 `RYFRAME_FRONTEND_REPOSITORY` 检出前端；未设置时默认使用 `${owner}/ryframe-vue3`。工作流始终使用 tag 解析出的完整 40 位 commit，不读取浮动分支。
3. 将后端仓库、前端仓库、两仓 tag object、两仓 commit、版本和相同的 OpenAPI SHA-256 写入确定性的 `release-manifest.json`。同一对输入重复生成的文件字节必须完全一致。
4. 在 Docker MySQL、AOF Redis 和 RustFS 上执行源码卫生、格式、Clippy、全量测试、迁移、Seeder、生成 schema 快照校验、应用 smoke、Redis 故障恢复、对象存储、备份恢复以及依赖审计。
5. 执行前端 contract、类型检查、lint、单元测试、覆盖率、E2E 和 bundle budget。
6. 自动门禁全部通过后进入受保护的 `stable-release` Environment，required reviewer 批准后才允许发布。不得配置自动晋级绕过人工审批。
7. 发布 job 按 validate 阶段记录的 tag object ID 和 commit 再次复核远端 tag，然后创建 Release。

前端门禁阈值固定为：session/auth/HTTP client 的 lines/functions/statements 不低于 90%、branches 不低于 80%；全部手写 TS/Vue 的前三项不低于 60%、branches 不低于 50%。生成文件和声明文件不计入覆盖率。首屏 gzip JS 不超过 350 KiB、CSS 不超过 100 KiB，单个异步原始 JS chunk 不超过 500 KiB。

## 4. 发布物与可复现性

稳定版 Release 始终包含 GitHub 自动生成的两项源码快照：

- `Source code (zip)`。
- `Source code (tar.gz)`。

唯一允许的自定义附件是 `release-manifest.json`。它是前后端兼容关系的机器可读证据，包含 tag、版本、后端和前端的仓库/commit/tag object，以及共享 OpenAPI SHA-256。重跑同一 tag 时，工作流会先删除该 tag 上的旧附件，再重新生成并验证该清单；发布后 Release 必须恰好只有这一项自定义附件。

工作流不上传后端可执行文件、前端 dist、OCI 镜像、GHCR 标签、SBOM 或校验和。Nightly 仅在对应仓库的 `main` CI 成功后更新，并继续只保留 GitHub 自动源码快照。部署方必须以已验证的稳定 tag 和 `release-manifest.json` 为输入，在自己的交付链中生成可执行文件、镜像、SBOM 与校验和。

## 5. 切换与回滚

按后端和前端同一维护窗口完成蓝绿切换：先让新后端通过 readiness，再原子切换 API upstream 与 SPA 静态目录。切换后立即验证 CSRF、登录、静默恢复、权限、私有头像、普通上传和探针。

数据库迁移只允许加法式、保持旧应用可读；v0.5 Redis 使用独立命名空间，旧应用不会读取新会话键。需要回滚时：

1. 停止新流量，保留故障实例日志和指标。
2. 同时切回旧后端 upstream 和旧前端 dist。
3. 不删除 v0.5 Redis 命名空间，不执行破坏性数据库逆迁移。
4. 必要时从已演练的 MySQL 备份恢复到新库，再经变更审批切换。
5. 回滚后验证旧应用探针和核心业务，并通知用户重新登录。
