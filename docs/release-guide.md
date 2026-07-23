# v0.5 发布与回滚指南

> 最后核对：2026-07-22

RyFrame 后端与 `ryframe-vue3` 位于独立仓库，但 v0.5 起使用相同的 SemVer 和 Git tag 协同发布。后端 API、前端生成类型和部署配置不提供跨版本兼容，禁止单独切换其中一端。

每个 RC/stable tag 都必须是 annotated tag，且 annotation 必须与该仓库 `CHANGELOG.md` 中对应 stable 版本的完整章节一致；只写版本标题、手写摘要、使用 lightweight tag 或把说明藏在注释中都会被联合发布门禁拒绝。在前端和后端各自仓库执行下面的 Bash 命令，仅替换 `release_tag`；如需 GPG 签名，可把 `-a` 换成 `-s`，但必须保留 `--cleanup=verbatim` 和 `-F`：

```bash
release_tag=v0.5.0-rc.3
stable_tag="${release_tag%%-rc.*}"
notes_file="$(mktemp)"
trap 'rm -f "$notes_file"' EXIT

awk -v version="$stable_tag" '
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

同名标签始终先在前端仓库创建并推送，再在后端仓库执行。发布工作流会分别校验两仓 annotation、tag object ID 和 peeled commit，并在真正创建 Release 前再次核对远端标签未被移动。

## 1. RC 准入

创建 stable tag 前必须先发布同版本候选，例如 `v0.5.0-rc.1`，并在与生产一致的 HTTPS、同站子域、Nginx、MySQL 8.4、Redis 和对象存储环境持续运行至少 48 小时。观察窗口至少包含：

- 登录成功率、CSRF 拒绝、refresh `409`/`401` 和重放撤销。
- Redis 错误、降级事件、readiness 失败和限流拒绝。
- 上传 `413`、对象存储错误、前端未捕获异常和动态路由恢复。
- 多标签刷新、页面重载恢复、access 过期登出和强制退出。

前后端在同一组提交上按上述通用命令创建同名 RC 标签。先推送前端标签，再推送后端标签；后端工作流会执行与 stable 相同的后端、前端、迁移、恢复、smoke、覆盖率和 bundle 门禁，随后自动发布只包含 GitHub 标准源码快照的 prerelease。只有 prerelease 发布完成后，才能从该标签构建并部署到 RC 环境开始观察。

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

仓库必须预先创建 `stable-release` GitHub Environment，配置至少一名 required reviewer、启用 `Prevent self-review`，并关闭管理员绕过保护规则。stable 工作流会通过官方 Environment API 校验 required reviewer 和防止自审配置。默认发布路径仍会在全部自动门禁通过后停在该 Environment 等待人工审批；仅当仓库显式启用自动晋级、且 stable tag 的推送者与配置的专用发布账号完全一致时，才跳过人工审批 job。审批人、审批或管理员绕过动作及时间保存在 GitHub Actions/Deployments 审计记录中。管理员绕过开关本身不在 Environment REST 响应中，发布负责人必须在仓库设置中复核并以审计日志作为配置证据。源码 Release 不构建部署专用前端产物，因此不需要仓库级生产 API 地址；实际部署时由部署方注入 `VITE_APP_BASE_API`。

stable 工作流会从 GitHub Releases 选择同版本最新发布的 prerelease，同时校验该 RC 的发布时间已超过 48 小时、同一 Deployment 的 `in_progress` 到 `success` 连续至少 48 小时、Deployment 精确绑定 RC 标签与提交，并要求 stable 的后端提交和前端提交都与该 RC 完全相同。仅有 `published_at` 不再构成观察证明。RC 后若修改任何代码，或观察过程被中断，必须发布/部署新的 RC 并重新开始 48 小时窗口。

### 1.1 自动观察与晋级

`.github/workflows/auto-promote.yml` 每小时第 17、47 分钟检查一次最新已发布 RC，不使用长时间 `sleep`。功能默认关闭；启用前在后端仓库 Actions 配置中创建：

- Repository variable `RYFRAME_AUTO_PROMOTE_STABLE=true`。
- Repository variable `RYFRAME_AUTO_PROMOTE_RC_TAG=v0.5.0-rc.N`，只在该同名 RC 已经真实部署到下述公网地址后设置。
- Repository variable `RYFRAME_RC_ADMIN_URL=https://ryframe.ryfac.com`。
- Repository variable `RYFRAME_RC_API_URL=https://api.ryframe.ryfac.com`。
- Repository variable `RYFRAME_RELEASE_BOT`，值为专用发布账号的 GitHub login。
- Actions secret `RYFRAME_RELEASE_TOKEN`，值为该账号的 fine-grained PAT。

