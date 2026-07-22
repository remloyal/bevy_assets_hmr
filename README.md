# bevy_assets_hmr

Bevy 0.19 通用 HMR（Hot Module Replacement）框架 -- 用 diff + 事件订阅实现配置的**定向精准热重载**。

当配置文件变更时，不再是"整表重载 + 全局刷新"，而是：

1. **Diff**：对比新旧版本，只算出 `added / removed / modified` 三组 id
2. **Debounce**：批处理短时间内的多次写入（如原子写：删除 + 重命名）
3. **Dispatch**：派发 `ConfigRefresh<T>` 事件，携带 `changed_ids` 和 `target_entities`
4. **Subscribe**：业务系统按 id / 实体精准过滤，只刷新真正受影响的对象

## 为什么不用原生方案？

Bevy 的 `AssetServer` + `bevy/file_watcher` 已提供文件监听、文件事件防抖、`AssetLoadFailedEvent`、Handle 原位替换和 loader dependency 重载。本插件不重复宣称这些能力；它处理资产替换后的业务层问题：配置表内部哪些 ID 变化、哪些实体受影响、依赖刷新由谁触发，以及隐藏页面何时补同步。插件额外提供的 150ms 窗口只用于按 AssetId 合并 HMR 派发，不代替 Bevy 的文件监听防抖。

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
| 文件事件防抖 | ✅ Bevy file watcher 已提供 | ✅ 复用原生能力，并按 AssetId 合并业务派发（默认 150ms） |
| 加载失败 | ✅ `AssetLoadFailedEvent` | ✅ 转发为携带旧快照和绑定实体的 `ConfigReloadFailed` |
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
/// - `id` 字段的 `Eq + Hash + Clone`：用于建立条目索引
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

derive 和 `impl_config_diff!` 都会先建立 `ID -> &Entry` HashMap，added、removed、modified 的整体计算平均为 O(n)。同一版本中出现重复 ID 时会记录 error，并把重复 ID 归入 modified，避免静默选择任意条目。

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
use bevy_assets_hmr::{ConfigRefresh, RefreshCause};
use bevy::ecs::message::MessageReader;

