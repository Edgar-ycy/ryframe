# RyFrame

RyFrame 是面向企业后台的 Rust 2024 服务端，与 RyFrame-Vue3 配套使用。它提供认证授权、系统管理、多租户、异步任务、筛选导出、对象存储和可观测性能力。

## 架构与开发效率优化结果

以下结果于 2026-09-02 在 Windows 本地环境完成验收。简单来说，前端首屏更小，后端保存后的等待时间更短，资源生成与门禁更快，同时补齐了主体切换时的数据隔离和架构约束。

| 关注点 | 优化前 | 最终结果 | 目标 | 结论 |
| --- | ---: | ---: | ---: | --- |
| 同租户用户切换 | 查询只按租户隔离 | 按租户、用户和会话代次隔离；旧请求与旧回调不会回写 | 不显示旧用户数据 | 通过 |
| 前端日常快速检查 P95 | 约 9.45 秒 | 9.506 秒 | 不超过 12 秒 | 通过 |
| 前端首屏 JavaScript gzip | 183,365 B | 162,102 B | 不超过 165,000 B | 通过 |
| 首屏 i18n gzip | 未拆分 | 8,657 B | 不超过 12 KiB | 通过 |
| 核心 API operation gzip | 单体生成文件 | 780 B | 不超过 8 KiB | 通过 |
| 资源生成器默认依赖数量 | 约 295 个 | 37 个 | 不超过 90 个 | 通过 |
| API 单目标保存构建 | 总是同时构建三个程序 | P50 缩短 34.5% | 至少缩短 30% | 通过 |
| Worker 单目标保存构建 | 总是同时构建三个程序 | P50 缩短 30.2% | 至少缩短 30% | 通过 |
| 仅修改配置 | 仍调用 Cargo | Cargo 调用 0 次，P50 缩短 94.0% | 至少缩短 70% | 通过 |
| 过期构建取消 P95 | 不支持 | 369.7 ms，无孤儿进程 | 不超过 1 秒 | 通过 |
| Worker 冷构建 P50 | 原始依赖面 | 缩短 30.6% | 至少缩短 20% | 通过 |
| migrate 冷构建 P50 | 包含多余运行时依赖 | 缩短 55.4% | 至少缩短 25% | 通过 |
| API 冷构建 P50 | 原始依赖面 | 缩短 18.0% | 至少缩短 10% | 通过 |
| 资源门禁 P95 | 约 107 秒 | 58.661 秒 | 不超过 60 秒 | 通过 |
| sccache 暖命中率 | 未固化 | 81.0%，缓存错误 0 | 至少 80% | 通过 |
| 后端日常 CI 业务任务 | 12 个，低频任务长期显示为跳过 | 6 个；低频深度检查移入扩展 CI | 减少无效排队与重复初始化 | 通过 |
| 前端日常 CI 业务任务 | 8 个，兼容与供应链任务独立排队 | 5 个；低频检查合并为 2 个扩展任务 | 保留门禁覆盖并减少任务数 | 通过 |

架构方面保持原有 12 个 crate，没有改变 HTTP、OpenAPI 或数据库业务契约。TLS 与 JWT 加密链路统一使用 AWS-LC，但这不等同于自动获得 FIPS 认证。正式验收还覆盖了完整 Rust/前端门禁、20 例资源门禁回放、真实 MySQL/Redis 协议测试以及本机 Chrome 登录、权限降低和无障碍检查。

日常提交只运行影响合并结果的检查；完整浏览器全栈、镜像扫描、物料清单和兼容性复核改为定时、发版或手动执行。检查内容没有被静默取消，只是不再为每次普通提交创建一排长期跳过的任务。

## 环境准备

本地开发使用 Windows，需要准备：

- Rust 1.97；
- MySQL；
- WSL 中的 Redis；
- 需要文件能力时启动 Windows RustFS；
- 前端所需的 Node.js 与 pnpm。

配置从 `config/` 中对应环境的文件加载，并可使用 `APP_` 环境变量覆盖。密码、令牌和证书请使用本机环境变量或密钥文件，不要写入配置样例。

## 启动开发环境

```powershell
$env:APP_ENV = "dev"
cargo migrate verify
cargo dev
```

`cargo migrate verify` 校验控制库结构，不修改数据库。需要更新本地数据库时运行 `cargo migrate up`；租户数据目标可使用 `cargo migrate verify tenant-data --all` 校验。

`cargo dev` 同时管理 API、Worker 和 Vite，并在后端修改后完成探活再切换版本。按 `Ctrl+C` 可停止整组进程。

只在排障时单独启动 Worker，并让 API 使用 external 任务模式：

```powershell
$env:APP_ENV = "dev"
$env:APP_JOBS_MODE = "external"
cargo run --locked -p ryframe --no-default-features --features bin-worker --bin ryframe-worker
```

## 开发与检查

日常修改后运行：

```powershell
cargo verify
```

准备联调或交付前运行完整检查：

```powershell
cargo verify --full
```

`--scope backend|frontend` 可限制主要检查侧。更多迁移、测试和排障命令见[开发指南](docs/development.md)。

## 同步 API 契约

`openapi/openapi.json` 是后端 HTTP 契约快照。接口变化后同步前端派生契约：

```powershell
cargo api-sync
```

完成同步后，在前端运行消费者检查并进行浏览器联调。请求格式、认证方式和稳定错误码见 [API 指南](docs/api.md)。

## 开发标准资源

标准 CRUD 资源通过资源清单离线生成。预览和检查都只读，只有显式的 `--write` 会更新生成结果：

```powershell
cargo resource post
cargo resource post --check
cargo resource --all --check
cargo resource post --write
cargo resource post --explain
```

资源清单位于 `catalog/resources/`。Post 和 Notice 可作为标准资源示例；导出、消息发布等特殊行为使用普通 Rust 用例扩展。完整流程见[开发指南](docs/development.md)。

## 文档

- [架构与扩展位置](docs/architecture.md)
- [开发指南](docs/development.md)
- [API 使用](docs/api.md)
- [数据与迁移](docs/data.md)
- [部署与排障](docs/operations.md)

字段、菜单、权限、配置默认值和生成结果分别以 OpenAPI、`catalog/access.toml`、配置结构、资源清单及命令 `--help` 为准。
