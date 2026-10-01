//! Preset definition: TOML descriptor + parsed expressions + palette.
//!
//! `PresetSpec` is the loaded-and-parsed-but-not-yet-on-GPU form. It owns
//! the parsed warp mappings as `Expr` trees and the path to the composite
//! shader. The `Preset` (in library.rs) wraps a spec with the compiled
//! GPU resources.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use bytemuck::{Pod, Zeroable};
use serde::Deserialize;

use crate::preset::expr::{self, Expr};

#[derive(Debug, Deserialize)]
struct RawPreset {
    name: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    description: String,
    mapping: RawMapping,
    shader: RawShader,
    #[serde(default)]
    palette: Option<RawPalette>,
}

#[derive(Debug, Deserialize)]
struct RawMapping {
    zoom: String,
    rotation: String,
    warp_amount: String,
    decay: String,
}

#[derive(Debug, Deserialize)]
struct RawShader {
    composite: String,
}

#[derive(Debug, Deserialize)]
struct RawPalette {
    color0: [f32; 4],
    color1: [f32; 4],
    color2: [f32; 4],
    color3: [f32; 4],
}

#[derive(Debug)]
pub struct PresetSpec {
    pub name: String,
    #[allow(dead_code)]
    pub author: String,
    #[allow(dead_code)]
    pub description: String,
    pub source_path: PathBuf,
    pub composite_path: PathBuf,
    pub mapping: WarpMapping,
    pub palette: Palette,
}

#[derive(Debug)]
pub struct WarpMapping {
    pub zoom: Expr,
    pub rotation: Expr,
    pub warp_amount: Expr,
    pub decay: Expr,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct Palette {
    pub colors: [[f32; 4]; 4],
}

impl Palette {
    pub fn neutral() -> Self {
        // Soft monochrome fallback when no palette is declared
        Self {
            colors: [
                [0.10, 0.10, 0.12, 1.0],
                [0.45, 0.50, 0.60, 1.0],
                [0.80, 0.80, 0.85, 1.0],
                [1.00, 1.00, 1.00, 1.0],
            ],
        }
    }
}

impl PresetSpec {
    pub fn load(toml_path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(toml_path)
            .with_context(|| format!("read preset {}", toml_path.display()))?;
        let raw: RawPreset = toml::from_str(&text)
            .with_context(|| format!("parse TOML in {}", toml_path.display()))?;

        let parse_expr_field = |name: &str, src: &str| -> Result<Expr> {
            expr::parse(src).map_err(|e| {
                anyhow!(
                    "preset '{}' field '{}' expression error: {} — source: '{}'",
                    raw.name,
                    name,
                    e,
                    src
                )
            })
        };

        let mapping = WarpMapping {
            zoom: parse_expr_field("zoom", &raw.mapping.zoom)?,
            rotation: parse_expr_field("rotation", &raw.mapping.rotation)?,
            warp_amount: parse_expr_field("warp_amount", &raw.mapping.warp_amount)?,
            decay: parse_expr_field("decay", &raw.mapping.decay)?,
        };

        let palette = match raw.palette {
            Some(p) => Palette {
                colors: [p.color0, p.color1, p.color2, p.color3],
            },
            None => Palette::neutral(),
        };

        // Composite shader path is relative to the preset TOML's directory.
        let preset_dir = toml_path.parent().unwrap_or_else(|| Path::new("."));
        let composite_path = preset_dir.join(&raw.shader.composite);

        Ok(PresetSpec {
            name: raw.name,
            author: raw.author,
            description: raw.description,
            source_path: toml_path.to_path_buf(),
            composite_path,
            mapping,
            palette,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn loads_minimal_preset() {
        let dir = std::env::temp_dir().join("seoul-preset-test-min");
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("min.toml");
        let mut f = std::fs::File::create(&p).unwrap();
        write!(
            f,
            r#"
name = "Test"
[mapping]
zoom = "1.0 + bass * 0.05"
rotation = "0.01"
warp_amount = "0.02"
decay = "0.97"
[shader]
composite = "test.wgsl"
"#
        )
        .unwrap();
        drop(f);

        let spec = PresetSpec::load(&p).unwrap();
        assert_eq!(spec.name, "Test");
        assert_eq!(spec.composite_path, dir.join("test.wgsl"));
    }

    #[test]
    fn loads_with_palette() {
        let dir = std::env::temp_dir().join("seoul-preset-test-pal");
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("pal.toml");
        let mut f = std::fs::File::create(&p).unwrap();
        write!(
            f,
            r#"
name = "Pal"
[mapping]
zoom = "1.0"
rotation = "0.0"
warp_amount = "0.0"
decay = "0.96"
[shader]
composite = "x.wgsl"
[palette]
color0 = [0.1, 0.2, 0.3, 1.0]
color1 = [0.4, 0.5, 0.6, 1.0]
color2 = [0.7, 0.8, 0.9, 1.0]
color3 = [1.0, 1.0, 1.0, 1.0]
"#
        )
        .unwrap();
        drop(f);

        let spec = PresetSpec::load(&p).unwrap();
        assert_eq!(spec.palette.colors[0], [0.1, 0.2, 0.3, 1.0]);
        assert_eq!(spec.palette.colors[3], [1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn rejects_bad_expression() {
        let dir = std::env::temp_dir().join("seoul-preset-test-bad");
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("bad.toml");
        let mut f = std::fs::File::create(&p).unwrap();
        write!(
            f,
            r#"
name = "Bad"
[mapping]
zoom = "1.0 + nonsense * 0.05"
rotation = "0.0"
warp_amount = "0.0"
decay = "0.97"
[shader]
composite = "x.wgsl"
"#
        )
        .unwrap();
        drop(f);

        assert!(PresetSpec::load(&p).is_err());
    }
}
