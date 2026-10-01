//! Tiny expression mini-language for preset parameter mappings.
//!
//! Grammar (recursive descent):
//!   expr     = term (('+' | '-') term)*
//!   term     = factor (('*' | '/') factor)*
//!   factor   = '-'? primary
//!   primary  = number | ident | ident '(' arglist ')' | '(' expr ')'
//!   arglist  = expr (',' expr)*
//!
//! Variables: bass, mid, treble, bass_att, mid_att, treble_att, volume, beat,
//!            time, dt, frame, bpm, beat_phase, beat_count, aspect
//! Functions: sin, cos, tan, abs, sqrt, exp, log, floor, fract, sign,
//!            pow(x,y), min(a,b), max(a,b), atan2(y,x), step(edge,x),
//!            clamp(x,lo,hi), mix(a,b,t), smoothstep(e0,e1,x)
//!
//! Functions follow WGSL semantics so a mapping reads the same as shader code.

use std::fmt;

use crate::audio::AudioFeatures;

#[derive(Debug, Clone)]
pub enum Expr {
    Lit(f32),
    Var(VarKind),
    Neg(Box<Expr>),
    BinOp(Op, Box<Expr>, Box<Expr>),
    Call(Func, Vec<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarKind {
    Bass,
    Mid,
    Treble,
    BassAtt,
    MidAtt,
    TrebleAtt,
    Volume,
    Beat,
    Time,
    Dt,
    Frame,
    Bpm,
    BeatPhase,
    BeatCount,
    Aspect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Func {
    Sin,
    Cos,
    Tan,
    Abs,
    Sqrt,
    Exp,
    Log,
    Floor,
    Fract,
    Sign,
    Pow,
    Min,
    Max,
    Atan2,
    Step,
    Clamp,
    Mix,
    Smoothstep,
}

impl Func {
    fn arity(self) -> usize {
        match self {
            Func::Sin
            | Func::Cos
            | Func::Tan
            | Func::Abs
            | Func::Sqrt
            | Func::Exp
            | Func::Log
            | Func::Floor
            | Func::Fract
            | Func::Sign => 1,
            Func::Pow | Func::Min | Func::Max | Func::Atan2 | Func::Step => 2,
            Func::Clamp | Func::Mix | Func::Smoothstep => 3,
        }
    }
}

#[derive(Debug)]
pub struct ParseError {
    pub message: String,
    pub pos: usize,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (at position {})", self.message, self.pos)
    }
}

impl std::error::Error for ParseError {}

pub struct EvalContext<'a> {
    pub features: &'a AudioFeatures,
}

impl<'a> EvalContext<'a> {
    pub fn new(features: &'a AudioFeatures) -> Self {
        Self { features }
    }
}

impl Expr {
    pub fn eval(&self, ctx: &EvalContext) -> f32 {
        match self {
            Expr::Lit(v) => *v,
            Expr::Var(v) => match v {
                VarKind::Bass => ctx.features.bass,
                VarKind::Mid => ctx.features.mid,
                VarKind::Treble => ctx.features.treble,
                VarKind::BassAtt => ctx.features.bass_att,
                VarKind::MidAtt => ctx.features.mid_att,
                VarKind::TrebleAtt => ctx.features.treble_att,
                VarKind::Volume => ctx.features.volume,
                VarKind::Beat => ctx.features.beat,
                VarKind::Time => ctx.features.time,
                VarKind::Dt => ctx.features.dt,
                VarKind::Frame => ctx.features.frame,
                VarKind::Bpm => ctx.features.bpm,
                VarKind::BeatPhase => ctx.features.beat_phase,
                VarKind::BeatCount => ctx.features.beat_count,
                VarKind::Aspect => ctx.features.aspect,
            },
            Expr::Neg(e) => -e.eval(ctx),
            Expr::BinOp(op, a, b) => {
                let av = a.eval(ctx);
                let bv = b.eval(ctx);
                match op {
                    Op::Add => av + bv,
                    Op::Sub => av - bv,
                    Op::Mul => av * bv,
                    Op::Div => av / bv,
                }
            }
            Expr::Call(f, args) => match f {
                Func::Sin => args[0].eval(ctx).sin(),
                Func::Cos => args[0].eval(ctx).cos(),
                Func::Tan => args[0].eval(ctx).tan(),
                Func::Abs => args[0].eval(ctx).abs(),
                Func::Sqrt => args[0].eval(ctx).sqrt(),
                Func::Exp => args[0].eval(ctx).exp(),
                Func::Log => args[0].eval(ctx).ln(),
                Func::Floor => args[0].eval(ctx).floor(),
                Func::Fract => {
                    let x = args[0].eval(ctx);
                    x - x.floor()
                }
                Func::Sign => {
                    let x = args[0].eval(ctx);
                    if x > 0.0 {
                        1.0
                    } else if x < 0.0 {
                        -1.0
                    } else {
                        0.0
                    }
                }
                Func::Atan2 => args[0].eval(ctx).atan2(args[1].eval(ctx)),
                Func::Step => {
                    let edge = args[0].eval(ctx);
                    if args[1].eval(ctx) < edge { 0.0 } else { 1.0 }
                }
                Func::Pow => args[0].eval(ctx).powf(args[1].eval(ctx)),
                Func::Min => args[0].eval(ctx).min(args[1].eval(ctx)),
                Func::Max => args[0].eval(ctx).max(args[1].eval(ctx)),
                Func::Clamp => args[0].eval(ctx).clamp(args[1].eval(ctx), args[2].eval(ctx)),
                Func::Mix => {
                    let a = args[0].eval(ctx);
                    let b = args[1].eval(ctx);
                    let t = args[2].eval(ctx);
                    a + (b - a) * t
                }
                Func::Smoothstep => {
                    let e0 = args[0].eval(ctx);
                    let e1 = args[1].eval(ctx);
                    let t = ((args[2].eval(ctx) - e0) / (e1 - e0)).clamp(0.0, 1.0);
                    t * t * (3.0 - 2.0 * t)
                }
            },
        }
    }
}

pub fn parse(src: &str) -> Result<Expr, ParseError> {
    let mut p = Parser::new(src);
    let e = p.parse_expr()?;
    p.skip_ws();
    if p.pos < p.bytes.len() {
        return Err(p.err(format!("unexpected trailing input: '{}'", &src[p.pos..])));
    }
    Ok(e)
}

struct Parser<'a> {
    bytes: &'a [u8],
    src: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            bytes: src.as_bytes(),
            src,
            pos: 0,
        }
    }

    fn err(&self, msg: impl Into<String>) -> ParseError {
        ParseError {
            message: msg.into(),
            pos: self.pos,
        }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn eat(&mut self, c: u8) -> bool {
        self.skip_ws();
        if self.peek() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_term()?;
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b'+') => {
                    self.pos += 1;
                    let rhs = self.parse_term()?;
                    lhs = Expr::BinOp(Op::Add, Box::new(lhs), Box::new(rhs));
                }
                Some(b'-') => {
                    self.pos += 1;
                    let rhs = self.parse_term()?;
                    lhs = Expr::BinOp(Op::Sub, Box::new(lhs), Box::new(rhs));
                }
                _ => break,
            }
        }
        Ok(lhs)
    }

    fn parse_term(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_factor()?;
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b'*') => {
                    self.pos += 1;
                    let rhs = self.parse_factor()?;
                    lhs = Expr::BinOp(Op::Mul, Box::new(lhs), Box::new(rhs));
                }
                Some(b'/') => {
                    self.pos += 1;
                    let rhs = self.parse_factor()?;
                    lhs = Expr::BinOp(Op::Div, Box::new(lhs), Box::new(rhs));
                }
                _ => break,
            }
        }
        Ok(lhs)
    }

    fn parse_factor(&mut self) -> Result<Expr, ParseError> {
        self.skip_ws();
        if self.peek() == Some(b'-') {
            self.pos += 1;
            let inner = self.parse_factor()?;
            Ok(Expr::Neg(Box::new(inner)))
        } else {
            self.parse_primary()
        }
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        self.skip_ws();
        match self.peek() {
            Some(b'(') => {
                self.pos += 1;
                let e = self.parse_expr()?;
                self.skip_ws();
                if !self.eat(b')') {
                    return Err(self.err("expected ')'"));
                }
                Ok(e)
            }
            Some(c) if c.is_ascii_digit() || c == b'.' => self.parse_number(),
            Some(c) if c.is_ascii_alphabetic() || c == b'_' => self.parse_ident(),
            Some(c) => Err(self.err(format!("unexpected character '{}'", c as char))),
            None => Err(self.err("unexpected end of input")),
        }
    }

    fn parse_number(&mut self) -> Result<Expr, ParseError> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || c == b'.' {
                self.pos += 1;
            } else {
                break;
            }
        }
        let text = &self.src[start..self.pos];
        text.parse::<f32>()
            .map(Expr::Lit)
            .map_err(|_| ParseError {
                message: format!("invalid number '{text}'"),
                pos: start,
            })
    }

    fn parse_ident(&mut self) -> Result<Expr, ParseError> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == b'_' {
                self.pos += 1;
            } else {
                break;
            }
        }
        let name = &self.src[start..self.pos];
        self.skip_ws();
        if self.peek() == Some(b'(') {
            // function call
            self.pos += 1;
            let func = lookup_func(name).ok_or_else(|| ParseError {
                message: format!("unknown function '{name}'"),
                pos: start,
            })?;
            let mut args = Vec::new();
            self.skip_ws();
            if self.peek() != Some(b')') {
                loop {
                    args.push(self.parse_expr()?);
                    self.skip_ws();
                    if self.eat(b',') {
                        continue;
                    } else {
                        break;
                    }
                }
            }
            if !self.eat(b')') {
                return Err(self.err("expected ')'"));
            }
            if args.len() != func.arity() {
                return Err(ParseError {
                    message: format!(
                        "function '{name}' expects {} argument(s), got {}",
                        func.arity(),
                        args.len()
                    ),
                    pos: start,
                });
            }
            Ok(Expr::Call(func, args))
        } else {
            let v = lookup_var(name).ok_or_else(|| ParseError {
                message: format!("unknown variable '{name}'"),
                pos: start,
            })?;
            Ok(Expr::Var(v))
        }
    }
}

