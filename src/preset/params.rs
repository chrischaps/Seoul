//! Preset `[params]`: named values computed on the CPU every frame and
//! handed to the preset's shaders.
//!
//! ```toml
//! [params]
//! cx    = "-0.745 + 0.02 * sin(time * 0.11)"             # plain expression
//! cy    = { follow = "0.11 + beat * 0.04", tau = 0.35 }   # eases toward the target
//! depth = { rate = "0.08 + bass_att * 0.25", wrap = 1.0 } # integrates per second
//! ```
//!
//! Params are evaluated in file order, so a param may use any declared
//! before it; `[mapping]` and `[post]` may use all of them. `follow` and
//! `rate` are per second, which makes them frame-rate independent by
//! construction. Shaders read them through a generated accessor
//! (`let q = params(); q.cx`), backed by the preset's uniform buffer
//! (`palette.params` in WGSL).

use anyhow::{Result, anyhow};

use crate::audio::AudioFeatures;
use crate::preset::expr::{self, EvalContext, Expr};

/// Capacity of `palette.params` (4 × vec4).
pub const MAX_PARAMS: usize = 16;

/// Longest step `follow`/`rate` integrate over, so a stall (window drag,
/// breakpoint) doesn't fling an integrator.
const MAX_STEP: f32 = 0.1;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamKind {
    /// The expression's value, every frame.
    Expr,
    /// Exponential approach to the expression's value, time constant `tau` s.
    Follow { tau: f32 },
    /// Integral of the expression over time (units per second), wrapped
    /// into `0..wrap` when given — for seamless loops.
    Rate { wrap: Option<f32> },
}

#[derive(Debug)]
pub struct ParamSpec {
    pub name: String,
    pub expr: Expr,
    pub kind: ParamKind,
}

/// Parse the `[params]` table. `preset` names the preset in errors.
pub fn parse_params(preset: &str, table: &toml::Table) -> Result<Vec<ParamSpec>> {
    if table.len() > MAX_PARAMS {
        return Err(anyhow!("preset '{preset}': at most {MAX_PARAMS} params, found {}", table.len()));
    }
    let mut specs: Vec<ParamSpec> = Vec::with_capacity(table.len());
    for (name, value) in table {
        check_name(preset, name)?;
        let scope: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        let parse = |src: &str| {
            expr::parse_with(src, &scope)
                .map_err(|e| anyhow!("preset '{preset}' param '{name}' expression error: {e} — source: '{src}'"))
        };
        let number = |v: &toml::Value, key: &str| -> Result<f32> {
            match v {
                toml::Value::Float(f) => Ok(*f as f32),
                toml::Value::Integer(i) => Ok(*i as f32),
                _ => Err(anyhow!("preset '{preset}' param '{name}': '{key}' must be a number")),
            }
        };
        let source = |v: &toml::Value, key: &str| -> Result<Expr> {
            match v {
                toml::Value::String(s) => parse(s),
                v => Ok(Expr::Lit(number(v, key)?)),
            }
        };

        let (expr, kind) = match value {
            toml::Value::Table(t) => {
                for key in t.keys() {
                    if !matches!(key.as_str(), "follow" | "tau" | "rate" | "wrap") {
                        return Err(anyhow!(
                            "preset '{preset}' param '{name}': unknown key '{key}' (follow, tau | rate, wrap)"
                        ));
                    }
                }
                match (t.get("follow"), t.get("rate")) {
                    (Some(f), None) => {
                        if t.contains_key("wrap") {
                            return Err(anyhow!("preset '{preset}' param '{name}': 'wrap' goes with 'rate'"));
                        }
                        let tau = t.get("tau").map(|v| number(v, "tau")).transpose()?.unwrap_or(0.25);
                        if tau <= 0.0 {
                            return Err(anyhow!("preset '{preset}' param '{name}': tau must be > 0"));
                        }
                        (source(f, "follow")?, ParamKind::Follow { tau })
                    }
                    (None, Some(r)) => {
                        if t.contains_key("tau") {
                            return Err(anyhow!("preset '{preset}' param '{name}': 'tau' goes with 'follow'"));
                        }
                        let wrap = t.get("wrap").map(|v| number(v, "wrap")).transpose()?;
                        if wrap.is_some_and(|w| w <= 0.0) {
                            return Err(anyhow!("preset '{preset}' param '{name}': wrap must be > 0"));
                        }
                        (source(r, "rate")?, ParamKind::Rate { wrap })
                    }
                    _ => {
                        return Err(anyhow!(
                            "preset '{preset}' param '{name}': a table needs exactly one of 'follow' or 'rate'"
                        ));
                    }
                }
            }
            v => (source(v, "value")?, ParamKind::Expr),
        };
        specs.push(ParamSpec {
            name: name.clone(),
            expr,
            kind,
        });
    }
    Ok(specs)
}

