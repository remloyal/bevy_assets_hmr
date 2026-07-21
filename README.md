# bevy_assets_hmr

Bevy 0.19 通用 HMR（Hot Module Replacement）框架 -- 用 diff + 事件订阅实现配置的**定向精准热重载**。

当配置文件变更时，不再是"整表重载 + 全局刷新"，而是：

1. **Diff**：对比新旧版本，只算出 `added / removed / modified` 三组 id
2. **Debounce**：批处理短时间内的多次写入（如原子写：删除 + 重命名）
3. **Dispatch**：派发 `ConfigRefresh<T>` 事件，携带 `changed_ids` 和 `target_entities`
4. **Subscribe**：业务系统按 id / 实体精准过滤，只刷新真正受影响的对象

## 为什么不用原生方案？

Bevy 的 `AssetServer` + `bevy/file_watcher` 提供了文件级热重载，但订阅方只能收到 `AssetEvent::Modified`——整表重载，你需要自己 diff、自己防抖、自己维护实体绑定。

**原生方案：手动监听 + 手动 diff（≈20 行样板代码，每种类型都要重复）**

```rust
// 需要自己保存旧版本做 diff
fn on_asset_event(
    mut events: EventReader<AssetEvent<NpcDatabase>>,
    assets: Res<Assets<NpcDatabase>>,
) {
    for evt in events.read() {
        let AssetEvent::Modified { id } = evt else { continue };
        let Some(new_db) = assets.get(*id) else { continue };
        // 自己比对新旧、算变更集、找实体、去重……
    }
}
```

**bevy_assets_hmr：一行注册，自动完成**

```rust
app.add_plugins(ConfigHmrPlugin::default());
app.register_config::<NpcDatabase>("data/npc.ron");
// ↑ 自动加载文件、自动 diff、自动防抖、自动派发 ConfigRefresh<T>
```

| 能力 | 原生方案 | bevy_assets_hmr |
|---|---|---|
| 文件监听 | ✅ 需启用 `bevy/file_watcher` | ✅ 共用 `bevy/file_watcher` |
| 变更检测 | `AssetEvent::Modified`（整表） | 自动 diff（added/removed/modified） |
| 防抖 | ❌ 自己实现 | ✅ 内置，默认 150ms |
| 实体绑定追踪 | ❌ 自己维护 `HashMap` | ✅ `ConfigBind<A>` / `AssetBind<A>` 自动追踪 |
| 按 id 精准刷新 | ❌ 自己写过滤逻辑 | ✅ `changed_ids` 直接筛选 |
| 多类型支持 | 每种类型手动注册 | ✅ 链式注册，类型独立 |

## 特性

- **两种接入模式**：
  - **包装模式** `register_config::<T>()`：框架用 `ConfigLoader` 加载 ron/json，包成 `ConfigAsset<T>`
  - **直接模式** `register_asset::<A>()`：用户用自己的 `AssetLoader`，Asset 本身就是 Config
- **一行接入**：`add_plugins(ConfigHmrPlugin)` + `register_config` / `register_asset`，自动加载文件 + 持有 handle
- **derive 宏**：`#[derive(ConfigDiff)]` + `#[config_diff(field, id)]`，一行实现 diff trait
- **自动追踪**：`ConfigBind<A>` 组件自动维护 handle↔entity 缓存
- **自动快照**：首次加载自动初始化快照，不派发"首次加载"事件
- **自动 diff + 派发**：文件变更后自动 diff + debounce + 派发 `ConfigRefresh<T>`
- **多类型并存**：一个 App 可注册多种类型，各自独立 diff、独立订阅
- **Bevy 0.19 适配**：`Message` / `MessageReader` / `MessageWriter`，`Asset` + `TypePath`

## 快速开始

### 1. 添加依赖

```toml
[dependencies]
bevy_assets_hmr = { git  = "https://github.com/remloyal/bevy_assets_hmr" }

[features]
# 启用 bevy 原生文件监听（修改 assets/*.ron 自动 reload）
hmr = ["bevy/file_watcher"]
```

> **💡 生产环境优化**：本库默认启用 HMR 运行时系统（`dev` feature）。
> 发布时通过 `--no-default-features` 可编译期禁用全部监听/diff/派发逻辑，
> 达到零运行时开销：
>
> ```toml
> [dependencies]
> bevy_assets_hmr = { git = "https://github.com/remloyal/bevy_assets_hmr", default-features = false }
> ```
>
> 此时资产仍正常加载，只是不再监听文件变更、不执行 diff 和派发事件。

