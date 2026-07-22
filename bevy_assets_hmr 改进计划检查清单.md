# bevy_assets_hmr 改进计划检查清单

本文档用于跟踪 `bevy_assets_hmr` 从当前 0.1.0 实现走向可发布、可验证版本的工作。每项完成必须同时满足代码、测试和文档验收条件，不能只以“实现了 API”为完成标准。

## 0. 设计基线

- [ ] 明确插件定位为“配置资产条目级 Diff + 影响范围路由 + 按需视图刷新”。
- [ ] 明确不承诺 Rust 代码热替换、全 3D 资产编辑器、WASM 本地文件监听。
- [ ] 确定下一版是否允许破坏 `ConfigRefresh<T>` API；建议在 0.2.0 前完成事件模型调整。
- [ ] 记录 1k、10k、100k 条目配置的当前 Diff 基准，作为优化前基线。
- [ ] 定义页面活动状态来源：应用 State、页面栈或显式 `ActiveView`，不直接以 `Visibility` 作为唯一依据。

## 1. P0 正确性

### 1.1 真实加载失败

- [x] 监听 `AssetLoadFailedEvent<ConfigAsset<T>>` 和直接模式对应的失败事件。
- [x] `ConfigReloadFailed` 携带 `asset_id`、source path 和 Bevy 原始错误信息。
- [x] 确认失败 reload 时旧资产由 Bevy 保留，不重复向 `Assets` 插入造成伪 Modified。
- [x] 删除或重构当前“资产不存在即回滚”的模拟逻辑。
- [x] 增加真实 RON 语法错误端到端测试。
- [ ] 增加修复文件后能够再次成功 reload 的恢复测试。

### 1.2 防抖与事件不丢失

- [ ] 删除“flush 后 500ms 直接丢事件”的 cooldown 行为。
- [ ] 同一 AssetId 的短时间事件只合并到最新状态，不丢弃最终状态。
- [ ] 定义 Removed + Added/Modified 同窗口内的最终语义。
- [ ] 测试 500ms 内连续两次合法保存都能得到最终配置。
- [ ] 测试编辑器原子保存、重命名替换和临时文件流程。

### 1.3 事件模型

- [ ] 引入 `ConfigDelta { added, removed, modified }`，不再只暴露并集。
- [ ] `ConfigRefresh` 携带可可靠区分实例的 `asset_id`。
- [ ] 引入 `RefreshCause`，至少区分 Direct、Dependency、Manual、Recovery。
- [ ] 依赖级联事件携带真实 `triggered_by` 子资产，而不是用空 changed_ids 隐式表示。
- [ ] `ConfigRemoved` 携带被删除资产的 ID。
- [ ] 为事件 API 添加 doctest 和多资产同类型测试。

### 1.4 缓存一致性

- [ ] `cache_validation_system` 比较完整的 entity-to-handle 映射。
- [ ] 资产删除时清理 `flushed_at`、snapshot source path 和依赖边。
- [ ] 最后一个绑定移除或资产删除时清理 `AssetBindCache.path_registry`。
- [ ] 测试实体换 Handle、组件移除、despawn 和 AssetId 重用。
- [ ] 增加缓存长度稳定性测试，持续创建/删除资产后不得单调增长。

## 2. P1 性能与规模

### 2.1 O(n) Diff

- [ ] derive 和 `impl_config_diff!` 使用 ID 到条目引用的 HashMap 索引。
- [ ] added、removed、modified 的整体计算保持平均 O(n)。
- [ ] 检测旧配置和新配置中的重复 ID，并给出明确错误或校验失败。
- [ ] 覆盖 String、u32、Uuid ID。
- [ ] benchmark 证明 10k 条目不再呈平方增长。

### 2.2 Clone 与调度开销

- [ ] 统计一次刷新中完整配置 Clone 的次数和字节量。
- [ ] 评估事件只携带 AssetId、delta、revision，由消费者读取 `Assets`。
- [x] 正常刷新路径不再因回滚分支持有 `ResMut<Assets<A>>`。
- [ ] 验证 HMR 系统不会每帧阻塞普通 `Res<Assets<A>>` 读取系统的并行调度。
- [ ] 为大配置和高频保存增加基准或 tracing 指标。

### 2.3 同类型多文件

