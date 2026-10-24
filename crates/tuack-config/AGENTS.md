# tuack-config

`conf.json` 的数据模型、加载/保存与版本迁移；只依赖 `tuack-lib`。

- 三级配置 contest -> day -> problem；`load_config` 从当前目录向上定位当前层级，`CurrentLocation` 表示所在层级。
- 加载整棵配置树；单个比赛日或题目失败只记录错误并继续。
- 配置有文件视图与完整视图（`FileView` / `FullView`）；写回 `conf.json` 只用文件视图。
- 版本在 `CONFIG_MIN_VERSION` 与 `CONFIG_VERSION` 之间时按迁移链升级；缺迁移器，或迁移要求人工确认（`force`）而调用方未启用时，按错误处理。

## 边界

只在本 crate 写：`conf.json` 字段、默认值、文件/完整视图、版本迁移与 `CurrentLocation`。消费配置的代码在 `tuack-utils` / `tuack-ng`，实现类逻辑不在这里。