// 包装模式：ConfigRefresh<NpcDatabase>（Config 类型 = NpcDatabase）
fn on_npc_refresh(mut reader: MessageReader<ConfigRefresh<NpcDatabase>>) {
    for refresh in reader.read() {
        // refresh.new_config: NpcDatabase（直接是 Config，不需要 .raw）
        // refresh.asset_id: UntypedAssetId（区分同类型的多个配置文件）
        // refresh.delta: ConfigDelta（分别包含 added / removed / modified）
        // refresh.changed_ids: HashSet<String>（added ∪ removed ∪ modified）
        // refresh.target_entities: Vec<Entity>（ConfigBind 绑定的实体）
        // refresh.diff_kind: DiffKind（Added/Removed/Modified/Mixed）
        // refresh.cause: RefreshCause（Direct/Dependency/Manual/Recovery）
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

### 6. 只同步活动页面（可选）

配置资产本身已经由 Bevy 替换，隐藏页面不需要立即重建 Text、Style 或子实体。页面实体同时挂载 `ConfigBind<A>` 和 `AppliedRevision<A>`；显示页面时插入 `ActiveConfigView`，隐藏时移除该 marker：

```rust
use bevy_assets_hmr::{ActiveConfigView, AppliedRevision, ConfigBind};

let handle = server.load::<bevy_assets_hmr::ConfigAsset<UiTheme>>("data/ui_theme.ron");
commands.spawn((
    UiPanel,
    ConfigBind::new(handle.clone()),
    AppliedRevision::new(handle),
    ActiveConfigView,
));
```

活动页面在配置变化、删除或加载失败时收到轻量 `ConfigViewSync<A>`；订阅方按 `asset_id` 从当前 `Assets<A>` 读取数据后重建视图。隐藏页面只保留旧 `AppliedRevision`，期间无论更新多少次，重新插入 `ActiveConfigView` 时都只收到一次最终 revision。`AssetRevisionStatus` 可区分 Available、Removed 和 LoadFailed；失败时 Bevy 保留的上一有效资产仍可读取。

这个机制只跳过派生视图重建，不会阻止全局 `ConfigRefresh<T>`，因此共享业务 Resource 仍应正常更新。直接持有 Image/Mesh Handle 的渲染组件由 Bevy 原生资产替换生效，通常无需使用这套视图重建机制。

复杂度边界：AssetId 的缓存定位平均 O(1)，条目 Diff 平均 O(n)。路由需要扫描该资产的 m 个绑定实体并只为其中 u 个活动视图派发同步，因此路由加重建为 O(m + u)，不是严格 O(1)；隐藏页面只承担一次轻量状态检查，不进入昂贵的视图重建路径。

## 兼容 `bevy_asset_loader`

如果你已经在用 [`bevy_asset_loader`](https://crates.io/crates/bevy_asset_loader)
的 `AssetCollection` + `LoadingState` 流程，在集合上增加
`#[derive(HmrAutoWatch)]`，再安装宏生成的 `hmr_plugin(state)`，就能在加载
完成后接入 HMR 框架，无需手写 `register_config` / `register_asset`。

### 用法

```rust
use bevy::prelude::*;
use bevy_asset_loader::prelude::*;
use bevy_assets_hmr::{
    ConfigAsset, ConfigHmrPlugin, ConfigRefresh, HmrAutoWatch, SimpleConfigDiff,
};

#[derive(States, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
enum GameState {
    #[default]
    Loading,
    Ready,
}

#[derive(Asset, TypePath, Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
struct MyConfig { title: String, max_players: u32 }
impl SimpleConfigDiff for MyConfig {}

// AssetCollection + HmrAutoWatch 双 derive
#[derive(AssetCollection, Resource, HmrAutoWatch)]
struct GameAssets {
    #[asset(path = "data/config.ron")]
    cfg: Handle<ConfigAsset<MyConfig>>,  // 自动接入 HMR
}

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(ConfigHmrPlugin::default())
        .init_state::<GameState>()
        .add_loading_state(
            LoadingState::new(GameState::Loading)
                .continue_to_state(GameState::Ready)
                .load_collection::<GameAssets>(),
        )
        // 一行接入 HMR：进入 Ready 状态时接管所有 Handle<A: HmrSource> 字段
        .add_plugins(GameAssets::hmr_plugin(GameState::Ready))
        .add_systems(Update, my_subscriber.run_if(in_state(GameState::Ready)))
        .run();
}
```

`ConfigAsset<T>` 是推荐的包装模式：框架提供 loader 和 `HmrSource` 实现，
所以不需要字段标记。已有自定义 Asset/loader 时可以使用直接模式：

```rust
#[derive(Asset, TypePath, Clone, PartialEq)]
pub struct SkillConfig { /* ... */ }

// SkillConfig 还需要实现 ConfigDiff 和 HmrSource，应用需要注册自己的 AssetLoader。

#[derive(AssetCollection, Resource, HmrAutoWatch)]
pub struct GameAssets {
    #[asset(path = "configs/skill.ron")]
    #[hmr_watch]
    pub skill_config: Handle<SkillConfig>,

    // 普通 Bevy 资产不标记，会被 HmrAutoWatch 跳过。
    #[asset(path = "textures/icon.png")]
    pub icon: Handle<Image>,
}
```

`#[hmr_watch]` 是 `HmrAutoWatch` derive 的 helper 属性，不能脱离
`#[derive(HmrAutoWatch)]` 单独使用。

### 规则

- **包装模式自动接入**：带静态 `#[asset(path = "...")]` 的
  `Handle<ConfigAsset<T>>` 默认接入 HMR。
- **直接模式显式接入**：`Handle<A: HmrSource>` 必须增加 `#[hmr_watch]`；
  `A` 的 Asset 和 loader 仍由应用注册。
- **跳过字段**：用 `#[hmr(skip)]` 跳过原本会自动接入的 `ConfigAsset<T>`。
- **普通资产**：未标记的 `Handle<Image>` 等普通 Bevy 资产自动跳过。
- **静态路径**：HMR 字段必须使用 `#[asset(path = "...")]`；dynamic asset
  key 目前不支持，因为宏无法在构建插件时确定源路径。
- **状态选择**：`hmr_plugin(state)` 的 `state` 应为 `LoadingState` 的
  `continue_to_state` 目标状态（如 `GameState::Ready`），表示资产已加载完毕。
- **无额外依赖**：`HmrAutoWatch` 宏和 `HmrAutoWatchPlugin` 只依赖 bevy 的
  `States`，不依赖 `bevy_asset_loader` crate 本身--用户需自行添加
  `bevy_asset_loader` 依赖并 `#[derive(AssetCollection)]`。

## API 概览

| 类型 | 说明 |
|---|---|
| `ConfigHmrPlugin` | 主插件，`add_plugins` 一次。可配置 `debounce_window`（默认 150ms） |
| `ConfigHmrAppExt` | App 扩展 trait，提供 `register_config` / `register_asset` / `insert_config` / `insert_asset` / `setup_hmr_headless` |
| `ConfigDiff` | trait：`fn diff(old, new) -> (added, removed, modified)`。支持 `#[derive(ConfigDiff)]`（含 `id_type` 参数支持 `u32`/`Uuid` 等非 String 主键） |
| `HmrSource` | trait：从 Asset 提取 Config（包装模式自动 impl，直接模式用户 impl） |
| `ConfigAsset<T>` | 包装 Asset：`{ raw: T, source_path: String }`（仅包装模式） |
| `ConfigRefresh<T>` | Message：携带 `asset_id`、结构化 `delta`、兼容用 `changed_ids`、`cause`、当前配置与目标实体 |
| `ConfigRemoved<T>` | Message：资产被删除时派发，携带 `asset_id`、`target_entities` 与 `source_path` |
| `ConfigDelta<Id>` | 分别保存 added、removed、modified ID 集合 |
| `RefreshCause` | `Direct / Dependency { triggered_by } / Manual / Recovery` |
| `SimpleConfigDiff` | 简化 trait：只需 `PartialEq`，自动获得 `ConfigDiff`（单对象/枚举用） |
| `DiffKind` | `Added / Removed / Modified / Mixed` |
| `ConfigBind<A>` | Component：实体绑定到某个 handle |
| `ConfigHandle<A>` | Resource：持有同类型所有已注册文件的强引用 handles，防止资产被回收 |
| `HandleEntityCache<A>` | Resource：handle ↔ entity 双向缓存，自动维护 |
| `LastSnapshot<A>` | Resource：每个 asset id 的上一版快照，自动初始化 |
| `AssetRevision<A>` | Resource：每个 AssetId 的单调 revision 与 Available/Removed/LoadFailed 状态 |
| `AppliedRevision<A>` | Component：派生视图已应用的 revision |
| `ActiveConfigView` | Component marker：存在时立即路由视图同步，移除时只累计 revision |
| `ConfigViewSync<A>` | Message：活动视图从当前 `Assets<A>` 应用指定最终 revision 的请求 |
| `HmrAutoWatch` | trait + derive：自动接入 `Handle<ConfigAsset<T>>`，并通过 `#[hmr_watch]` 接入直接模式的 `Handle<A: HmrSource>` |
| `HmrAutoWatchPlugin` | 由 `HmrAutoWatch` derive 生成的 `hmr_plugin(state)` 返回的 Plugin |
| `take_over_handle` | 手动接管已加载的 handle 接入 HMR（宏内部使用） |
| `RefreshDebouncer<A>` | Resource：批处理窗口（默认 150ms） |
| `AssetBind<A>` | Component：实体绑定到任意 `Asset`（无需 `ConfigDiff`） |
| `AssetBindCache<A>` | Resource：`AssetId → {Entity}` 缓存，同 `HandleEntityCache` 但约束 `Asset` |
| `AssetChanged<A>` | Message：任意 Asset 文件变更通知（无 diff，由 Bevy AssetServer 重载）。`source_path` 从 `AssetBind` 的 handle path 自动填充 |
| `AssetRemoved<A>` | Message：任意 Asset 被删除时派发，携带 `asset_id` + `target_entities` + `source_path` |

### App 扩展方法（`ConfigHmrAppExt`）

| 方法 | 模式 | 说明 |
|---|---|---|
| `register_config::<T>(path)` | 包装 | 注册 ConfigLoader + 资源 + 系统 + 自动加载 + 持有 handle；同类型可注册多个 path |
| `register_asset::<A>(path)` | 直接 | 只注册资源 + 系统 + 自动加载 + 持有 handle；同类型可注册多个 path（用户自己注册 loader） |
| `insert_config::<T>(id, raw, path)` | 包装 | 直接注入数据（测试/headless 用） |
| `insert_asset::<A>(id, asset, path)` | 直接 | 直接注入数据 + 初始化快照（测试/headless 用，消除手动样板） |
| `watch_asset::<A>()` | 通用 | 注册实体追踪 + 变更通知，无 `ConfigDiff` 要求（用户自己 load） |
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
8. **自动依赖图 + 级联**：构建跨类型 `Handle<*>` 依赖图，子资产变更自动派发 `ConfigRefresh` 给父资产订阅方，并在 `RefreshCause::Dependency` 中保留触发源

## 依赖链级联（`DependencyGraph`）

当一个父配置通过 `Handle<*>` 字段持有子配置的引用时，修改子配置会自动让父配置订阅方收到一个 `ConfigRefresh<T>`——这让"父配置缓存子配置派生数据"成为可能。

### 工作机制

1. **`#[derive(Asset)]`** 自动生成 `VisitAssetDependencies` impl，遍历所有标有 `#[dependency]` 的 `Handle<*>` 字段。
2. **`dependency_registry_system<A>`**（注册在 `flush_debounced_refresh` 之后）每次 `AssetEvent<A>::Added/Modified` 时重建该资产在 `DependencyGraph` 中的边。
3. **`flush_debounced_refresh<A>`**（修改后）在派发 `ConfigRefresh` 后，查询图找出此资产的所有父资产，推入 `CascadeQueue.pending`。
4. **`cascade_dispatch_system<A>`**（每类型，链末尾）排空 `CascadeQueue` 中匹配当前类型的条目，为每个父资产派发一次 `ConfigRefresh<A::Config>`，并合并本帧所有 `triggered_by` 子资产。

### 订阅方语义

```rust
fn on_parent_refreshed(mut reader: MessageReader<ConfigRefresh<ParentDb>>) {
    for refresh in reader.read() {
        match &refresh.cause {
            RefreshCause::Dependency { triggered_by } => {
                rederive_state_from_children(&refresh.new_config, triggered_by);
            }
            RefreshCause::Direct => {
                apply_diff(&refresh.delta, &refresh.new_config);
            }
            _ => {}
        }
    }
}
```

### 使用示例

```rust
use bevy::prelude::*;
use bevy_assets_hmr::{ConfigHmrAppExt, ConfigHmrPlugin, ConfigDiff, HmrSource};
use bevy::asset::Asset;
use bevy::reflect::TypePath;
use serde::{Deserialize, Serialize};

#[derive(Asset, TypePath, Serialize, Deserialize, Clone, PartialEq, Default, ConfigDiff)]
#[config_diff(field = "entries", id = "id")]
struct ItemDb {
    entries: Vec<Item>,
}

#[derive(Asset, TypePath, Serialize, Deserialize, Clone, PartialEq, Default, ConfigDiff)]
#[config_diff(field = "entries", id = "id")]
struct NpcDatabase {
    entries: Vec<NpcEntry>,
    /// 这个 Handle 字段让 NpcDatabase 自动依赖 ItemDb。
    /// 用 `#[dependency]` 标记后，修改 item.ron 会让 NpcDatabase
    /// 订阅方收到 `changed_ids` 为空的 ConfigRefresh。
    #[dependency]
    item_table: Handle<ItemDb>,
}

app.add_plugins(ConfigHmrPlugin::default())
    .register_config::<ItemDb>("data/items.ron")
    .register_config::<NpcDatabase>("data/npcs.ron");
```

### API 概览

| 符号 | 说明 |
|---|---|
| `DependencyGraph` | Resource：`UntypedAssetId -> Vec<(parent_untyped, parent TypeId)>` |
| `CascadeQueue` | Resource：包含 parent、类型与 `triggered_by` 的待级联请求 |
| `dependency_registry_system<A>` | 系统：从 `AssetEvent<A>::Added/Modified` 重建依赖边 |
| `dependency_cleanup_system<A>` | 系统：从 `AssetEvent<A>::Removed` 清理边 |
| `cascade_dispatch_system<A>` | 系统：派发级联 `ConfigRefresh<A::Config>`（下帧执行） |

## 通用 Asset 文件监听（`watch_asset`）

除配置表的 diff-based HMR 外，本框架还支持**任意 Bevy Asset 类型**的轻量热更通知——图片、3D 模型、音频、字体等。不要求 `ConfigDiff`，只做变更通知 + 实体追踪。

Bevy 的 `AssetServer`（启用 `bevy/file_watcher` 时）已自动处理文件的磁盘监听、重载和 GPU 上传。`watch_asset` 在此基础上附加：

1. **实体绑定追踪**：`AssetBind<A>` 组件 + `AssetBindCache<A>` 缓存
2. **变更通知事件**：`AssetChanged<A>` Message（携带 `asset_id`、`target_entities`、`source_path`）
3. **删除通知事件**：`AssetRemoved<A>` Message（资产被删除时派发，携带 `asset_id`、`target_entities`、`source_path`）

> `source_path` 从 `AssetBind<A>` 注册时记录的 `Handle::path()` 自动填充。若 handle 无路径（如直接 `Assets::insert`），则为空字符串。
>
> `AssetChanged<A>` 不复制资产值。图片等大型资产包含解码后的字节缓冲，复制进
> Message 会造成明显的帧停顿和内存峰值。需要读取新值时调用
> `event.asset(&assets)`，或用 `event.asset_id` 查询 `Assets<A>`。

### 使用示例

```rust
use bevy_assets_hmr::{ConfigHmrAppExt, ConfigHmrPlugin, AssetChanged, AssetBind};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins) // 启用 bevy/file_watcher
        .add_plugins(ConfigHmrPlugin::default())
        // 一行声明要追踪的类型（不加载文件，只注册基础设施）
        .watch_asset::<Image>()
        .watch_asset::<Scene>()
        .add_systems(Update, on_image_changed)
        .add_systems(Startup, setup)
        .run();
}

