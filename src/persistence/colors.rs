//! Opaque RGB preferences and independently recoverable light/dark note palettes.

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

/// An opaque color: every channel is validated before entering the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb([u8; 3]);

impl Rgb {
    pub const fn from_rgb_u8(red: u8, green: u8, blue: u8) -> Self {
        Self([red, green, blue])
    }

    pub fn from_channels(red: i32, green: i32, blue: i32) -> Result<Self> {
        Ok(Self([
            u8::try_from(red).context("red must be between 0 and 255")?,
            u8::try_from(green).context("green must be between 0 and 255")?,
            u8::try_from(blue).context("blue must be between 0 and 255")?,
        ]))
    }

    /// Parse the visible RGB fields, including incomplete and out-of-range input.
    pub fn from_channel_texts(red: &str, green: &str, blue: &str) -> Result<Self> {
        fn channel(text: &str) -> Result<i32> {
            ensure!(
                !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()),
                "RGB channels must use decimal integers between 0 and 255"
            );
            text.parse()
                .context("RGB channel must be between 0 and 255")
        }
        Self::from_channels(channel(red)?, channel(green)?, channel(blue)?)
    }

    /// Accept exactly six hexadecimal digits after '#', with no alpha or whitespace.
    pub fn parse(text: &str) -> Result<Self> {
        let bytes = text.as_bytes();
        ensure!(
            bytes.len() == 7 && bytes[0] == b'#' && bytes[1..].iter().all(u8::is_ascii_hexdigit),
            "color must use #RRGGBB"
        );
        // The check above establishes ASCII byte boundaries before slicing UTF-8.
        Ok(Self([
            u8::from_str_radix(&text[1..3], 16)?,
            u8::from_str_radix(&text[3..5], 16)?,
            u8::from_str_radix(&text[5..7], 16)?,
        ]))
    }

    pub fn channels(self) -> [u8; 3] {
        self.0
    }

    pub fn hex(self) -> String {
        let [red, green, blue] = self.0;
        format!("#{red:02x}{green:02x}{blue:02x}")
    }
}

impl Serialize for Rgb {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.hex())
    }
}

impl<'de> Deserialize<'de> for Rgb {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// Indices 0..=5 follow the six named themes; index 6 is the note-window border.
/// White/black remains fixed and deliberately has no editable palette entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotePalette {
    pub yellow: Rgb,
    pub green: Rgb,
    pub pink: Rgb,
    pub purple: Rgb,
    pub blue: Rgb,
    pub gray: Rgb,
    pub border: Rgb,
}

impl NotePalette {
    pub const fn light_default() -> Self {
        Self {
            yellow: Rgb([0xff, 0xf4, 0xbf]),
            green: Rgb([0xda, 0xf2, 0xce]),
            pink: Rgb([0xfb, 0xd8, 0xe2]),
            purple: Rgb([0xe8, 0xdd, 0xfa]),
            blue: Rgb([0xd5, 0xeb, 0xfb]),
            gray: Rgb([0xe9, 0xe7, 0xe3]),
            border: Rgb([0x8b, 0x92, 0x9e]),
        }
    }

    pub const fn dark_default() -> Self {
        Self {
            yellow: Rgb([0x3a, 0x33, 0x23]),
            green: Rgb([0x27, 0x39, 0x2c]),
            pink: Rgb([0x3e, 0x29, 0x32]),
            purple: Rgb([0x35, 0x2e, 0x43]),
            blue: Rgb([0x25, 0x37, 0x42]),
            gray: Rgb([0x34, 0x36, 0x38]),
            border: Rgb([0x74, 0x7c, 0x88]),
        }
    }

    pub fn get(&self, index: usize) -> Option<Rgb> {
        match index {
            0 => Some(self.yellow),
            1 => Some(self.green),
            2 => Some(self.pink),
            3 => Some(self.purple),
            4 => Some(self.blue),
            5 => Some(self.gray),
            6 => Some(self.border),
            _ => None,
        }
    }

    pub fn set(&mut self, index: usize, color: Rgb) -> Result<()> {
        let target = match index {
            0 => &mut self.yellow,
            1 => &mut self.green,
            2 => &mut self.pink,
            3 => &mut self.purple,
            4 => &mut self.blue,
            5 => &mut self.gray,
            6 => &mut self.border,
            _ => bail!("unknown palette entry {index}"),
        };
        *target = color;
        Ok(())
    }

