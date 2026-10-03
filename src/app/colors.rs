//! Isolated settings color drafts, RGB/HEX synchronization and contrast previews.

use super::*;
use crate::ColorSlot;
use crate::persistence::{NoteColors, NotePalette, Rgb};

const SLOT_NAMES: [&str; 7] = ["黄色", "绿色", "粉色", "紫色", "蓝色", "灰色", "窗口边框"];

pub(super) struct ColorDraft {
    pub(super) colors: NoteColors,
    mode: usize,
    slot: usize,
}

impl ColorDraft {
    fn palette(&self) -> &NotePalette {
        if self.mode == 0 {
            &self.colors.light
        } else {
            &self.colors.dark
        }
    }

    fn palette_mut(&mut self) -> &mut NotePalette {
        if self.mode == 0 {
            &mut self.colors.light
        } else {
            &mut self.colors.dark
        }
    }
}

pub(super) fn ui_color(rgb: Rgb) -> slint::Color {
    let [r, g, b] = rgb.channels();
    slint::Color::from_rgb_u8(r, g, b)
}

pub(super) fn palette_model(palette: &NotePalette) -> ModelRc<slint::Color> {
    ModelRc::new(VecModel::from(
        [
            palette.yellow,
            palette.green,
            palette.pink,
            palette.purple,
            palette.blue,
            palette.gray,
        ]
        .into_iter()
        .map(ui_color)
        .collect::<Vec<_>>(),
    ))
}

impl App {
    pub(super) fn reset_color_values(&mut self) {
        let Some(ui) = &self.settings_ui else { return };
        let (mode, slot) = self.color_draft.as_ref().map_or_else(
            || (usize::from(ui.global::<AppAppearance>().get_dark()), 0),
            |draft| (draft.mode, draft.slot),
        );
        self.color_draft = Some(ColorDraft {
            colors: self.settings.note.colors,
            mode,
            slot,
        });
        self.sync_color_controls();
    }

    pub(super) fn wire_colors(&self, shared: &Rc<RefCell<Self>>, ui: &SettingsWindow) {
        let weak = Rc::downgrade(shared);
        ui.on_select_color(move |mode, slot| {
            dispatch(&weak, |app, _| app.select_color(mode, slot));
        });
        let weak = Rc::downgrade(shared);
        ui.on_edit_color_rgb(move |r, g, b| {
            dispatch(&weak, |app, _| {
                match Rgb::from_channel_texts(r.as_str(), g.as_str(), b.as_str()) {
                    Ok(color) => app.edit_color(color)?,
                    Err(_) => app.color_input_error("RGB 每个分量必须是 0–255 的整数"),
                }
                Ok(())
            });
        });
        let weak = Rc::downgrade(shared);
        ui.on_edit_color_hex(move |text| {
            dispatch(&weak, |app, _| {
                if let Some(ui) = &app.settings_ui {
                    ui.set_color_hex(text.clone());
                }
                match Rgb::parse(text.as_str()) {
                    Ok(color) => app.edit_color(color)?,
                    Err(_) => app.color_input_error("HEX 必须是 #RRGGBB，例如 #aabbcc"),
                }
                Ok(())
            });
        });
        let weak = Rc::downgrade(shared);
        ui.on_reset_color(move |all| dispatch(&weak, |app, _| app.reset_colors(all)));
    }

    fn select_color(&mut self, mode: i32, slot: i32) -> Result<()> {
        let Some(ui) = &self.settings_ui else {
            return Ok(());
        };
        // Incomplete input remains visible instead of vanishing on selection.
        // A valid edit in either representation or Reset can repair it.
        if !ui.get_colors_valid() {
            ui.set_color_error("请先修正 RGB / HEX，或恢复当前颜色默认值，再切换颜色".into());
            return Ok(());
        }
        anyhow::ensure!(
            (0..=1).contains(&mode) && (0..=6).contains(&slot),
            "invalid color selection"
        );
        if let Some(draft) = &mut self.color_draft {
            draft.mode = usize::try_from(mode)?;
            draft.slot = usize::try_from(slot)?;
            self.sync_color_controls();
        }
        Ok(())
    }

    fn edit_color(&mut self, color: Rgb) -> Result<()> {
        let draft = self
            .color_draft
            .as_mut()
            .context("color settings are closed")?;
        let slot = draft.slot;
        draft.palette_mut().set(slot, color)?;
        self.sync_color_controls();
        Ok(())
    }

