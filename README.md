# bevy_assets_hmr

极简的 **配置文件热重载** 插件（Bevy 0.20）。

只需要注册要监听的文件；文件在磁盘上变化后，值会被原地写回世界。
消费侧没有任何包装类型、事件或钩子——你的配置类型本身就是 `Resource`，
在任何地方 `Res<你的类型>` 即可。

内置支持 `.ron` 和 `.json`（按扩展名自动选择），其它格式用 `watch_config_with` 传入解析函数。

## 快速开始

```toml
[dependencies]
bevy_assets_hmr = { git = "https://github.com/remloyal/bevy_assets_hmr" }
```

```rust
use bevy::prelude::*;
use bevy_assets_hmr::ConfigWatchAppExt;
use serde::Deserialize;

#[derive(Resource, Deserialize, Default)]
struct Settings {
    title: String,
}

fn main() {
    App::new()
        .add_plugins(MinimalPlugins)
        // 格式由扩展名决定（.ron / .json）；默认相对 `assets/` 解析
        .watch_config::<Settings>("data/settings.ron")
        .add_systems(Update, use_settings)
        .run();
}

// 像读任何其他 Resource 一样读它
fn use_settings(settings: Res<Settings>) {
    if !settings.is_changed() {
        return;
    }
    println!("title = {}", settings.title);
}
```

## 多格式

```rust
app.watch_config::<Settings>("data/settings.ron")   // ron::from_str
   .watch_config::<Server>("data/server.json")      // serde_json::from_str
   .watch_config_with::<Theme, _>("data/theme.toml", |text| {
       toml::from_str(text).map_err(|error| error.to_string())
   });
```

| 扩展名 | 解析器 |
|---|---|
| `.ron` | `ron::from_str` |
| `.json` | `serde_json::from_str` |
| 其它 | 必须用 `watch_config_with` 提供解析函数；直接 `watch_config` 会 panic 并提示 |

## 对配置类型的要求

`T` 需要 `Resource + Deserialize + Default`：

- `Deserialize`：解析文件；
- `Resource`：让值能存在于世界中；
- `Default`：在首次加载成功之前 `Res<T>` 也能解析（此时读到 `T::default()`，
  文件缺失会写一条 warning 日志）。这样消费方不需要 `Option<Res<T>>`，
  也不会因为资源不存在而 panic。

## 行为

- **自动加载**：注册的类型在 app 第一次 update 时即加载文件，无需单独的 Startup 步骤。
- **变更检测**：文件每次轮询都会重新读取并与上一版文本比较（不依赖文件系统 mtime 精度）。只有加载成功才重新插入资源，因此 `Res::is_changed()` 只在真正热更新时触发，不会空转。
- **防抖**：默认每 150ms 轮询一次，天然合并编辑器的原子写入（删除 + 重命名）。
- **出错不丢数据**：解析失败或文件暂时缺失时，上一份合法值仍保留在世界里，错误写入日志（`bevy::log`）；文件修好后自动恢复。
- **一个类型一个文件**：同一个 `T` 重复注册会打 warning 并忽略第二个路径。

## API

| 项 | 说明 |
|---|---|
| `App::watch_config::<T>(path)` | 按扩展名（`.ron`/`.json`）注册要监听的文件。`T` 需实现 `Resource + Deserialize + Default`。 |
| `App::watch_config_with::<T, F>(path, parser)` | 自定义格式，`parser: Fn(&str) -> Result<T, String>`。 |
| `ConfigWatchPlugin` | 可选插件，用于自定义 `root`（默认 `assets`）与 `poll_interval`（默认 150ms）。须在注册之前添加。 |
| `DEFAULT_ROOT` / `DEFAULT_POLL_INTERVAL` | 上述默认值常量。 |

## 说明

- 路径相对于 `ConfigWatchPlugin::root`（默认 `assets/`）；绝对路径按原样使用。
- 本插件不使用 Bevy 的 `AssetServer`，因此不需要 `bevy/file_watcher` feature，也不经过资产系统。
- 解析错误与文件缺失只写日志：因为资源就是你的类型本身，没有地方存放结构化的错误信息。需要程序化处理错误时，可自行在业务系统里校验读到的值。

## 示例

```bash
cargo run --example simplest     # 自包含：加载 / 重载 / 损坏 / 恢复
cargo run --example live_watch   # 手动编辑 assets/data/npc.ron 看热更新
cargo run --example formats      # ron + json + 自定义格式共存
```

## License

MIT