PAT 只授权 `ryframe` 和 `ryframe-vue3` 两个仓库，Repository permissions 设置为 Actions 读写、Deployments 读写和 Contents 读写；Deployments 和 Actions 写权限只在后端仓库实际使用。使用有有效期的专用账号凭据，不使用个人长期 classic PAT。工作流会通过 `/user` 校验 token 所属账号与 `RYFRAME_RELEASE_BOT` 完全一致，并且只在需要写 Deployment、重试 Release 工作流或推送 tag 的步骤中注入 secret。

被观察的部署必须从两仓对应 RC tag 的干净检出构建，并把完整提交 SHA 写入产物。后端构建时设置 `RYFRAME_BUILD_COMMIT`，前端构建时设置 `VITE_APP_BUILD_COMMIT`：

```bash
test -z "$(git status --porcelain)"
commit="$(git rev-parse HEAD)"
RYFRAME_BUILD_COMMIT="$commit" cargo build --release

# 使用仓库内的生产 Dockerfile 时也必须传入同一提交；缺失或无效 SHA 会直接使镜像构建失败。
docker build --file deploy/Dockerfile \
  --build-arg RYFRAME_BUILD_COMMIT="$commit" \
  --tag "ryframe:$commit" .
```

```bash
test -z "$(git status --porcelain)"
commit="$(git rev-parse HEAD)"
VITE_APP_BUILD_COMMIT="$commit" pnpm build
```

后端 `/api/v1/version` 会返回 `source_commit`，前端 dist 根目录会生成 `build-identity.json`。自动路径只处理 `RYFRAME_AUTO_PROMOTE_RC_TAG` 明确登记、且仍是最新已发布 prerelease 的候选版本；设置该变量就是部署方对“公网环境正在运行此 RC”的显式声明。工作流还要求它的前后端提交分别等于两仓当前 `main`，并在每次轮询时通过 HTTPS 检查管理端首页、前端构建 SHA、API `/livez`、`/readyz`、版本及后端构建 SHA。任一 SHA 与候选 tag 提交不一致或探针失败，都会把当前 Deployment 标为 `failure`。工作流还检查前一次定时运行，连续超过 2 小时没有成功心跳时废弃原观察窗口，从新的 `in_progress` 重新计算 48 小时。满足窗口后，它会：

1. 使用现有 `validate_release.py` 复核 RC 发布时间、Deployment、Environment、版本、OpenAPI、提交身份和两仓 CHANGELOG。
2. 从两仓 `vX.Y.Z` 完整 CHANGELOG 章节创建 annotated stable tag，不允许空说明、lightweight tag、移动或覆盖已有 tag。
3. 先推送前端 tag 并复核远端 tag object 与 peeled commit，再推送后端 tag。
4. 由后端 tag push 启动原有 `release.yml` 全量后端/前端门禁；全部通过后创建只包含 GitHub 标准源码快照的 stable Release。

若 RC 发布后任一仓库 `main` 发生变化，自动晋级会拒绝继续，必须发布并部署包含最新代码的同版本下一号 RC、更新 `RYFRAME_AUTO_PROMOTE_RC_TAG`，再重新观察 48 小时。若两仓 stable tag 已存在但 GitHub Release 尚未生成，自动工作流不会移动或重建标签，而是按同一标签调度或重跑 `release.yml`；失败最多尝试 3 次，每次仍执行完整发布门禁，超过上限后要求人工诊断。关闭 `RYFRAME_AUTO_PROMOTE_STABLE` 后，stable 仍走受保护 Environment 的人工审批路径。

## 2. 上线前准备

1. 确认后端和前端 `main` 均已通过各自 CI，版本、OpenAPI 和生成类型一致。
2. 使用 `deploy.sh backup` 备份 MySQL，并执行 `validate` 与 `rehearse` 临时库恢复演练。
3. 备份旧配置；以 `deploy/redis/redis.conf` 为基线确认生产 Redis 开启 AOF 持久化并使用 `noeviction`，同时配置部署环境专属的 TLS、网络边界和 ACL。
4. 验证 API 与管理端证书、同站子域、可信代理 CIDR、CORS Origin 和 Cookie Secure 属性。
5. 准备蓝绿或双 upstream，两端的新版本在未接流量时先通过 `/livez` 和 `/readyz`。

