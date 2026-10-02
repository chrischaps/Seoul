//! Command-line flags.

use anyhow::{Result, anyhow, bail};

pub const USAGE: &str = "\
seoul — a MilkDrop-inspired audio visualizer

USAGE:
    seoul [OPTIONS]

OPTIONS:
    --config <PATH>         Settings file (default: seoul.toml)
    --synth                 Use the built-in test track instead of system audio
    --preset <NAME>         Start on this preset
    --auto <SECS>           Auto-advance every SECS seconds (on a beat when possible)
    --transition <STYLE>    crossfade | dissolve | radial | clock | zoom | random
    --render-scale <F>      Feedback resolution relative to the window (0.25–2.0)
    --size <WxH>            Initial window size in physical pixels
    --fullscreen            Start borderless fullscreen
    --screenshot-at <SECS>  Save a screenshot after SECS seconds
    --tour <SECS>           Visit every preset for SECS seconds, screenshot each
                            into screenshots/tour/, then exit. With --preset,
                            only presets whose name contains it.
    --tour-shots <N>        Screenshots per preset during a tour (default 1)
    -h, --help              Show this help
";

#[derive(Debug, Default, Clone)]
pub struct Args {
    pub config: Option<String>,
    pub synth: bool,
    pub preset: Option<String>,
    pub auto: Option<f32>,
    pub transition: Option<String>,
    pub render_scale: Option<f32>,
    pub size: Option<(u32, u32)>,
    pub fullscreen: bool,
    pub screenshot_at: Option<f32>,
    pub tour: Option<f32>,
    pub tour_shots: Option<u32>,
}

impl Args {
    pub fn parse() -> Result<Option<Self>> {
        Self::parse_from(std::env::args().skip(1))
    }

    /// `Ok(None)` means help was printed and the program should exit.
    pub fn parse_from(it: impl IntoIterator<Item = String>) -> Result<Option<Self>> {
        let mut args = Args::default();
        let mut it = it.into_iter();
        while let Some(flag) = it.next() {
            let mut value = |name: &str| it.next().ok_or_else(|| anyhow!("{name} needs a value"));
            match flag.as_str() {
                "--synth" => args.synth = true,
                "--fullscreen" => args.fullscreen = true,
                "--preset" => args.preset = Some(value("--preset")?),
                "--config" => args.config = Some(value("--config")?),
                "--auto" => args.auto = Some(num(&value("--auto")?)?),
                "--transition" => args.transition = Some(value("--transition")?),
                "--render-scale" => args.render_scale = Some(num(&value("--render-scale")?)?),
                "--screenshot-at" => args.screenshot_at = Some(num(&value("--screenshot-at")?)?),
                "--tour" => args.tour = Some(num(&value("--tour")?)?),
                "--tour-shots" => args.tour_shots = Some(value("--tour-shots")?.parse()?),
                "--size" => {
                    let v = value("--size")?;
                    let (w, h) = v
                        .split_once(['x', 'X'])
                        .ok_or_else(|| anyhow!("--size expects WxH, got '{v}'"))?;
                    args.size = Some((w.trim().parse()?, h.trim().parse()?));
                }
                "-h" | "--help" => {
                    print!("{USAGE}");
                    return Ok(None);
                }
                other => bail!("unknown option '{other}'\n\n{USAGE}"),
            }
        }
        Ok(Some(args))
    }
}

fn num(s: &str) -> Result<f32> {
    s.parse::<f32>().map_err(|_| anyhow!("expected a number, got '{s}'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<Option<Args>> {
        Args::parse_from(s.split_whitespace().map(String::from))
    }

    #[test]
    fn parses_flags() {
        let a = parse("--synth --preset Vortex --tour 4 --size 1280x720 --render-scale 0.5 --auto 8 --transition clock")
            .unwrap()
            .unwrap();
        assert_eq!(a.auto, Some(8.0));
        assert_eq!(a.transition.as_deref(), Some("clock"));
        assert!(a.synth);
        assert_eq!(a.preset.as_deref(), Some("Vortex"));
        assert_eq!(a.tour, Some(4.0));
        assert_eq!(a.size, Some((1280, 720)));
        assert_eq!(a.render_scale, Some(0.5));
    }

    #[test]
    fn rejects_unknown_and_missing_values() {
        assert!(parse("--bogus").is_err());
        assert!(parse("--preset").is_err());
        assert!(parse("--size 12").is_err());
    }

    #[test]
    fn help_short_circuits() {
        assert!(parse("--help").unwrap().is_none());
    }
}