    fn read_preferences(&mut self, value: &Value, repaired: &mut bool) {
        let Some(object) = value.as_object() else {
            *repaired = true;
            return;
        };
        for (name, target) in [
            ("yellow", &mut self.yellow),
            ("green", &mut self.green),
            ("pink", &mut self.pink),
            ("purple", &mut self.purple),
            ("blue", &mut self.blue),
            ("gray", &mut self.gray),
            ("border", &mut self.border),
        ] {
            if let Some(value) = object.get(name) {
                match value.as_str().and_then(|text| Rgb::parse(text).ok()) {
                    Some(color) => *target = color,
                    None => *repaired = true,
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NoteColors {
    pub light: NotePalette,
    pub dark: NotePalette,
}

impl Default for NoteColors {
    fn default() -> Self {
        Self {
            light: NotePalette::light_default(),
            dark: NotePalette::dark_default(),
        }
    }
}

impl NoteColors {
    pub(super) fn read_preferences(value: Option<&Value>, repaired: &mut bool) -> Self {
        let mut colors = Self::default();
        let Some(value) = value else {
            return colors;
        };
        let Some(object) = value.as_object() else {
            *repaired = true;
            return colors;
        };
        if let Some(light) = object.get("light") {
            colors.light.read_preferences(light, repaired);
        }
        if let Some(dark) = object.get("dark") {
            colors.dark.read_preferences(dark, repaired);
        }
        colors
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::Settings;

    #[test]
    fn rgb_is_strict_opaque_and_round_trips_as_hex() -> Result<()> {
        let color = Rgb::parse("#aB09Ff")?;
        assert_eq!(color.channels(), [171, 9, 255]);
        assert_eq!(serde_json::to_string(&color)?, "\"#ab09ff\"");
        assert_eq!(serde_json::from_str::<Rgb>("\"#AB09FF\"")?, color);
        for invalid in [
            "#abc",
            "#000000ff",
            "  #123456",
            "#１２３",
            "#gg0000",
            "123456",
        ] {
            assert!(Rgb::parse(invalid).is_err(), "{invalid}");
        }
        assert!(Rgb::from_channels(-1, 0, 0).is_err());
        assert!(Rgb::from_channels(0, 256, 0).is_err());
        assert_eq!(Rgb::from_channels(0, 255, 9)?.hex(), "#00ff09");
        assert_eq!(Rgb::from_channel_texts("0", "255", "009")?.hex(), "#00ff09");
        for invalid in [
            "",
            "256",
            "999",
            "-1",
            "+1",
            "1.5",
            " 1",
            "１",
            "99999999999",
        ] {
            for channels in [
                [invalid, "0", "0"],
                ["0", invalid, "0"],
                ["0", "0", invalid],
            ] {
                assert!(Rgb::from_channel_texts(channels[0], channels[1], channels[2]).is_err());
            }
        }
        let mut palette = NotePalette::dark_default();
        palette.set(6, color)?;
        assert_eq!(palette.get(6), Some(color));
        assert!(palette.set(7, color).is_err());
        assert_eq!(palette.get(7), None);
        Ok(())
    }

    #[test]
    fn settings_repair_colors_independently_and_preserve_other_preferences() -> Result<()> {
        let mut file = tempfile::NamedTempFile::new()?;
        serde_json::to_writer(
            &mut file,
            &serde_json::json!({
                "note": {"default_theme": "blue", "colors": {
                    "light": {"yellow": "#010203", "pink": "#fff", "border": "#ff000080"},
                    "dark": {"blue": "#AABBCC"}
                }},
                "editor": {"font_size": 20}, "scale_percent": 125
            }),
        )?;
        let loaded = Settings::load(file.path())?;
        assert!(loaded.writable && loaded.warning.is_some());
        assert_eq!(
            loaded.settings.note.colors.light.yellow,
            Rgb::parse("#010203")?
        );
        assert_eq!(
            loaded.settings.note.colors.light.pink,
            NotePalette::light_default().pink
        );
        assert_eq!(
            loaded.settings.note.colors.light.border,
            NotePalette::light_default().border
        );
        assert_eq!(
            loaded.settings.note.colors.dark.blue,
            Rgb::parse("#aabbcc")?
        );
        assert_eq!(loaded.settings.note.default_theme, "blue");
        assert_eq!(loaded.settings.editor.font_size, 20.0);
        assert_eq!(loaded.settings.scale_percent, 125);
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("settings.json");
        loaded.settings.save(&path)?;
        let reopened = Settings::load(&path)?;
        assert_eq!(reopened.settings.note.colors, loaded.settings.note.colors);
        assert!(reopened.warning.is_none());
        let mut repaired = false;
        let groups = serde_json::json!({"light": [], "dark": {"green": "#112233"}});
        let colors = NoteColors::read_preferences(Some(&groups), &mut repaired);
        assert!(repaired);
        assert_eq!(colors.light, NotePalette::light_default());
        assert_eq!(colors.dark.green, Rgb::parse("#112233")?);
        repaired = false;
        assert_eq!(
            NoteColors::read_preferences(None, &mut repaired),
            NoteColors::default()
        );
        assert!(!repaired);
        assert_eq!(
            NoteColors::read_preferences(Some(&Value::Null), &mut repaired),
            NoteColors::default()
        );
        assert!(repaired);
        Ok(())
    }
}
