# 依赖维护与重复版本治理

依赖升级必须同时运行：

```bash
cargo tree --duplicates --locked
cargo deny check bans sources
cargo audit --deny warnings
```

`deny.toml` 将重复版本设为 `warn`，使 CI 和本地升级审查持续显示重复依赖，而不会因为暂时无法由本仓库控制的传递依赖阻塞安全更新。

## 当前治理原则

1. 工作区直接依赖统一在根 `Cargo.toml` 声明，不允许成员 crate 自行引入不同版本。
2. 同一主版本的重复依赖优先通过升级上游、收敛 feature 或移除不再使用的依赖解决。
3. 不用全局 patch 强行替换不兼容 API；现有 vendor patch 必须通过全工作区编译、测试和安全审计。
4. 每次依赖升级在 PR 中记录新增、消除和仍保留的重复版本族及原因。
5. 能够稳定统一的依赖族应加入 `deny.toml` 的显式禁用规则，逐步缩小重复范围。

重点关注密码学、TLS、压缩与异步运行时依赖，因为它们的重复版本同时影响安全修复覆盖面、二进制体积和编译时间。
