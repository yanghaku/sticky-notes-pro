# Sticky Notes Pro 产品规格

本文为产品与技术基线的入口，正文按职责维护在 `product-spec/`。

## 分主题规范

| 文件 | 负责内容 |
|---|---|
| [产品范围](product-spec/scope.md) | 定位、当前交付范围、暂不实现与设计原则 |
| [窗口与平台](product-spec/windows.md) | 单实例、窗口生命周期、启动恢复、尺寸 / 位置继承和平台降级 |
| [界面与便签管理](product-spec/interface.md) | 标题栏、外观 / 颜色、列表、删除 / 恢复、设置 Tab |
| [编辑器](product-spec/editor.md) | 文档结构、列表、选区 / 删除、Undo / Redo、剪贴板与 IME |
| [快捷键](product-spec/shortcuts.md) | 当前固定按键及作用范围 |
| [存储与保存](product-spec/storage.md) | 数据格式、版本、目录、事务、自动保存与失败恢复 |
| [技术架构](product-spec/architecture.md) | 技术职责、实际模块边界、性能与资源约束 |

## 阅读与维护

- 开始某项工作时先读范围，再读对应主题及其引用的契约；跨编辑 / 保存 / 窗口的变更需要一起核对。
- 每条规则在所属主题维护一次，其他主题通过链接引用；待验证场景只记录在开发待办。
- 完整 SQL、依赖版本和具体算法以链接到的源码为实现来源。
- 未实现、待测及实现差距只记录在 [development-status.md](development-status.md)，不向其中追加完成历史。
