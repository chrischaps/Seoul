//! Preset definition: TOML descriptor + parsed expressions + palette.
//!
//! `PresetSpec` is the loaded-and-parsed-but-not-yet-on-GPU form. It owns
//! the parsed warp mappings as `Expr` trees and the path to the composite
//! shader. The `Preset` (in library.rs) wraps a spec with the compiled
//! GPU resources.
//!
//! Unknown keys are rejected so a typo like `bloon = 1.0` fails loudly on
//! hot-reload instead of silently doing nothing.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use bytemuck::{Pod, Zeroable};
use serde::Deserialize;

use crate::preset::expr::{self, EvalContext, Expr};
use crate::render::post::PostParams;
use crate::render::warp::{EdgeMode, WarpParams};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPreset {
    name: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    description: String,
    /// Feedback edge policy: "mirror" (default), "fade" or "clamp".
    #[serde(default)]
    edge: Option<String>,
    mapping: RawMapping,
    shader: RawShader,
    #[serde(default)]
    palette: Option<RawPalette>,
    #[serde(default)]
    post: RawPost,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMapping {
    zoom: String,
    rotation: String,
    warp_amount: String,
    decay: String,
    cx: Option<String>,
    cy: Option<String>,
    dx: Option<String>,
    dy: Option<String>,
    sx: Option<String>,
    sy: Option<String>,
    warp_scale: Option<String>,
    warp_speed: Option<String>,
    hue_shift: Option<String>,
    blur: Option<String>,
    sharpen: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawShader {
    composite: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPalette {
    color0: [f32; 4],
    color1: [f32; 4],
    color2: [f32; 4],
    color3: [f32; 4],
}

/// Per-preset overrides of the global post defaults.
#[derive(Debug, Default, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawPost {
    pub exposure: Option<f32>,
    pub bloom: Option<f32>,
    pub bloom_threshold: Option<f32>,
    pub chroma: Option<f32>,
    pub vignette: Option<f32>,
    pub grain: Option<f32>,
    pub led_mask: Option<bool>,
    pub led_pitch: Option<f32>,
    pub saturation: Option<f32>,
    pub contrast: Option<f32>,
}

impl RawPost {
    pub fn apply(&self, base: &PostParams) -> PostParams {
        PostParams {
            exposure: self.exposure.unwrap_or(base.exposure),
            bloom: self.bloom.unwrap_or(base.bloom),
            bloom_threshold: self.bloom_threshold.unwrap_or(base.bloom_threshold),
            chroma: self.chroma.unwrap_or(base.chroma),
            vignette: self.vignette.unwrap_or(base.vignette),
            grain: self.grain.unwrap_or(base.grain),
            led_mask: self.led_mask.map_or(base.led_mask, |b| if b { 1.0 } else { 0.0 }),
            led_pitch: self.led_pitch.unwrap_or(base.led_pitch),
            saturation: self.saturation.unwrap_or(base.saturation),
            contrast: self.contrast.unwrap_or(base.contrast),
        }
    }
}

#[derive(Debug)]
pub struct PresetSpec {
    pub name: String,
    pub author: String,
    pub description: String,
    pub source_path: PathBuf,
    pub composite_path: PathBuf,
    pub mapping: WarpMapping,
    pub edge: EdgeMode,
    pub palette: Palette,
    pub post: RawPost,
}

#[derive(Debug)]
pub struct WarpMapping {
    pub zoom: Expr,
    pub rotation: Expr,
    pub warp_amount: Expr,
    pub decay: Expr,
    pub cx: Expr,
    pub cy: Expr,
    pub dx: Expr,
    pub dy: Expr,
    pub sx: Expr,
    pub sy: Expr,
    pub warp_scale: Expr,
    pub warp_speed: Expr,
    pub hue_shift: Expr,
    pub blur: Expr,
    pub sharpen: Expr,
}

impl WarpMapping {
    pub fn eval(&self, ctx: &EvalContext, edge: EdgeMode) -> WarpParams {
        WarpParams {
            zoom: self.zoom.eval(ctx),
            rotation: self.rotation.eval(ctx),
            warp_amount: self.warp_amount.eval(ctx),
            decay: self.decay.eval(ctx),
            cx: self.cx.eval(ctx),
            cy: self.cy.eval(ctx),
            dx: self.dx.eval(ctx),
            dy: self.dy.eval(ctx),
            sx: self.sx.eval(ctx),
            sy: self.sy.eval(ctx),
            warp_scale: self.warp_scale.eval(ctx),
            warp_speed: self.warp_speed.eval(ctx),
            hue_shift: self.hue_shift.eval(ctx),
            blur: self.blur.eval(ctx),
            sharpen: self.sharpen.eval(ctx),
            edge,
        }
    }
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
        Self::parse(&text, toml_path)
    }

    pub fn parse(text: &str, toml_path: &Path) -> Result<Self> {
        let raw: RawPreset = toml::from_str(text)
            .with_context(|| format!("parse TOML in {}", toml_path.display()))?;

        let parse = |name: &str, src: &str| -> Result<Expr> {
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
        let m = &raw.mapping;
        let opt = |name: &str, src: &Option<String>, default: &str| -> Result<Expr> {
            parse(name, src.as_deref().unwrap_or(default))
        };

        let mapping = WarpMapping {
            zoom: parse("zoom", &m.zoom)?,
            rotation: parse("rotation", &m.rotation)?,
            warp_amount: parse("warp_amount", &m.warp_amount)?,
            decay: parse("decay", &m.decay)?,
            cx: opt("cx", &m.cx, "0.5")?,
            cy: opt("cy", &m.cy, "0.5")?,
            dx: opt("dx", &m.dx, "0")?,
            dy: opt("dy", &m.dy, "0")?,
            sx: opt("sx", &m.sx, "1")?,
            sy: opt("sy", &m.sy, "1")?,
            warp_scale: opt("warp_scale", &m.warp_scale, "1")?,
            warp_speed: opt("warp_speed", &m.warp_speed, "1")?,
            hue_shift: opt("hue_shift", &m.hue_shift, "0")?,
            blur: opt("blur", &m.blur, "0")?,
            sharpen: opt("sharpen", &m.sharpen, "0")?,
        };

        let edge = match raw.edge.as_deref() {
            None => EdgeMode::default(),
            Some(s) => EdgeMode::parse(s).ok_or_else(|| {
                anyhow!("preset '{}': unknown edge mode '{s}' (mirror|fade|clamp)", raw.name)
            })?,
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
            edge,
            palette,
            post: raw.post,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::AudioFeatures;

    const MIN: &str = r#"
name = "Test"
[mapping]
zoom = "1.0 + bass * 0.05"
rotation = "0.01"
warp_amount = "0.02"
decay = "0.97"
[shader]
composite = "test.wgsl"
"#;

    fn parse(src: &str) -> Result<PresetSpec> {
        PresetSpec::parse(src, Path::new("presets/test.toml"))
    }

    #[test]
    fn loads_minimal_preset() {
        let spec = parse(MIN).unwrap();
        assert_eq!(spec.name, "Test");
        assert_eq!(spec.composite_path, Path::new("presets").join("test.wgsl"));
        assert_eq!(spec.edge, EdgeMode::Mirror);
    }

    #[test]
    fn optional_mapping_fields_default_to_identity() {
        let spec = parse(MIN).unwrap();
        let f = AudioFeatures::default();
        let w = spec.mapping.eval(&EvalContext::new(&f), spec.edge);
        assert_eq!((w.cx, w.cy, w.dx, w.dy, w.sx, w.sy), (0.5, 0.5, 0.0, 0.0, 1.0, 1.0));
        assert_eq!((w.warp_scale, w.warp_speed, w.hue_shift, w.blur, w.sharpen), (1.0, 1.0, 0.0, 0.0, 0.0));
    }

    #[test]
    fn loads_with_palette() {
        let spec = parse(&format!(
            "{MIN}
[palette]
color0 = [0.1, 0.2, 0.3, 1.0]
color1 = [0.4, 0.5, 0.6, 1.0]
color2 = [0.7, 0.8, 0.9, 1.0]
color3 = [1.0, 1.0, 1.0, 1.0]
"
        ))
        .unwrap();
        assert_eq!(spec.palette.colors[0], [0.1, 0.2, 0.3, 1.0]);
        assert_eq!(spec.palette.colors[3], [1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn post_overrides_apply_over_defaults() {
        let spec = parse(&format!("{MIN}\n[post]\nbloom = 1.5\nled_mask = true\n")).unwrap();
        let base = PostParams::default();
        let p = spec.post.apply(&base);
        assert_eq!(p.bloom, 1.5);
        assert_eq!(p.led_mask, 1.0);
        assert_eq!(p.vignette, base.vignette);
    }

    #[test]
    fn edge_mode_parses() {
        let spec = parse(&format!("edge = \"fade\"\n{MIN}")).unwrap();
        assert_eq!(spec.edge, EdgeMode::Fade);
        assert!(parse(&format!("edge = \"wobble\"\n{MIN}")).is_err());
    }

    #[test]
    fn rejects_bad_expression() {
        assert!(parse(&MIN.replace("1.0 + bass * 0.05", "1.0 + nonsense * 0.05")).is_err());
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(parse(&format!("{MIN}\n[post]\nbloon = 1.0\n")).is_err());
        assert!(parse(&MIN.replace("decay = \"0.97\"", "decay = \"0.97\"\nzooom = \"1\"")).is_err());
    }
}