fn lookup_var(name: &str) -> Option<VarKind> {
    Some(match name {
        "bass" => VarKind::Bass,
        "mid" => VarKind::Mid,
        "treble" => VarKind::Treble,
        "bass_att" => VarKind::BassAtt,
        "mid_att" => VarKind::MidAtt,
        "treble_att" => VarKind::TrebleAtt,
        "volume" => VarKind::Volume,
        "beat" => VarKind::Beat,
        "time" => VarKind::Time,
        "dt" => VarKind::Dt,
        "frame" => VarKind::Frame,
        "bpm" => VarKind::Bpm,
        "beat_phase" => VarKind::BeatPhase,
        "beat_count" => VarKind::BeatCount,
        "aspect" => VarKind::Aspect,
        _ => return None,
    })
}

fn lookup_func(name: &str) -> Option<Func> {
    Some(match name {
        "sin" => Func::Sin,
        "cos" => Func::Cos,
        "tan" => Func::Tan,
        "abs" => Func::Abs,
        "sqrt" => Func::Sqrt,
        "exp" => Func::Exp,
        "log" => Func::Log,
        "floor" => Func::Floor,
        "fract" => Func::Fract,
        "sign" => Func::Sign,
        "pow" => Func::Pow,
        "min" => Func::Min,
        "max" => Func::Max,
        "atan2" => Func::Atan2,
        "step" => Func::Step,
        "clamp" => Func::Clamp,
        "mix" => Func::Mix,
        "smoothstep" => Func::Smoothstep,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_with(bass: f32, mid: f32, treble: f32, beat: f32, time: f32) -> AudioFeatures {
        AudioFeatures {
            bass,
            mid,
            treble,
            volume: 0.5,
            beat,
            time,
            ..Default::default()
        }
    }

    fn eval(src: &str, f: &AudioFeatures) -> f32 {
        parse(src).unwrap().eval(&EvalContext::new(f))
    }

    #[test]
    fn literal() {
        let f = AudioFeatures::default();
        assert_eq!(eval("1.5", &f), 1.5);
        assert_eq!(eval("0", &f), 0.0);
    }

    #[test]
    fn variables() {
        let f = ctx_with(0.3, 0.6, 0.9, 1.0, 2.5);
        assert_eq!(eval("bass", &f), 0.3);
        assert_eq!(eval("mid", &f), 0.6);
        assert_eq!(eval("treble", &f), 0.9);
        assert_eq!(eval("beat", &f), 1.0);
        assert_eq!(eval("time", &f), 2.5);
    }

    #[test]
    fn precedence() {
        let f = AudioFeatures::default();
        assert_eq!(eval("1 + 2 * 3", &f), 7.0);
        assert_eq!(eval("(1 + 2) * 3", &f), 9.0);
        assert_eq!(eval("10 - 4 - 2", &f), 4.0);
        assert_eq!(eval("8 / 2 / 2", &f), 2.0);
    }

    #[test]
    fn negation() {
        let f = ctx_with(0.5, 0.0, 0.0, 0.0, 0.0);
        assert_eq!(eval("-bass", &f), -0.5);
        assert_eq!(eval("1 - -bass", &f), 1.5);
    }

    #[test]
    fn function_calls() {
        let f = ctx_with(0.0, 0.0, 0.0, 0.0, 0.0);
        assert!((eval("sin(0)", &f) - 0.0).abs() < 1e-6);
        assert!((eval("cos(0)", &f) - 1.0).abs() < 1e-6);
        assert_eq!(eval("abs(-3)", &f), 3.0);
        assert_eq!(eval("pow(2, 8)", &f), 256.0);
        assert_eq!(eval("min(5, 3)", &f), 3.0);
        assert_eq!(eval("max(5, 3)", &f), 5.0);
        assert_eq!(eval("clamp(10, 0, 5)", &f), 5.0);
        assert_eq!(eval("mix(0, 10, 0.25)", &f), 2.5);
    }

    #[test]
    fn att_vars_read_their_own_fields() {
        let f = AudioFeatures {
            bass: 0.9,
            bass_att: 0.3,
            mid_att: 0.4,
            treble_att: 0.5,
            ..Default::default()
        };
        assert_eq!(eval("bass_att", &f), 0.3);
        assert_eq!(eval("mid_att", &f), 0.4);
        assert_eq!(eval("treble_att", &f), 0.5);
    }

    #[test]
    fn timing_and_tempo_vars() {
        let f = AudioFeatures {
            dt: 0.016,
            frame: 42.0,
            bpm: 128.0,
            beat_phase: 0.25,
            beat_count: 7.0,
            aspect: 2.0,
            ..Default::default()
        };
        assert_eq!(eval("dt", &f), 0.016);
        assert_eq!(eval("frame", &f), 42.0);
        assert_eq!(eval("bpm", &f), 128.0);
        assert_eq!(eval("beat_phase", &f), 0.25);
        assert_eq!(eval("beat_count", &f), 7.0);
        assert_eq!(eval("aspect", &f), 2.0);
    }

    #[test]
    fn wgsl_style_functions() {
        let f = AudioFeatures::default();
        assert_eq!(eval("floor(2.7)", &f), 2.0);
        assert!((eval("fract(2.75)", &f) - 0.75).abs() < 1e-6);
        assert!((eval("fract(0 - 0.25)", &f) - 0.75).abs() < 1e-6);
        assert_eq!(eval("sign(0 - 3)", &f), -1.0);
        assert_eq!(eval("sign(0)", &f), 0.0);
        assert_eq!(eval("step(0.5, 0.4)", &f), 0.0);
        assert_eq!(eval("step(0.5, 0.5)", &f), 1.0);
        assert_eq!(eval("smoothstep(0, 1, 0.5)", &f), 0.5);
        assert_eq!(eval("smoothstep(0, 1, 2)", &f), 1.0);
        assert!((eval("atan2(1, 1)", &f) - std::f32::consts::FRAC_PI_4).abs() < 1e-6);
        assert!((eval("exp(log(5))", &f) - 5.0).abs() < 1e-5);
        assert!(eval("tan(0)", &f).abs() < 1e-6);
    }

    #[test]
    fn realistic_mapping() {
        let f = ctx_with(0.4, 0.0, 0.0, 0.0, 0.0);
        // From the plan: zoom = 1.0 + bass * 0.08
        assert!((eval("1.0 + bass * 0.08", &f) - 1.032).abs() < 1e-6);
    }

    #[test]
    fn unknown_var_errors() {
        assert!(parse("flub * 2").is_err());
    }

    #[test]
    fn unknown_func_errors() {
        assert!(parse("sin(bass) + bogus(1)").is_err());
    }

    #[test]
    fn arity_mismatch_errors() {
        assert!(parse("pow(2)").is_err());
        assert!(parse("clamp(1, 2)").is_err());
    }

    #[test]
    fn unbalanced_parens_errors() {
        assert!(parse("(1 + 2").is_err());
        assert!(parse("1 + 2)").is_err());
    }

    #[test]
    fn whitespace_tolerant() {
        let f = AudioFeatures::default();
        assert_eq!(eval("  1   +   2  ", &f), 3.0);
        assert_eq!(eval("\t1\n+\t2\n", &f), 3.0);
    }
}