    fn color_input_error(&self, message: &str) {
        if let Some(ui) = &self.settings_ui {
            ui.set_colors_valid(false);
            ui.set_colors_dirty(true);
            ui.set_color_error(message.into());
        }
    }

    fn reset_colors(&mut self, all: bool) -> Result<()> {
        let draft = self
            .color_draft
            .as_mut()
            .context("color settings are closed")?;
        let defaults = NoteColors::default();
        if all {
            draft.colors = defaults;
        } else {
            let palette = if draft.mode == 0 {
                defaults.light
            } else {
                defaults.dark
            };
            let slot = draft.slot;
            draft
                .palette_mut()
                .set(slot, palette.get(slot).context("invalid color slot")?)?;
        }
        self.sync_color_controls();
        Ok(())
    }

    pub(super) fn color_values_for_save(&self) -> Result<NoteColors> {
        let ui = self.settings_ui.as_ref().context("settings are closed")?;
        anyhow::ensure!(ui.get_colors_valid(), "请先修正便签配色中的 RGB / HEX 输入");
        let draft = self
            .color_draft
            .as_ref()
            .context("color settings are closed")?;
        // Never save the last valid value while the visible field holds a
        // different or invalid value, even if an input callback was dropped.
        let shown = Rgb::parse(ui.get_color_hex().as_str())?;
        let rgb = Rgb::from_channel_texts(
            ui.get_color_red().as_str(),
            ui.get_color_green().as_str(),
            ui.get_color_blue().as_str(),
        )?;
        anyhow::ensure!(
            rgb == shown && draft.palette().get(draft.slot) == Some(shown),
            "配色输入尚未同步，请重新修改后保存"
        );
        Ok(draft.colors)
    }

    fn sync_color_controls(&self) {
        let (Some(ui), Some(draft)) = (&self.settings_ui, &self.color_draft) else {
            return;
        };
        let Some(selected) = draft.palette().get(draft.slot) else {
            return;
        };
        let slots = [&draft.colors.light, &draft.colors.dark]
            .into_iter()
            .flat_map(|palette| {
                SLOT_NAMES.iter().enumerate().filter_map(|(slot, name)| {
                    palette.get(slot).map(|color| ColorSlot {
                        name: (*name).into(),
                        value: ui_color(color),
                        hex: color.hex().into(),
                    })
                })
            })
            .collect::<Vec<_>>();
        ui.set_color_slots(ModelRc::new(VecModel::from(slots)));
        ui.set_color_mode(draft.mode as i32);
        ui.set_color_slot(draft.slot as i32);
        let [r, g, b] = selected.channels();
        ui.set_color_red(r.to_string().into());
        ui.set_color_green(g.to_string().into());
        ui.set_color_blue(b.to_string().into());
        ui.set_color_hex(selected.hex().into());
        ui.set_color_error("".into());
        ui.set_colors_valid(true);
        ui.set_colors_dirty(draft.colors != self.settings.note.colors);
        let dark = draft.mode == 1;
        let background = if draft.slot < 6 {
            selected
        } else {
            fixed_background(dark)
        };
        let foreground = body_color(dark);
        ui.set_color_preview_background(ui_color(background));
        ui.set_color_preview_border(ui_color(draft.palette().border));
        ui.set_color_preview_foreground(ui_color(foreground));
        let low_contrast = if draft.slot < 6 {
            contrast(foreground, background) < 4.5
                || contrast(draft.palette().border, background) < 1.5
        } else {
            (0..6)
                .filter_map(|slot| draft.palette().get(slot))
                .chain([background])
                .any(|background| contrast(selected, background) < 1.5)
        };
        ui.set_color_warning(
            if low_contrast {
                "正文或边框与背景对比偏低，建议调整；仍可保存此配色。"
            } else {
                ""
            }
            .into(),
        );
    }
}

fn fixed_background(dark: bool) -> Rgb {
    if dark {
        Rgb::from_rgb_u8(32, 33, 36)
    } else {
        Rgb::from_rgb_u8(255, 255, 255)
    }
}

fn body_color(dark: bool) -> Rgb {
    if dark {
        Rgb::from_rgb_u8(235, 238, 242)
    } else {
        Rgb::from_rgb_u8(52, 48, 43)
    }
}

fn contrast(a: Rgb, b: Rgb) -> f64 {
    fn luminance(rgb: Rgb) -> f64 {
        let [r, g, b] = rgb.channels().map(|channel| {
            let value = f64::from(channel) / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        });
        r * 0.2126 + g * 0.7152 + b * 0.0722
    }
    let a = luminance(a);
    let b = luminance(b);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}