/// A param name must be a plain identifier usable both in expressions and
/// as a WGSL struct field.
fn check_name(preset: &str, name: &str) -> Result<()> {
    let mut chars = name.chars();
    let ident = chars.next().is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && name != "_"
        && !name.starts_with("__");
    if !ident {
        return Err(anyhow!(
            "preset '{preset}': param name '{name}' must be lowercase letters, digits and '_'"
        ));
    }
    if expr::is_builtin(name) {
        return Err(anyhow!("preset '{preset}': param '{name}' shadows a built-in variable or function"));
    }
    if WGSL_RESERVED.contains(&name) {
        return Err(anyhow!("preset '{preset}': param name '{name}' is a reserved word in WGSL"));
    }
    Ok(())
}

/// Live values of one preset's params.
#[derive(Debug, Clone, Default)]
pub struct ParamState {
    values: Vec<f32>,
    primed: bool,
}

impl ParamState {
    pub fn new(count: usize) -> Self {
        Self {
            values: vec![0.0; count],
            primed: false,
        }
    }

    pub fn values(&self) -> &[f32] {
        &self.values
    }

    /// Advance one frame. The first tick jumps followers straight to their
    /// target (nothing eases in from zero); integrators start at 0.
    pub fn tick(&mut self, specs: &[ParamSpec], features: &AudioFeatures) {
        let dt = features.dt.clamp(0.0, MAX_STEP);
        for (i, spec) in specs.iter().enumerate() {
            let target = spec.expr.eval(&EvalContext::with_params(features, &self.values));
            let v = &mut self.values[i];
            *v = match spec.kind {
                ParamKind::Expr => target,
                ParamKind::Follow { .. } if !self.primed => target,
                ParamKind::Follow { tau } => *v + (target - *v) * (1.0 - (-dt / tau).exp()),
                ParamKind::Rate { .. } if !self.primed => 0.0,
                ParamKind::Rate { wrap } => {
                    let next = *v + target * dt;
                    match wrap {
                        Some(w) => next.rem_euclid(w),
                        None => next,
                    }
                }
            };
            if !v.is_finite() {
                *v = 0.0;
            }
        }
        self.primed = true;
    }

    /// Values packed for `palette.params` (4 × vec4).
    pub fn packed(&self) -> [[f32; 4]; 4] {
        let mut out = [[0.0; 4]; 4];
        for (i, v) in self.values.iter().take(MAX_PARAMS).enumerate() {
            out[i / 4][i % 4] = *v;
        }
        out
    }
}

/// WGSL appended to a preset's prelude so shaders can read params by name:
/// `struct Params { cx: f32, … }` and `fn params() -> Params`. Empty when
/// the preset declares none.
pub fn wgsl_accessor(specs: &[ParamSpec]) -> String {
    if specs.is_empty() {
        return String::new();
    }
    let fields: Vec<String> = specs.iter().map(|s| format!("{}: f32", s.name)).collect();
    let reads: Vec<String> = (0..specs.len())
        .map(|i| format!("palette.params[{}].{}", i / 4, ['x', 'y', 'z', 'w'][i % 4]))
        .collect();
    format!(
        "// Generated from this preset's [params].\nstruct Params {{ {} }}\nfn params() -> Params {{ return Params({}); }}\n",
        fields.join(", "),
        reads.join(", ")
    )
}