- [ ] `ConfigPathRegistry` 支持一种类型对应多个 path/AssetId。
- [ ] 强 Handle 持有资源支持多个实例，不再由后一次注册覆盖前一次。
- [ ] 相同类型重复注册不重复安装 HMR 系统和 loader。
- [ ] 测试两个同类型文件独立刷新、独立删除和独立失败。

### 2.4 条目级实体路由

- [ ] 确认项目是否存在“一张表绑定大量实体/页面”的真实需求。
- [ ] 若存在，设计 `(AssetId, EntryId) -> HashSet<Entity>` 索引。
- [ ] 提供 `ConfigEntryBind` 或等价 API，并维护反向 Entity 映射。
- [ ] changed ID 的实体集合查询保持平均 O(1)，总路由成本为 O(m + u)。
- [ ] 测试条目改 ID、实体换条目、despawn 和重复绑定。

## 3. 按需视图刷新

### 3.1 状态模型

- [ ] 为每个配置资产维护单调递增的 `AssetRevision`。
- [ ] 为派生视图记录 `AppliedRevision`，或使用可清除的 `DirtyView` 标记。
- [ ] 活动页面收到刷新后立即同步并更新 AppliedRevision。
- [ ] 非活动页面只记录失效，不执行 Text、Style、子实体等重建。
- [ ] 页面激活时从当前 Handle/Assets 读取最新配置，不依赖已经过期的 Message。

### 3.2 生命周期与边界

- [ ] 页面进入、重新显示或重新 spawn 时统一调用 `sync_active_view`。
- [ ] 页面隐藏期间连续多次热更，激活时只应用最终版本一次。
- [ ] 隐藏期间条目删除，激活时正确执行删除、占位或关闭页面策略。
- [ ] 隐藏期间资产加载失败，激活时仍显示最后有效版本并可读取失败状态。
- [ ] 共享业务 Resource 不因页面隐藏而跳过必要更新。
- [ ] 直接使用 Image/Mesh Handle 的组件不做无意义的重复重建。

### 3.3 验收测试

- [ ] 活动页面热更后在预期帧内更新。
- [ ] 隐藏页面热更时不执行昂贵的视图重建系统。
- [ ] 隐藏页面重新激活后立即显示最新配置。
- [ ] 三次隐藏期更新只触发一次激活补同步。
- [ ] 多页面共享同一资产时，仅活动页面立即更新。

## 4. 依赖与扩展能力

- [ ] 明确区分 Bevy loader dependency 与插件业务 Handle dependency。
- [ ] 如需多级业务级联，增加 visited 集合、深度限制和循环测试。
- [ ] 同一父资产被多个子资产同时触发时只派发一次，并保留完整原因集合。
- [ ] 为反序列化后的配置增加可选 Validator Hook。
- [ ] 验证依赖子项删除、恢复和加载失败的级联语义。
- [ ] 运行时开关和路径过滤仅在有明确使用场景后实现。

## 5. 工程与发布

- [ ] `bevy_assets_hmr_derive` 声明版本并纳入明确的 workspace/publish 流程。
- [ ] crates.io keywords 不超过 5 个，category 改为 game-development 等合适分类。
- [ ] `cargo package --allow-dirty` 成功。
- [ ] `cargo test --all-targets` 成功且无测试警告。
- [ ] `cargo test --doc` 成功。
- [ ] `cargo clippy --all-targets -- -D warnings` 成功。
- [ ] PowerShell 执行 `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --no-deps` 成功。
- [ ] `cargo check --no-default-features --all-targets` 成功。
- [ ] CI 覆盖 Windows、Linux、默认 feature 和 no-default-features。
- [ ] README 明确 Bevy 原生已有 debounce、失败事件和 loader 依赖重载，避免错误对比。
- [ ] README 给出复杂度边界：资产定位平均 O(1)，Diff 目标 O(n)，视图更新 O(u)。

## 6. 暂不实施

- [ ] Rust 结构体布局运行时热变更。
- [ ] 内置 Git 式历史版本栈和撤销/重做。
- [ ] 全 GLTF/材质/纹理编辑器级双缓冲系统。
- [ ] WASM 对本地开发目录的轮询监听。
- [ ] 与尚未稳定的 Bevy 编辑器协议深度绑定。
- [ ] 在核心 crate 内继续内置 YAML/TOML 等所有格式解析器。

这些项目保持未勾选表示“明确不在当前范围”，不计入版本完成率。