v0.4 会话没有 `sid`，切换后会主动失效，用户需要重新登录。上线公告必须明确这一点。

## 3. Stable tag

两个仓库必须在最新合格 RC 的原提交上按上述通用命令创建相同的 stable 标签（例如把 `release_tag` 改为 `v0.5.0`），不能包含 RC 观察期间之后的提交。先推送前端标签，确认远端可检出后再推送后端标签；后端标签会立即重新执行联合门禁。

后端 `.github/workflows/release.yml` 是最终发布门禁，依次完成：

1. 标签必须为 annotation 与对应 CHANGELOG 完整章节一致的 annotated tag，指向 `main` 已包含的提交；版本与全部 workspace crate、后端 OpenAPI、前端 package 和前端 OpenAPI 一致，stable 还必须与最新合格 RC 的前后端提交完全相同。
2. 检出前端同名标签并校验契约完全一致。
3. 在 Docker MySQL、AOF Redis 和 RustFS 上执行源码卫生、格式、Clippy、全量测试、迁移、Seeder、生成 schema 快照校验、应用 smoke、Redis 故障下的 `/livez=200`/`/readyz=503` 与恢复、对象存储、备份恢复以及依赖审计。
4. 执行前端 contract、类型检查、lint、单元测试、覆盖率、E2E 和 bundle budget。
5. stable 在所有自动门禁成功后进入受保护的 `stable-release` Environment，required reviewer 批准后才允许发布；仅显式启用且由匹配的专用发布账号触发的自动晋级可跳过该人工 job，RC 无此人工推广步骤。
6. 发布 job 按 validate 阶段记录的前后端 tag object ID 和提交 SHA 检出并再次复核远端标签，随后使用两仓 CHANGELOG 章节创建非空源码 Release；RC 标记为 prerelease，stable 标记为 latest。

前端门禁阈值固定为：session/auth/HTTP client 的 lines/functions/statements 不低于 90%、branches 不低于 80%；全部手写 TS/Vue 的前三项不低于 60%、branches 不低于 50%。生成文件和声明文件不计入覆盖率。首屏 gzip JS 不超过 350 KiB、CSS 不超过 100 KiB，单个异步原始 JS chunk 不超过 500 KiB。

## 4. 发布物

后端项目级 RC/stable Release 与前后端 Nightly Release 都只包含 GitHub 针对对应标签自动提供的两项源码快照：

- `Source code (zip)`。
- `Source code (tar.gz)`。

工作流不上传后端可执行文件、前端 dist、OCI 镜像、GHCR 标签、SBOM、校验和或 RC 观察证明附件。若同一标签重跑，发布步骤会先删除该目标标签 Release 上已有的全部自定义附件，再更新 Release；GitHub 自动源码快照不属于 Release assets API，不受清理影响。Nightly 只在对应仓库的 `main` CI 成功后更新。前端继续先推送同名 RC/stable tag 供后端联合门禁校验，但不会在后端门禁完成前独立创建 Release。工作流不会遍历其他标签，既有历史标签上的旧附件在本次发布策略迁移中一次性远程清理。

Deployment 状态、RC 观察过程和 stable Environment 审批继续保存在 GitHub Actions、Deployments 与审计日志中，不再复制为 Release 附件。部署方必须从已验证标签自行构建后端和前端，并在自己的交付链中生成所需的可执行文件、镜像、SBOM 与校验和。

## 5. 切换与回滚

按后端和前端同一维护窗口完成蓝绿切换：先让新后端通过 readiness，再原子切换 API upstream 与 SPA 静态目录。切换后立即验证 CSRF、登录、静默恢复、权限、私有头像、普通上传和探针。

数据库迁移只允许加法式、保持旧应用可读；v0.5 Redis 使用独立命名空间，旧应用不会读取新会话键。需要回滚时：

1. 停止新流量，保留故障实例日志和指标。
2. 同时切回旧后端 upstream 和旧前端 dist。
3. 不删除 v0.5 Redis 命名空间，不执行破坏性数据库逆迁移。
4. 必要时从已演练的 MySQL 备份恢复到新库，再经变更审批切换。
5. 回滚后验证旧应用探针和核心业务，并通知用户重新登录。
