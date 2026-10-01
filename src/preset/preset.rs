//! Preset definition: TOML descriptor + parsed expressions + palette.
//!
//! `PresetSpec` is the loaded-and-parsed-but-not-yet-on-GPU form. It owns
//! the parsed warp mappings as `Expr` trees and the paths to the shaders.
//! The `Preset` (in library.rs) wraps a spec with the compiled GPU
//! resources.
//!
//! Unknown keys are rejected so a typo like `bloon = 1.0` fails loudly on
//! hot-reload instead of silently doing nothing.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use bytemuck::{Pod, Zeroable};
use serde::Deserialize;

use crate::preset::expr::{self, EvalContext, Expr};
use crate::render::particles::{ColorMode, ParticleParams, SpawnShape};
use crate::render::post::{EchoOrient, Mirror, PostParams};
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
    #[serde(default)]
    particles: Option<RawParticles>,
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
    /// Optional custom warp shader (defines `fs_warp`).
    warp: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPalette {
    color0: [f32; 4],
    color1: [f32; 4],
    color2: [f32; 4],
    color3: [f32; 4],
}

/// A `[post]` value: a plain number or an expression string evaluated
/// every frame (e.g. `chroma = "beat * 0.8"`).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum NumOrExpr {
    Num(f64),
    Expr(String),
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPost {
    exposure: Option<NumOrExpr>,
    bloom: Option<NumOrExpr>,
    bloom_threshold: Option<NumOrExpr>,
    chroma: Option<NumOrExpr>,
    vignette: Option<NumOrExpr>,
    grain: Option<NumOrExpr>,
    led_mask: Option<bool>,
    led_pitch: Option<NumOrExpr>,
    saturation: Option<NumOrExpr>,
    contrast: Option<NumOrExpr>,
    echo_alpha: Option<NumOrExpr>,
    echo_zoom: Option<NumOrExpr>,
    echo_orient: Option<String>,
    mirror: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawParticles {
    count: Option<u32>,
    spawn: Option<String>,
    speed: Option<f32>,
    flow: Option<f32>,
    flow_scale: Option<f32>,
    drag: Option<f32>,
    size: Option<f32>,
    life: Option<f32>,
    /// Palette index 0–3, "ramp" or "spectrum".
    color: Option<RawColor>,
    burst: Option<f32>,
    bass_push: Option<f32>,
    gravity: Option<f32>,
    intensity: Option<f32>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawColor {
    Index(u32),
    Name(String),
}

/// Per-preset overrides of the global post defaults, evaluated per frame.
#[derive(Debug, Default)]
pub struct PostMapping {
    exposure: Option<Expr>,
    bloom: Option<Expr>,
    bloom_threshold: Option<Expr>,
    chroma: Option<Expr>,
    vignette: Option<Expr>,
    grain: Option<Expr>,
    led_mask: Option<bool>,
    led_pitch: Option<Expr>,
    saturation: Option<Expr>,
    contrast: Option<Expr>,
    echo_alpha: Option<Expr>,
    echo_zoom: Option<Expr>,
    echo_orient: Option<EchoOrient>,
    mirror: Option<Mirror>,
}

impl PostMapping {
    pub fn apply(&self, base: &PostParams, ctx: &EvalContext) -> PostParams {
        let v = |e: &Option<Expr>, d: f32| e.as_ref().map_or(d, |e| e.eval(ctx));
        PostParams {
            exposure: v(&self.exposure, base.exposure),
            bloom: v(&self.bloom, base.bloom),
            bloom_threshold: v(&self.bloom_threshold, base.bloom_threshold),
            chroma: v(&self.chroma, base.chroma),
            vignette: v(&self.vignette, base.vignette),
            grain: v(&self.grain, base.grain),
            led_mask: self.led_mask.map_or(base.led_mask, |b| if b { 1.0 } else { 0.0 }),
            led_pitch: v(&self.led_pitch, base.led_pitch),
            saturation: v(&self.saturation, base.saturation),
            contrast: v(&self.contrast, base.contrast),
            echo_alpha: v(&self.echo_alpha, base.echo_alpha),
            echo_zoom: v(&self.echo_zoom, base.echo_zoom),
            echo_orient: self.echo_orient.unwrap_or(base.echo_orient),
            mirror: self.mirror.unwrap_or(base.mirror),
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
    pub warp_path: Option<PathBuf>,
    pub mapping: WarpMapping,
    pub edge: EdgeMode,
    pub palette: Palette,
    pub post: PostMapping,
    pub particles: Option<ParticleParams>,
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
        let name = raw.name.clone();

        let parse = |field: &str, src: &str| -> Result<Expr> {
            expr::parse(src).map_err(|e| {
                anyhow!("preset '{name}' field '{field}' expression error: {e} — source: '{src}'")
            })
        };
        let m = &raw.mapping;
        let opt = |field: &str, src: &Option<String>, default: &str| -> Result<Expr> {
            parse(field, src.as_deref().unwrap_or(default))
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
            Some(s) => EdgeMode::parse(s)
                .ok_or_else(|| anyhow!("preset '{name}': unknown edge mode '{s}' (mirror|fade|clamp)"))?,
        };

        let post_expr = |field: &str, v: &Option<NumOrExpr>| -> Result<Option<Expr>> {
            Ok(match v {
                None => None,
                Some(NumOrExpr::Num(n)) => Some(Expr::Lit(*n as f32)),
                Some(NumOrExpr::Expr(s)) => Some(parse(&format!("post.{field}"), s)?),
            })
        };
        let rp = &raw.post;
        let post = PostMapping {
            exposure: post_expr("exposure", &rp.exposure)?,
            bloom: post_expr("bloom", &rp.bloom)?,
            bloom_threshold: post_expr("bloom_threshold", &rp.bloom_threshold)?,
            chroma: post_expr("chroma", &rp.chroma)?,
            vignette: post_expr("vignette", &rp.vignette)?,
            grain: post_expr("grain", &rp.grain)?,
            led_mask: rp.led_mask,
            led_pitch: post_expr("led_pitch", &rp.led_pitch)?,
            saturation: post_expr("saturation", &rp.saturation)?,
            contrast: post_expr("contrast", &rp.contrast)?,
            echo_alpha: post_expr("echo_alpha", &rp.echo_alpha)?,
            echo_zoom: post_expr("echo_zoom", &rp.echo_zoom)?,
            echo_orient: rp
                .echo_orient
                .as_deref()
                .map(|s| {
                    EchoOrient::parse(s)
                        .ok_or_else(|| anyhow!("preset '{name}': echo_orient '{s}' (none|x|y|xy)"))
                })
                .transpose()?,
            mirror: rp
                .mirror
                .as_deref()
                .map(|s| {
                    Mirror::parse(s).ok_or_else(|| {
                        anyhow!("preset '{name}': mirror '{s}' (none|x|y|quad|kaleido|kaleido:N)")
                    })
                })
                .transpose()?,
        };

        let particles = raw.particles.as_ref().map(|rp| parse_particles(&name, rp)).transpose()?;

        let palette = match raw.palette {
            Some(p) => Palette {
                colors: [p.color0, p.color1, p.color2, p.color3],
            },
            None => Palette::neutral(),
        };

        // Shader paths are relative to the preset TOML's directory.
        let preset_dir = toml_path.parent().unwrap_or_else(|| Path::new("."));
        let composite_path = preset_dir.join(&raw.shader.composite);
        let warp_path = raw.shader.warp.as_ref().map(|w| preset_dir.join(w));

        Ok(PresetSpec {
            name: raw.name,
            author: raw.author,
            description: raw.description,
            source_path: toml_path.to_path_buf(),
            composite_path,
            warp_path,
            mapping,
            edge,
            palette,
            post,
            particles,
        })
    }
}

fn parse_particles(name: &str, rp: &RawParticles) -> Result<ParticleParams> {
    let d = ParticleParams::default();
    let spawn = match rp.spawn.as_deref() {
        None => d.spawn,
        Some(s) => SpawnShape::parse(s).ok_or_else(|| {
            anyhow!("preset '{name}': particle spawn '{s}' (center|ring|edges|waveform|random)")
        })?,
    };
    let color = match &rp.color {
        None => d.color,
        Some(RawColor::Index(i)) if *i < 4 => ColorMode::Palette(*i),
        Some(RawColor::Index(i)) => return Err(anyhow!("preset '{name}': particle color index {i} (0–3)")),
        Some(RawColor::Name(s)) => match s.to_ascii_lowercase().as_str() {
            "ramp" => ColorMode::Ramp,
            "spectrum" => ColorMode::Spectrum,
            _ => return Err(anyhow!("preset '{name}': particle color '{s}' (0–3|ramp|spectrum)")),
        },
    };
    Ok(ParticleParams {
        count: rp.count.unwrap_or(d.count),
        spawn,
        speed: rp.speed.unwrap_or(d.speed),
        flow: rp.flow.unwrap_or(d.flow),
        flow_scale: rp.flow_scale.unwrap_or(d.flow_scale),
        drag: rp.drag.unwrap_or(d.drag),
        size: rp.size.unwrap_or(d.size),
        life: rp.life.unwrap_or(d.life),
        color,
        burst: rp.burst.unwrap_or(d.burst),
        bass_push: rp.bass_push.unwrap_or(d.bass_push),
        gravity: rp.gravity.unwrap_or(d.gravity),
        intensity: rp.intensity.unwrap_or(d.intensity),
    })
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

    fn eval_post(spec: &PresetSpec, f: &AudioFeatures) -> PostParams {
        spec.post.apply(&PostParams::default(), &EvalContext::new(f))
    }

    #[test]
    fn loads_minimal_preset() {
        let spec = parse(MIN).unwrap();
        assert_eq!(spec.name, "Test");
        assert_eq!(spec.composite_path, Path::new("presets").join("test.wgsl"));
        assert_eq!(spec.edge, EdgeMode::Mirror);
        assert!(spec.warp_path.is_none());
        assert!(spec.particles.is_none());
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
        let p = eval_post(&spec, &AudioFeatures::default());
        assert_eq!(p.bloom, 1.5);
        assert_eq!(p.led_mask, 1.0);
        assert_eq!(p.vignette, base.vignette);
    }

    #[test]
    fn post_values_can_be_audio_reactive_expressions() {
        let spec = parse(&format!(
            "{MIN}\n[post]\nchroma = \"beat * 0.8\"\necho_alpha = 0.4\nmirror = \"kaleido:5\"\necho_orient = \"x\"\n"
        ))
        .unwrap();
        let f = AudioFeatures {
            beat: 0.5,
            ..Default::default()
        };
        let p = eval_post(&spec, &f);
        assert!((p.chroma - 0.4).abs() < 1e-6);
        assert!((p.echo_alpha - 0.4).abs() < 1e-6);
        assert_eq!(p.mirror, Mirror::Kaleido(5));
        assert_eq!(p.echo_orient, EchoOrient::FlipX);
        assert!(parse(&format!("{MIN}\n[post]\nchroma = \"beet * 2\"\n")).is_err());
        assert!(parse(&format!("{MIN}\n[post]\nmirror = \"spiral\"\n")).is_err());
    }

    #[test]
    fn custom_warp_path_is_relative_to_toml() {
        let spec = parse(&MIN.replace("composite = \"test.wgsl\"", "composite = \"test.wgsl\"\nwarp = \"test_warp.wgsl\""))
            .unwrap();
        assert_eq!(spec.warp_path, Some(Path::new("presets").join("test_warp.wgsl")));
    }

    #[test]
    fn particles_table_parses_with_defaults() {
        let spec = parse(&format!(
            "{MIN}\n[particles]\ncount = 5000\nspawn = \"ring\"\ncolor = \"spectrum\"\nburst = 0.5\n"
        ))
        .unwrap();
        let p = spec.particles.unwrap();
        assert_eq!(p.count, 5000);
        assert_eq!(p.spawn, SpawnShape::Ring);
        assert_eq!(p.color, ColorMode::Spectrum);
        assert_eq!(p.burst, 0.5);
        assert_eq!(p.life, ParticleParams::default().life);

        let idx = parse(&format!("{MIN}\n[particles]\ncolor = 2\n")).unwrap();
        assert_eq!(idx.particles.unwrap().color, ColorMode::Palette(2));
        assert!(parse(&format!("{MIN}\n[particles]\ncolor = 7\n")).is_err());
        assert!(parse(&format!("{MIN}\n[particles]\nspawn = \"donut\"\n")).is_err());
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
        assert!(parse(&format!("{MIN}\n[particles]\ncuont = 5\n")).is_err());
    }
}
