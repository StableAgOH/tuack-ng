# tuack-ng-parser

基于 `rushdown` 的 Markdown 解析器，输出 AST；独立 crate，不依赖 workspace 内其他 crate。

- span：多数叶子节点精确覆盖源码；`Code` 叶子的 span 为空区间（指向开头反引号）；块与复合节点只保证定位，`start` 可靠。
- 实现 `Visitor` 时，重写 `visit_*` 后须自行调用对应的 `walk_*`，否则不会继续遍历子树。

## 边界

只在本 crate 写解析、AST、span、visitor / transform 与打印。题面规则在 `tuack-utils/doc`，渲染与导出行为在 `tuack-utils/ren`、`tuack-utils/dump`，业务类型与契约在 `tuack-lib`。