/// WGSL keywords and reserved words (WGSL spec §15) — none can be a field name.
const WGSL_RESERVED: &[&str] = &[
    "alias", "break", "case", "const", "const_assert", "continue", "continuing", "default", "diagnostic",
    "discard", "else", "enable", "false", "fn", "for", "if", "let", "loop", "override", "requires", "return",
    "struct", "switch", "true", "var", "while", "nullptr", "null", "self", "super", "this", "abstract", "active",
    "alignas", "alignof", "as", "asm", "asm_fragment", "async", "attribute", "auto", "await", "become",
    "cast", "catch", "class", "co_await", "co_return", "co_yield", "coherent", "column_major", "common",
    "compile", "compile_fragment", "concept", "const_cast", "consteval", "constexpr", "constinit", "crate",
    "debugger", "decltype", "delete", "demote", "demote_to_helper", "do", "dynamic_cast", "enum", "explicit",
    "export", "extends", "extern", "external", "fallthrough", "filter", "final", "finally", "friend", "from",
    "fxgroup", "get", "goto", "groupshared", "highp", "impl", "implements", "import", "inline", "instanceof",
    "interface", "layout", "lowp", "macro", "macro_rules", "match", "mediump", "meta", "mod", "module", "move",
    "mut", "mutable", "namespace", "new", "nil", "noexcept", "noinline", "nointerpolation", "non_coherent",
    "noncoherent", "noperspective", "null", "operator", "package", "packoffset", "partition", "pass", "patch",
    "pixelfragment", "precise", "precision", "premerge", "priv", "protected", "pub", "public", "readonly",
    "ref", "regardless", "register", "reinterpret_cast", "require", "resource", "restrict", "self", "set",
    "shared", "sizeof", "smooth", "snorm", "static", "static_assert", "static_cast", "std", "subroutine",
    "target", "template", "throw", "trait", "try", "type", "typedef", "typeid", "typename", "typeof", "union",
    "unless", "unorm", "unsafe", "unsized", "use", "using", "varying", "virtual", "volatile", "wgsl", "where",
    "with", "writeonly", "yield",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn table(src: &str) -> toml::Table {
        toml::from_str(src).unwrap()
    }

    fn specs(src: &str) -> Result<Vec<ParamSpec>> {
        parse_params("T", &table(src))
    }

    fn at(dt: f32, bass: f32) -> AudioFeatures {
        AudioFeatures {
            dt,
            bass,
            ..Default::default()
        }
    }

    #[test]
    fn keeps_declaration_order_and_scopes_earlier_params() {
        let s = specs("zeta = \"1\"\nalpha = \"zeta * 2\"\n").unwrap();
        assert_eq!(s[0].name, "zeta");
        assert_eq!(s[1].name, "alpha");
        let mut st = ParamState::new(2);
        st.tick(&s, &at(1.0 / 60.0, 0.0));
        assert_eq!(st.values(), &[1.0, 2.0]);
        // A param can't see one declared after it.
        assert!(specs("a = \"b\"\nb = \"1\"\n").is_err());
    }

    #[test]
    fn numbers_and_kinds_parse() {
        let s = specs("a = 0.5\nb = { follow = \"bass\", tau = 2 }\nc = { rate = 1, wrap = 3.0 }\n").unwrap();
        assert_eq!(s[0].kind, ParamKind::Expr);
        assert_eq!(s[1].kind, ParamKind::Follow { tau: 2.0 });
        assert_eq!(s[2].kind, ParamKind::Rate { wrap: Some(3.0) });
    }

    #[test]
    fn rejects_bad_tables_and_names() {
        assert!(specs("a = { follow = \"1\", rate = \"1\" }").is_err());
        assert!(specs("a = { tau = 1 }").is_err());
        assert!(specs("a = { follow = \"1\", wrap = 1 }").is_err());
        assert!(specs("a = { rate = \"1\", tua = 1 }").is_err());
        assert!(specs("a = { follow = \"1\", tau = 0 }").is_err());
        assert!(specs("bass = \"1\"").is_err());
        assert!(specs("sin = \"1\"").is_err());
        assert!(specs("cv1 = \"1\"").is_err());
        assert!(specs("gate2_age = \"1\"").is_err());
        assert!(specs("cv9 = \"1\"").is_ok());
        assert!(specs("loop = \"1\"").is_err());
        assert!(specs("Cx = \"1\"").is_err());
        assert!(specs("__x = \"1\"").is_err());
        assert!(specs("a = \"nope\"").is_err());
        let many: String = (0..17).map(|i| format!("p{i} = 0\n")).collect();
        assert!(specs(&many).is_err());
    }

    #[test]
    fn follower_starts_at_target_then_eases() {
        let s = specs("a = { follow = \"bass\", tau = 1.0 }").unwrap();
        let mut st = ParamState::new(1);
        st.tick(&s, &at(1.0 / 60.0, 0.4));
        assert_eq!(st.values()[0], 0.4);
        st.tick(&s, &at(0.1, 1.4));
        assert!((st.values()[0] - (0.4 + (1.0 - (-0.1f32).exp()))).abs() < 1e-5);
    }

    /// The same wall-clock second gives the same value at 60 and 144 Hz.
    #[test]
    fn follow_and_rate_are_frame_rate_independent() {
        let s = specs("f = { follow = \"bass\", tau = 0.3 }\nr = { rate = \"0.5 + bass\" }").unwrap();
        let run = |hz: f32| {
            let mut st = ParamState::new(2);
            st.tick(&s, &at(1.0 / hz, 0.0));
            for _ in 0..hz as usize {
                st.tick(&s, &at(1.0 / hz, 1.0));
            }
            st.values().to_vec()
        };
        let (a, b) = (run(60.0), run(144.0));
        assert!((a[0] - b[0]).abs() < 1e-4, "{a:?} vs {b:?}");
        assert!((a[1] - 1.5).abs() < 1e-3 && (b[1] - 1.5).abs() < 1e-3, "{a:?} vs {b:?}");
    }

    #[test]
    fn rate_wraps_and_ignores_stalls() {
        let s = specs("r = { rate = 1, wrap = 0.25 }").unwrap();
        let mut st = ParamState::new(1);
        st.tick(&s, &at(0.0, 0.0));
        for _ in 0..3 {
            st.tick(&s, &at(0.1, 0.0));
        }
        assert!((st.values()[0] - 0.05).abs() < 1e-5, "{}", st.values()[0]);
        st.tick(&s, &at(5.0, 0.0)); // a 5 s stall counts as MAX_STEP
        assert!((st.values()[0] - 0.15).abs() < 1e-5, "{}", st.values()[0]);
    }

    #[test]
    fn accessor_names_fields_in_order() {
        let s = specs("cx = 0\ncy = 0\nd = 0\ne = 0\nf = 0\n").unwrap();
        let w = wgsl_accessor(&s);
        assert!(w.contains("struct Params { cx: f32, cy: f32, d: f32, e: f32, f: f32 }"), "{w}");
        assert!(w.contains("palette.params[0].w, palette.params[1].x"), "{w}");
        assert!(wgsl_accessor(&[]).is_empty());
    }

    #[test]
    fn packs_into_vec4s() {
        let s = specs("a = 1\nb = 2\nc = 3\nd = 4\ne = 5\n").unwrap();
        let mut st = ParamState::new(5);
        st.tick(&s, &at(0.0, 0.0));
        assert_eq!(st.packed()[0], [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(st.packed()[1], [5.0, 0.0, 0.0, 0.0]);
    }
}