fn setup(mut commands: Commands, asset_server: Res<AssetServer>) {
    let handle = asset_server.load::<Image>("textures/player.png");
    commands.spawn((
        Sprite::from_image(handle.clone()),
        AssetBind::new(handle),  // HMR 自动追踪，变更时填充 target_entities
    ));
}

fn on_image_changed(
    mut reader: MessageReader<AssetChanged<Image>>,
    images: Res<Assets<Image>>,
) {
    for evt in reader.read() {
        if let Some(image) = evt.asset(&images) {
            info!(
                "图片已更新，{} 个实体受影响，尺寸 {:?}",
                evt.target_entities.len(),
                image.texture_descriptor.size,
            );
        }
    }
}
```

### 在 `bsn!` 宏中使用 `AssetBind`

Bevy 0.19 的 `bsn!` 宏支持把 `AssetBind<A>` 当作组件写在场景条目里。`AssetBind<A>` 内含 `Handle<A>`,而 Bevy 用 `SpecializeFromTemplate` auto-trait 把 `Handle<A>` 刻意排除在 `Unpin` 之外,导致它无法走 `FromTemplate` 的 blanket impl。为此本 crate 提供了手写的 `AssetBindTemplate<A>` 作为 `<AssetBind<A> as FromTemplate>::Template`,你无需关心其存在,直接写 `AssetBind<A>` 即可。

```rust
fn setup(mut commands: Commands, asset_server: Res<AssetServer>) {
    // 写法 A:{expr} 内联表达式
    commands.spawn_scene(bsn! {
        Node { width: percent(100), height: percent(100) }
        Children [
            (
                ImageNode { image: {asset_server.load("textures/bg.jpg")} }
                AssetBind<Image> { handle: {asset_server.load("textures/bg.jpg")} }
            ),
        ]
    });

    // 写法 B:外部变量(注意:裸 ident 在 bsn! 中会被当成函数名,必须用 { } 包裹)
    let handle = asset_server.load::<Image>("textures/bg.jpg");
    commands.spawn_scene(bsn! {
        Node { width: percent(100), height: percent(100) }
        Children [
            (
                ImageNode { image: {handle.clone()} }
                AssetBind<Image> { handle: {handle.clone()} }
            ),
        ]
    });
}
```

> **注意**:`ImageNode` 的图片热更由 Bevy 原生 `RenderAssets::<GpuImage>` 负责(改了文件自动重传 GPU),**不需要** `AssetBind`。`AssetBind` 只在你需要"知道哪些实体绑定了这张图、收到 `AssetChanged<Image>` 后做额外自定义响应"时才加(如重建图集、重建碰撞体、记日志等)。

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

# bsn! 内联 AssetBind：验证 AssetBind<Image> 能直接在 bsn! 宏中作为组件使用
cargo run --example bsn_asset_bind -p bevy_assets_hmr

# 窗口版查看器：打开 bevy 窗口，UI 显示配置内容 + 提示（需要 bevy_render + bevy_winit features）
cargo run --example basic_viewer -p bevy_assets_hmr

# 控制台查看器：格式化打印配置内容 + 修改提示
cargo run --example viewer -p bevy_assets_hmr

# bevy_asset_loader 窗口版查看器：LoadingState 加载 + HMR 实时刷新
cargo run --example asset_loader_viewer -p bevy_assets_hmr
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

### `bsn_asset_bind.rs` - `bsn!` 内联 `AssetBind`

验证 `AssetBind<Image>` 能否作为组件直接写在 `bsn!` 宏的场景条目中,以及 `{asset_server.load(...)}` 内联表达式作为字段值是否合法。同时演示外部 `Handle` 变量作为字段值的写法。

```rust
commands.spawn_scene(bsn! {
    Node { width: percent(100), height: percent(100) }
    Children [
        (
            ImageNode { image: {asset_server.load(BG_PATH)} }
            AssetBind<Image> { handle: {asset_server.load(BG_PATH)} }
        ),
    ]
});
```

> **前提**:App 构建时需调用 `app.watch_asset::<Image>()` 注册 `AssetBindCache<Image>` 与追踪系统。运行前需要 `assets/textures/bg.jpg`(已随仓库附带)。

### `basic_viewer.rs` - 窗口版查看器（`DefaultPlugins`）

打开一个真实 bevy 窗口，在 UI 中显示示例 NPC 配置内容，并提示"修改文件后可重新运行查看更新"。演示了 `MinimalPlugins + WindowPlugin + WinitPlugin` 组合下的窗口 + UI 文本渲染。不含 HMR 消息系统（窗口环境下消息初始化有 GPU 驱动依赖差异）。

```rust
app.add_plugins(DefaultPlugins);  // 开窗口
app.add_systems(Startup, setup_ui);  // UI 渲染
```

> **前提**：需要 `bevy_render` + `bevy_winit` dev-dependencies。

### `viewer.rs` - 控制台查看器

启动后格式化打印当前配置内容，并提示修改 RON 文件后可重新运行查看更新。基于 `basic.rs` 的 headless HMR 流程，是日常开发最常用的查看方式。

```rust
app.register_config::<NpcDatabase>("data/npc.ron");  // 自动加载
// 启动系统直接读取 Assets 打印
// 修改文件后重新运行即可看到更新
```

运行输出示例：
```
╔══════════════════════════════════════╗
║  📦 bevy_assets_hmr 配置查看器     ║
║  共 3 条 NPC 数据：                ║
║    npc_1 — 商人 (HP: 100)          ║
║    npc_2 — 守卫 (HP: 150)          ║
║    npc_3 — 法师 (HP: 80)            ║
║  💡 修改 assets/data/npc.ron 后     ║
║  重新运行即可看到更新内容。          ║
╚══════════════════════════════════════╝
```

### `asset_loader_viewer.rs` - `bevy_asset_loader` 窗口版（GUI 完整流程）

用 `LoadingState` 加载 `ConfigAsset<MyConfig>`，进入 `GameState::Ready` 后由 `GameAssets::hmr_plugin(GameState::Ready)` 把 handle 自动接入 HMR，UI 显示配置内容，修改 `assets/data/config.ron` 后窗口自动更新。是"兼容 bevy_asset_loader"章节的可运行实例。

```rust
app.init_state::<GameState>()
    .add_loading_state(
        LoadingState::new(GameState::Loading)
            .continue_to_state(GameState::Ready)
            .load_collection::<GameAssets>(),
    )
    .add_plugins(GameAssets::hmr_plugin(GameState::Ready));