### 2. 定义 Asset + 实现 ConfigDiff

#### 模式 A：包装模式（配置数据表）

适用于 ron/json 配置表，框架用 `ConfigLoader` 加载，包成 `ConfigAsset<T>`。

```rust
use bevy::asset::Asset;
use bevy::reflect::TypePath;
use bevy_assets_hmr::ConfigDiff;
use serde::{Deserialize, Serialize};

/// `#[derive(ConfigDiff)]` 自动实现基于 `npcs` 字段的 diff，
/// 以 `NpcEntry.id` 作为条目唯一标识。
///
/// # 约束
/// - `PartialEq`：diff 检测 modified 条目
/// - `Entry` 的 `Eq + Hash`：让 id 可入 HashSet
#[derive(
    Asset, TypePath, Serialize, Deserialize, Clone, Debug, PartialEq, Default, ConfigDiff,
)]
#[config_diff(field = "npcs", id = "id")]
pub struct NpcDatabase {
    pub npcs: Vec<NpcEntry>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
pub struct NpcEntry {
    pub id: String,
    pub name: String,
    pub hp: u32,
}
```

`#[config_diff]` 的选项：
- `field = "npcs"`：指定 `Vec<Entry>` 字段名（省略时自动找第一个 `Vec<_>` 字段）
- `id = "id"`：指定 entry 的 id 字段名（省略时默认 `"id"`）

对于非 Vec 模式（如单对象配置 `UiTheme`），手动实现 `ConfigDiff`：

```rust
use bevy_assets_hmr::SimpleConfigDiff;

impl SimpleConfigDiff for UiTheme {
    fn diff_id() -> &'static str { "theme" }
}
```

#### 模式 B：直接模式（自定义 Asset + 自定义 Loader）

适用于已有自定义 `AssetLoader` 的 Asset（如 bevy 内置的 `Image`/`Audio`，或自定义二进制格式）。

```rust
use bevy::asset::{Asset, AssetLoader, io::Reader, LoadContext};
use bevy::reflect::TypePath;
use bevy_assets_hmr::{HmrSource, SimpleConfigDiff};
use serde::{Deserialize, Serialize};

#[derive(Asset, TypePath, Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
struct LevelAsset {
    id: String,
    name: String,
    max_turns: u32,
}

// 1. 实现 ConfigDiff（和模式 A 一样）
impl SimpleConfigDiff for LevelAsset {
    fn diff_id() -> &'static str { "level" }
}

// 2. 实现 HmrSource：Asset 本身就是 Config，直接返回 self
impl HmrSource for LevelAsset {
    type Config = LevelAsset;
    fn config(&self) -> &Self::Config { self }
}

// 3. 用户自己的 Loader（和 bevy 官方 custom_asset.rs 一样）
struct LevelAssetLoader;
impl AssetLoader for LevelAssetLoader {
    type Asset = LevelAsset;
    type Settings = ();
    type Error = /* ... */;
    async fn load(/* ... */) -> /* ... */ { /* ... */ }
    fn extensions(&self) -> &[&str] { &["level"] }
}
```

### 3. 注册 HMR

#### 模式 A：包装模式

```rust
use bevy_assets_hmr::{ConfigHmrAppExt, ConfigHmrPlugin};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(ConfigHmrPlugin::default())
        .register_config::<NpcDatabase>("data/npc.ron")  // 一行注册：loader + 资源 + 系统 + 自动加载 + 持有 handle
        .add_systems(Update, on_npc_refresh)
        .run();
}
```

`register_config` 自动完成：
- 注册 `ConfigLoader<T>`（ron + json 双格式）
- 初始化 per-type resources（`HandleEntityCache` / `RefreshDebouncer` / `LastSnapshot`）
- 注册 `ConfigRefresh<T>` message + HMR 核心系统
- Startup 系统自动 `asset_server.load(path)` + 持有强引用 handle（防止资产被回收）

#### 模式 B：直接模式

```rust
use bevy_assets_hmr::{ConfigHmrAppExt, ConfigHmrPlugin};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(ConfigHmrPlugin::default())
        // 用户自己注册 Asset + Loader
        .init_asset::<LevelAsset>()
        .register_asset_loader(LevelAssetLoader)
        // 一行注册 HMR（不注册 ConfigLoader，不 init_asset，用户自己做）
        .register_asset::<LevelAsset>("levels/level_1.level")
        .add_systems(Update, on_level_refresh)
        .run();
}
```

`register_asset` 自动完成：
- 初始化 per-type resources + `ConfigRefresh<A::Config>` message + HMR 核心系统
- Startup 系统自动加载 + 持有 handle
- **不**注册 `ConfigLoader`（用户用自己的 loader）
- **不**调用 `init_asset`（用户自己调用）

### 4. 订阅 ConfigRefresh 做精准刷新

```rust
use bevy_assets_hmr::ConfigRefresh;
use bevy::ecs::message::MessageReader;

// 包装模式：ConfigRefresh<NpcDatabase>（Config 类型 = NpcDatabase）
fn on_npc_refresh(mut reader: MessageReader<ConfigRefresh<NpcDatabase>>) {
    for refresh in reader.read() {
        // refresh.new_config: NpcDatabase（直接是 Config，不需要 .raw）
        // refresh.changed_ids: HashSet<String>（added ∪ removed ∪ modified）
        // refresh.target_entities: Vec<Entity>（ConfigBind 绑定的实体）
        // refresh.diff_kind: DiffKind（Added/Removed/Modified/Mixed）
        println!("变更 id: {:?}", refresh.changed_ids);
        for id in &refresh.changed_ids {
            if let Some(npc) = refresh.new_config.npcs.iter().find(|n| &n.id == id) {
                println!("  [新增/修改] {} -> hp={}", id, npc.hp);
            } else {
                println!("  [删除] {}", id);
            }
        }
    }
}

// 直接模式：ConfigRefresh<LevelAsset>（Config 类型 = LevelAsset 本身）
fn on_level_refresh(mut reader: MessageReader<ConfigRefresh<LevelAsset>>) {
    for refresh in reader.read() {
        // refresh.new_config: LevelAsset（直接是 Asset 本身）
        println!("关卡变更: {} -> max_turns={}", refresh.new_config.name, refresh.new_config.max_turns);
    }
}
```

### 5. 实体绑定模式（可选）

如果实体直接绑定某个配置文件（如 UI 面板绑定主题），用 `ConfigBind<A>`：

```rust
use bevy_assets_hmr::ConfigBind;

fn spawn_ui(mut commands: Commands, server: Res<AssetServer>) {
    // 包装模式：load ConfigAsset<T>
    let handle = server.load::<bevy_assets_hmr::ConfigAsset<UiTheme>>("data/ui_theme.ron");
    commands.spawn((
        UiPanel,
        ConfigBind::new(handle),  // HMR 自动追踪，刷新时填充 target_entities
    ));
}
```

## API 概览

| 类型 | 说明 |
|---|---|
| `ConfigHmrPlugin` | 主插件，`add_plugins` 一次。可配置 `debounce_window`（默认 150ms） |
| `ConfigHmrAppExt` | App 扩展 trait，提供 `register_config` / `register_asset` / `insert_config` / `setup_hmr_headless` |
| `ConfigDiff` | trait：`fn diff(old, new) -> (added, removed, modified)`。支持 `#[derive(ConfigDiff)]` |
| `HmrSource` | trait：从 Asset 提取 Config（包装模式自动 impl，直接模式用户 impl） |
| `ConfigAsset<T>` | 包装 Asset：`{ raw: T, source_path: String }`（仅包装模式） |
| `ConfigRefresh<T>` | Message：`{ new_config: T, target_entities, changed_ids, diff_kind, source_path }` |
| `ConfigRemoved<T>` | Message：资产被删除时派发，携带 `target_entities` + `source_path` |
| `SimpleConfigDiff` | 简化 trait：只需 `PartialEq`，自动获得 `ConfigDiff`（单对象/枚举用） |
| `DiffKind` | `Added / Removed / Modified / Mixed` |
| `ConfigBind<A>` | Component：实体绑定到某个 handle |
| `ConfigHandle<A>` | Resource：持有强引用 handle 防止资产被回收 |
| `HandleEntityCache<A>` | Resource：handle ↔ entity 双向缓存，自动维护 |
| `LastSnapshot<A>` | Resource：每个 asset id 的上一版快照，自动初始化 |
| `RefreshDebouncer<A>` | Resource：批处理窗口（默认 150ms） |
| `AssetBind<A>` | Component：实体绑定到任意 `Asset`（无需 `ConfigDiff`） |
| `AssetBindCache<A>` | Resource：`AssetId → {Entity}` 缓存，同 `HandleEntityCache` 但约束 `Asset` |
| `AssetChanged<A>` | Message：任意 Asset 文件变更通知（无 diff，由 Bevy AssetServer 重载） |

### App 扩展方法（`ConfigHmrAppExt`）

| 方法 | 模式 | 说明 |
|---|---|---|
| `register_config::<T>(path)` | 包装 | 注册 ConfigLoader + 资源 + 系统 + 自动加载 + 持有 handle |
| `register_asset::<A>(path)` | 直接 | 只注册资源 + 系统 + 自动加载 + 持有 handle（用户自己注册 loader） |
| `insert_config::<T>(id, raw, path)` | 包装 | 直接注入数据（测试/headless 用） |
| `watch_asset::<A>(path)` | 通用 | 监听任意 Asset（Image/Scene/Audio 等），无需 `ConfigDiff` |
| `setup_hmr_headless()` | 通用 | 配置 headless 环境（无渲染世界时启用 Messages 缓冲交换） |

### 两种模式对比

| | 包装模式 `register_config` | 直接模式 `register_asset` |
|---|---|---|
| 适用场景 | ron/json 配置表 | 自定义 Asset + 自定义 Loader |
| Loader | 框架的 `ConfigLoader<T>`（ron + json） | 用户自己的 `AssetLoader` |
| Asset 类型 | `ConfigAsset<T>`（框架包装） | `A` 本身（用户定义） |
| Config 类型 | `T`（`ConfigAsset<T>::Config = T`） | `A`（`A::Config = A`） |
| `ConfigRefresh<T>` 的 T | `T`（如 `NpcDatabase`） | `A`（如 `LevelAsset`） |
| `new_config` 字段 | `T`（不需要 `.raw`） | `A`（直接是 Asset） |
| 需 impl | `ConfigDiff`（可用 derive 宏） | `ConfigDiff` + `HmrSource` |

### 两种刷新模式对比

| | 数据表模式（一对多） | 实体绑定模式（一对一） |
|---|---|---|
| 典型场景 | `npc.ron` 含所有 NPC，消费方从 Resource 读 | 一个 UI 面板对应一个 `layout.ron` |
| 是否用 `ConfigBind` | 否 | 是 |
| `target_entities` | 空 | 自动填充绑定的实体 |
| 订阅方过滤依据 | `changed_ids` ∩ 实体自身的 id 字段 | `target_entities` |

## 自动化行为

1. **自动加载文件**：`register_config` / `register_asset` 注册 Startup 系统自动 `asset_server.load(path)` + 持有强引用 handle
2. **自动追踪实体绑定**：`ConfigBind<A>` 被 `config_binding_registry_system` 自动扫描注册到 `HandleEntityCache<A>`
3. **自动初始化快照**：首次加载时自动记录初始版本，不派发"首次加载"事件
4. **自动 diff + 派发**：资产变更后自动 diff + debounce + 派发 `ConfigRefresh<T>`
5. **自动跳过空 diff**：diff 为空时不派发，订阅方不会收到无意义的刷新
6. **自动 debounce**：短时间内的多次写入合并为一次刷新（窗口可配置）
7. **自动缓存校验**：每 30 秒自动校验 `HandleEntityCache` 与实际组件的一致性

## 通用 Asset 文件监听（`watch_asset`）

除配置表的 diff-based HMR 外，本框架还支持**任意 Bevy Asset 类型**的轻量热更通知——图片、3D 模型、音频、字体等。不要求 `ConfigDiff`，只做变更通知 + 实体追踪。

Bevy 的 `AssetServer`（启用 `bevy/file_watcher` 时）已自动处理文件的磁盘监听、重载和 GPU 上传。`watch_asset` 在此基础上附加：

1. **实体绑定追踪**：`AssetBind<A>` 组件 + `AssetBindCache<A>` 缓存
2. **变更通知事件**：`AssetChanged<A>` Message（携带 `asset_id`、`new_asset`、`target_entities`）

### 使用示例

```rust
use bevy_assets_hmr::{ConfigHmrAppExt, ConfigHmrPlugin, AssetChanged, AssetBind};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins) // 启用 bevy/file_watcher
        .add_plugins(ConfigHmrPlugin::default())
        // 一行监听任意 Asset 类型
        .watch_asset::<Image>("textures/player.png")
        .watch_asset::<Scene>("models/character.gltf#Scene0")
        .watch_asset::<AudioSource>("audio/bgm.ogg")
        .add_systems(Update, on_image_changed)
        .run();
}

fn on_image_changed(mut reader: MessageReader<AssetChanged<Image>>) {
    for evt in reader.read() {
        println!("图片 {} 已更新，{} 个实体受影响",
            evt.asset_id, evt.target_entities.len());
        // evt.new_asset: Image（AssetServer 已重载 + GPU 上传完毕）
    }
}
```

### 对比 `register_config` / `register_asset`

| | `register_config` / `register_asset` | `watch_asset` |
|---|---|---|
| 适用类型 | 配置表（ron/json） | 任意 Asset（Image/Scene/Audio/Mesh/字体） |
| 要求 | `ConfigDiff`（或 `SimpleConfigDiff`） | 仅 `Asset + Clone + Send + Sync` |
| Diff | ✅ 条目级 diff（added/removed/modified） | ❌ 无 diff，整体变更通知 |
| Debounce | ✅ | ❌ 直接派发（AssetServer 已批处理） |
| 重载 | ConfigLoader / 用户自定义 Loader | Bevy 内置 Loader / 用户自定义 Loader |
| 通知事件 | `ConfigRefresh<T>` / `ConfigRemoved<T>` | `AssetChanged<A>` |
| 实体绑定 | `ConfigBind<A>` | `AssetBind<A>` |

## 示例

```bash
# 基础：定义 Asset -> derive ConfigDiff -> register_config -> 模拟变更 -> 订阅事件
cargo run --example basic -p bevy_assets_hmr

# 实体定向刷新：ConfigBind + HandleEntityCache 精准命中绑定的面板
cargo run --example config_bind -p bevy_assets_hmr

# 多类型并存：NpcDatabase + ItemDatabase 各自独立订阅
cargo run --example multi_type -p bevy_assets_hmr

# 直接模式：自定义 Asset + 自定义 Loader + register_asset
cargo run --example direct_mode -p bevy_assets_hmr
```

### `basic.rs` - 包装模式最简流程

```rust
#[derive(Asset, TypePath, Serialize, Deserialize, Clone, Debug, PartialEq, Default, ConfigDiff)]
#[config_diff(field = "npcs", id = "id")]
struct NpcDatabase { npcs: Vec<NpcEntry> }

app.add_plugins(ConfigHmrPlugin::default());
app.register_config::<NpcDatabase>("data/npc.ron");
// 订阅 MessageReader<ConfigRefresh<NpcDatabase>>
```

### `config_bind.rs` - 实体绑定模式

```rust
// 实体挂 ConfigBind::<ConfigAsset<UiTheme>>
commands.spawn((
    UiPanel { label: "panel_1".into() },
    ConfigBind::with_id(theme_id),
));
// 订阅方用 refresh.target_entities 精准刷新
```

### `multi_type.rs` - 多类型链式注册

```rust
app.register_config::<NpcDatabase>("data/npc.ron")
   .register_config::<ItemDatabase>("data/item.ron");
```

### `direct_mode.rs` - 直接模式（自定义 Asset + Loader）

```rust
impl HmrSource for LevelAsset {
    type Config = LevelAsset;
    fn config(&self) -> &Self { self }
}

app.init_asset::<LevelAsset>();
app.register_asset_loader(LevelAssetLoader);
app.register_asset::<LevelAsset>("levels/level_1.level");
// 订阅 MessageReader<ConfigRefresh<LevelAsset>>
```

## Bevy 0.19 兼容性

### `bevy/file_watcher` 文件监听

本 crate 不启用 `bevy/file_watcher`，由消费方按需启用：

```toml
[features]
hmr = ["bevy/file_watcher"]
```

启用后，修改 `assets/` 下的文件会自动 reload → 派发 `AssetEvent::Modified` → HMR 核心 diff → `ConfigRefresh<T>`。渲染资产（mesh/texture）走 bevy 原生自动更新，配置数据走 HMR 框架 diff + 定向刷新，两者共享同一个 file_watcher，不冲突。

### Headless 环境配置

无渲染世界（测试 / 示例）下需调用 `app.setup_hmr_headless()` 启用 Messages 缓冲交换。生产环境（有 `DefaultPlugins`）不需要。

### `Assets::insert` 的事件派发时机

`Assets::insert` 把事件写入 `queued_events`，由 `Assets::asset_events` 系统（在 `PostUpdate`）flush 到 `Messages<AssetEvent>`。同一帧内 `insert` 后立刻读 `MessageReader` 读不到（要等下一帧）。测试用 `insert_config` 直接初始化快照绕过这个延迟。

## License

MIT
