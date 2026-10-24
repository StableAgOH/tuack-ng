# tuack-lib

契约层：只定义数据与接口，不实现文件系统或工具链，不依赖 `gctx()`、`tuack-config` 类型与 `tokio`；全同步。

## 流式 I/O（硬规则）

输入、输出、答案都可能到 GB 级：

- 一律 `Box<dyn Reader>` / `&mut dyn Reader` 流转，用 `std::io::copy`；不 `read_to_end` / `fs::read`，不引入中间 `Vec<u8>`，不克隆流。
- `Option` 表示文件不存在，不用「空文件」代替；读输入/答案失败是判定失败（UKE），不要中止整个运行。
- 重新打开文件前先关句柄；临时文件作为产物返回时用 `KeepAliveReader` 携带临时目录。
- 只有小数据可进 `Vec<u8>`：stderr、编译/检查/校验命令输出、配置/模板、题面里直接展示的短样例。

## 会话

`TestSession::judge` 是单测试点原子工具：设置 limits/io-mode、注入输入、执行、校验并归一化成 `[0,1]` 分数；不处理子任务与总分（评分策略在 CLI）。`DmkSession` 同样以单测试点为单位生成输入/输出。

## 插件 API 版本

`PLUGIN_API_VERSION` 是宿主 API 版本，插件清单用 `pluginapi` 声明。加载要求主版本号相同、次版本号不高于宿主，补丁版本不参与判定；破坏性变更升主版本号，兼容性新增升次版本号。

## 与其他 crate 的边界

- 实现去 `tuack-utils`，配置模型去 `tuack-config`，CLI 展示去 `tuack-ng`。
- 只在本 crate 写：trait 与共享数据类型、会话契约、插件跨边界的数据交换与 `PLUGIN_API_VERSION`。
