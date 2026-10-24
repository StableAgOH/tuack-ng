# tuack-plugin-sdk

插件侧 Rust SDK，编译目标 `wasm32-wasip1`；只依赖 `tuack-lib` 与 `tuack-ng-parser`。

- 版本：SDK 的主、次版本号必须与宿主 `PLUGIN_API_VERSION` 一致（有测试守卫），补丁版本可独立升；插件清单的 `pluginapi` 写 SDK 版本，宿主判定时忽略补丁版本。SDK 的 `Cargo.toml` 不继承 workspace 版本。

## 边界

插件契约在 `tuack-lib`，宿主实现（extism 运行时、路径与命令校验）在 `tuack-utils`；本 crate 不依赖 `tuack-utils`，也不被 workspace 内其他 crate 依赖。
