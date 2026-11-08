//! Adjust Image presets: save, load, import and export as JSON.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::adjust::Adjustments;
use crate::process::ImageMode;

/// One named preset: the image adjustments plus mode and resolution.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AdjustPreset {
    pub name: String,
    pub adjustments: Adjustments,
    pub mode: ImageMode,
    pub dpi: f64,
    pub pass_through: bool,
}

impl Default for AdjustPreset {
    fn default() -> Self {
        Self { name: String::new(), adjustments: Adjustments::default(), mode: ImageMode::default(), dpi: 254.0, pass_through: false }
    }
}

/// Errors reading or writing presets.
#[derive(Debug)]
pub enum PresetError {
    Io(std::io::Error),
    Json(serde_json::Error),
    /// The file was written by a newer DarkBurn.
    UnsupportedVersion(u32),
}

impl std::fmt::Display for PresetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "could not read or write the preset file: {e}"),
            Self::Json(e) => write!(f, "the preset file is not valid: {e}"),
            Self::UnsupportedVersion(v) => write!(f, "the preset file is version {v}, which this DarkBurn cannot read"),
        }
    }
}

impl std::error::Error for PresetError {}

impl From<std::io::Error> for PresetError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for PresetError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

/// A list of presets, stored as one JSON file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PresetLibrary {
    pub version: u32,
    pub presets: Vec<AdjustPreset>,
}

impl Default for PresetLibrary {
    fn default() -> Self {
        Self { version: Self::VERSION, presets: Vec::new() }
    }
}

impl PresetLibrary {
    pub const VERSION: u32 = 1;

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("presets serialise")
    }

    /// Reads a library, or a single preset object, from JSON.
    pub fn from_json(s: &str) -> Result<Self, PresetError> {
        let value: serde_json::Value = serde_json::from_str(s)?;
        if value.get("presets").is_some() {
            let lib: Self = serde_json::from_value(value)?;
            if lib.version > Self::VERSION {
                return Err(PresetError::UnsupportedVersion(lib.version));
            }
            Ok(lib)
        } else {
            let preset: AdjustPreset = serde_json::from_value(value)?;
            Ok(Self { version: Self::VERSION, presets: vec![preset] })
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), PresetError> {
        Ok(std::fs::write(path, self.to_json())?)
    }

    pub fn load(path: &Path) -> Result<Self, PresetError> {
        Self::from_json(&std::fs::read_to_string(path)?)
    }

    pub fn get(&self, name: &str) -> Option<&AdjustPreset> {
        self.presets.iter().find(|p| p.name == name)
    }

    /// Adds or replaces the preset with the same name.
    pub fn upsert(&mut self, preset: AdjustPreset) {
        match self.presets.iter_mut().find(|p| p.name == preset.name) {
            Some(p) => *p = preset,
            None => self.presets.push(preset),
        }
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.presets.len();
        self.presets.retain(|p| p.name != name);
        self.presets.len() != before
    }

    /// Imports presets from another library. Name clashes are kept as "name (2)", "name (3)"…
    /// Returns the names given to the imported presets.
    pub fn import(&mut self, other: PresetLibrary) -> Vec<String> {
        let mut names = Vec::new();
        for mut p in other.presets {
            if self.get(&p.name).is_some() {
                let base = p.name.clone();
                let mut n = 2;
                while self.get(&format!("{base} ({n})")).is_some() {
                    n += 1;
                }
                p.name = format!("{base} ({n})");
            }
            names.push(p.name.clone());
            self.presets.push(p);
        }
        names
    }

    /// A library holding only the named presets, for export.
    pub fn export(&self, names: &[&str]) -> PresetLibrary {
        PresetLibrary { version: Self::VERSION, presets: self.presets.iter().filter(|p| names.contains(&p.name.as_str())).cloned().collect() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preset(name: &str) -> AdjustPreset {
        AdjustPreset {
            name: name.into(),
            adjustments: Adjustments { contrast: 30.0, ..Default::default() },
            mode: ImageMode::Halftone { cells_per_inch: 60.0, angle_deg: 45.0 },
            dpi: 300.0,
            pass_through: false,
        }
    }

    #[test]
    fn save_load_round_trip() {
        let mut lib = PresetLibrary::default();
        lib.upsert(preset("Photo"));
        lib.upsert(preset("Slate"));
        let dir = std::env::temp_dir().join(format!("darkburn-raster-preset-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("presets.json");
        lib.save(&file).unwrap();
        assert_eq!(PresetLibrary::load(&file).unwrap(), lib);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn import_renames_clashes_and_export_filters() {
        let mut lib = PresetLibrary::default();
        lib.upsert(preset("Photo"));
        let mut other = PresetLibrary::default();
        other.upsert(preset("Photo"));
        other.upsert(preset("Wood"));
        let names = lib.import(other.clone());
        assert_eq!(names, vec!["Photo (2)", "Wood"]);
        assert_eq!(lib.import(other.export(&["Photo"])), vec!["Photo (3)"]);
        let out = lib.export(&["Wood"]);
        assert_eq!(out.presets.len(), 1);
        assert!(lib.remove("Wood"));
        assert!(lib.get("Wood").is_none());
    }

    #[test]
    fn single_preset_and_partial_json() {
        let lib = PresetLibrary::from_json(r#"{"name":"Quick","dpi":200}"#).unwrap();
        assert_eq!(lib.presets[0].name, "Quick");
        assert_eq!(lib.presets[0].adjustments, Adjustments::default());
        assert!(matches!(PresetLibrary::from_json(r#"{"version":99,"presets":[]}"#), Err(PresetError::UnsupportedVersion(99))));
        assert!(matches!(PresetLibrary::from_json("nope"), Err(PresetError::Json(_))));
    }
}
