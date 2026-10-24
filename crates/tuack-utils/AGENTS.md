# tuack-utils

`tuack-lib` 契约的具体实现；不依赖 CLI、不使用 `gctx()`、不直接向用户输出（警告与产物经返回值）。

## 异步模型

代码库唯一的异步点在 `process.rs`：`ProcessSupervisor` 用 `tokio::select!` 同时等待子进程退出与内存采样，确认超限即终止；同步入口 `supervise_blocking` 在惰性构建的 `current_thread` 运行时上 `block_on`。`tokio` 只在这里使用，其余全部同步。

## 插件 ABI

宿主函数表与 SDK 的 `#[host_fn]` 声明没有自动一致性校验，改 ABI 需要用真实插件验证。

## 边界

契约在 `tuack-lib`，配置模型在 `tuack-config`，AST 在 `tuack-ng-parser`，CLI 展示在 `tuack-ng`。本 crate 只写实现与副作用（文件系统、子进程、外部工具、插件运行时）。