```

> **前提**：需要 `bevy/file_watcher` feature（dev-deps 已启用）。

## Bevy 0.19 兼容性

### `bevy/file_watcher` 文件监听

本 crate 不启用 `bevy/file_watcher`，由消费方按需启用：

```toml
[features]
hmr = ["bevy/file_watcher"]
```

启用后，修改 `assets/` 下的文件会自动 reload → 派发 `AssetEvent::Modified` → HMR 核心 diff → `ConfigRefresh<T>`。渲染资产（mesh/texture）走 bevy 原生自动更新，配置数据走 HMR 框架 diff + 定向刷新，两者共享同一个 file_watcher，不冲突。

### Headless 环境配置

标准的 `DefaultPlugins` 或 `MinimalPlugins` 应用不需要调用
`setup_hmr_headless()`；该 helper 只用于手动驱动 schedule/message 的特殊测试环境。

### `Assets::insert` 的事件派发时机

`Assets::insert` 把事件写入 `queued_events`，由 `Assets::asset_events` 系统（在 `PostUpdate`）flush 到 `Messages<AssetEvent>`。同一帧内 `insert` 后立刻读 `MessageReader` 读不到（要等下一帧）。测试用 `insert_config` 直接初始化快照绕过这个延迟。

## 常见问题（FAQ）

### Q: ron/json 语法写错了会怎样？会丢失数据吗？

不会。Bevy 在热重载失败时会保留上一有效资产；插件监听真实的加载失败事件并派发 `ConfigReloadFailed<T>`，其中包含资产 ID、源路径、原始错误以及可用时的上一有效配置。插件不会向 `Assets` 人工回写快照，因此不会额外制造一次 `Modified` 事件。

### Q: 修改 GLTF 贴图后，材质为什么不联动刷新？

Bevy 的 `AssetServer` 只重载被修改的文件本身，不解析资产内部依赖树。`watch_asset` 只通知文件变更，不会自动刷新引用该贴图的材质。如需联动，请在 `AssetChanged<Image>` 订阅方手动处理材质重建。

### Q: 频繁保存文件（多次 Ctrl+S）会触发多次重载吗？

不会。内置防抖窗口（默认 150ms，可通过 `ConfigHmrPlugin { debounce_window: ... }` 配置）会合并同一资产短时间内的多次事件，并以最后一次事件代表最终状态。插件不会在刷新后的固定时间内丢弃新事件，因此连续合法保存仍会更新到最终版本。

### Q: Release 构建时 HMR 还有运行时开销吗？

零开销。`dev` feature（默认启用）控制所有 HMR 运行时系统。发布时用 `--no-default-features` 编译，所有监听、diff、派发逻辑被编译期完全剔除：
```toml
[dependencies]
bevy_assets_hmr = { git = "https://github.com/remloyal/bevy_assets_hmr", default-features = false }
```

### Q: Web（WASM）平台能用吗？

目前不支持。`bevy/file_watcher` 在 WASM 环境中无法工作，本框架暂无替代轮询方案。这是已知限制。

## License

MIT
