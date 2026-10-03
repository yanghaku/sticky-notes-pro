# 固定快捷键

[规格入口](../product-spec.md)

当前不支持改键或系统全局快捷键。设置页展示实际支持的操作；绑定与说明的实现见 [src/shortcuts.rs](../../src/shortcuts.rs)。

下表以 Windows / Linux 的 Ctrl 为例，macOS 使用 Cmd。便签操作与编辑命令只在当前便签窗口生效，不影响其他应用。

## 便签窗口

| 操作 | 按键 |
|---|---|
| 新建便签 | Ctrl+N |
| 打开 / 激活便签列表 | Ctrl+Shift+L |
| 打开 / 激活设置 | Ctrl+, |
| 保存 / 重试保存 | Ctrl+S |
| 关闭当前便签 | Ctrl+W |
| 置顶 / 取消置顶 | Ctrl+Shift+P（平台支持时） |

自动保存始终开启，不依赖 Ctrl+S。IME 组合、冻结状态或菜单打开时不误触便签操作，长按不重复创建窗口。

## 编辑与导航

| 操作 | 按键 |
|---|---|
| 复制 / 剪切 / 粘贴纯文本 | Ctrl+C / Ctrl+X / Ctrl+V |
| 全选 | Ctrl+A |
| 撤销 | Ctrl+Z |
| 重做 | Ctrl+Shift+Z；Windows / Linux 另支持 Ctrl+Y |
| 移动光标 / 扩展选区 | 方向键 / Shift+方向键 |
| 段落开头 / 末尾 | Home / End |
| 选择到段落开头 / 末尾 | Shift+Home / Shift+End |
| 文档开头 / 末尾 | Ctrl+Home / Ctrl+End |
| 选择到文档开头 / 末尾 | Ctrl+Shift+Home / Ctrl+Shift+End |

## 列表与界面操作

- Enter：换段、列表续项；空列表项退出格式。
- Backspace / Delete：删除前 / 后字素或选区；列表项首 Backspace 退出格式。
- 段落开头输入 `1. `、`- `、`* `：转换有序 / 无序列表，不是组合快捷键。
- Escape：关闭便签菜单；设置切页确认中取消切换，保留当前修改。

完整结构与历史语义见[编辑器规范](editor.md)，设置的保存 / 丢弃 / 取消见[界面规范](interface.md#设置)。
