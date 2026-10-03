//! Fixed, note-local shortcuts and their settings-page descriptions.

use slint::winit_030::winit::keyboard::{Key, ModifiersState};

use crate::ShortcutEntry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteShortcut {
    NewNote,
    NotesList,
    Settings,
    Save,
    Close,
    TogglePin,
}

struct Binding {
    action: NoteShortcut,
    label: &'static str,
    key: &'static str,
    shift: bool,
}

const NOTE_BINDINGS: [Binding; 6] = [
    Binding {
        action: NoteShortcut::NewNote,
        label: "新建便签",
        key: "N",
        shift: false,
    },
    Binding {
        action: NoteShortcut::NotesList,
        label: "打开便签列表",
        key: "L",
        shift: true,
    },
    Binding {
        action: NoteShortcut::Settings,
        label: "打开设置",
        key: ",",
        shift: false,
    },
    Binding {
        action: NoteShortcut::Save,
        label: "保存 / 重试保存",
        key: "S",
        shift: false,
    },
    Binding {
        action: NoteShortcut::Close,
        label: "关闭当前便签",
        key: "W",
        shift: false,
    },
    Binding {
        action: NoteShortcut::TogglePin,
        label: "置顶 / 取消置顶（桌面支持时）",
        key: "P",
        shift: true,
    },
];

/// Do not treat Super on Windows/Linux, or AltGr, as an editor command.
pub fn primary_pressed(modifiers: ModifiersState) -> bool {
    let primary = if cfg!(target_os = "macos") {
        modifiers.super_key() && !modifiers.control_key()
    } else {
        modifiers.control_key() && !modifiers.super_key()
    };
    primary && !modifiers.alt_key()
}

pub fn note_action(key: &Key, modifiers: ModifiersState) -> Option<NoteShortcut> {
    if !primary_pressed(modifiers) {
        return None;
    }
    let Key::Character(key) = key else {
        return None;
    };
    NOTE_BINDINGS
        .iter()
        .find(|binding| {
            key.eq_ignore_ascii_case(binding.key) && modifiers.shift_key() == binding.shift
        })
        .map(|binding| binding.action)
}

pub fn descriptions() -> Vec<ShortcutEntry> {
    let primary = if cfg!(target_os = "macos") {
        "Cmd"
    } else {
        "Ctrl"
    };
    let mut entries = Vec::new();
    entries.push(entry("便签窗口", "", true));
    entries.extend(NOTE_BINDINGS.iter().map(|binding| {
        let keys = format!(
            "{primary}+{}{key}",
            if binding.shift { "Shift+" } else { "" },
            key = binding.key
        );
        entry(binding.label, &keys, false)
    }));
    entries.push(entry("编辑", "", true));
    for (label, keys) in [
        ("复制", "C"),
        ("剪切", "X"),
        ("粘贴纯文本", "V"),
        ("全选", "A"),
        ("撤销", "Z"),
        ("重做", "Shift+Z / {primary}+Y"),
    ] {
        let keys = format!("{primary}+{keys}").replace("{primary}", primary);
        entries.push(entry(label, &keys, false));
    }
    entries.push(entry("移动与选择", "", true));
    for (label, keys) in [
        ("移动光标", "← / → / ↑ / ↓"),
        ("扩展选区", "Shift+方向键"),
        ("段落开头 / 末尾", "Home / End"),
        ("选择到段落开头 / 末尾", "Shift+Home / End"),
    ] {
        entries.push(entry(label, keys, false));
    }
    entries.push(entry(
        "便签开头 / 末尾",
        &format!("{primary}+Home / End"),
        false,
    ));
    entries.push(entry(
        "选择到便签开头 / 末尾",
        &format!("{primary}+Shift+Home / End"),
        false,
    ));
    entries.push(entry("段落与列表", "", true));
    for (label, keys) in [
        ("换段 / 列表续项，空列表项退出", "Enter"),
        ("删除前一字符 / 选区；项首退出列表", "Backspace"),
        ("删除后一字符 / 选区", "Delete"),
        ("段落开头转换有序列表", "输入 1. 后按空格"),
        ("段落开头转换无序列表", "输入 - 或 * 后按空格"),
        ("关闭便签菜单 / 取消设置切页确认", "Escape"),
    ] {
        entries.push(entry(label, keys, false));
    }
    entries
}

fn entry(label: &str, keys: &str, section: bool) -> ShortcutEntry {
    ShortcutEntry {
        label: label.into(),
        keys: keys.into(),
        section,
    }
}
