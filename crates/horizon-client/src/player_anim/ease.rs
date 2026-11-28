//! PlayerAnimator easings (`dev.kosmx.playerAnim.core.util.Ease`).
//!
//! Every curve is defined in its "in" form; "out" mirrors it and "in-out"
//! joins the two halves. Both file formats name them differently
//! (`EASEINOUTQUAD`, `easeInOutSine`, `INQUAD`): [`Ease::parse`] accepts all.

use std::f32::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Sine,
    Quad,
    Cubic,
    Quart,
    Quint,
    Expo,
    Circ,
    Back,
    Elastic,
    Bounce,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    In,
    Out,
    InOut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ease {
    Linear,
    /// Holds the source value until the destination key.
    Constant,
    /// Jumps at `arg` (default one half).
    Step,
    Curve(Family, Direction),
}

impl Ease {
    /// `INOUTSINE`, the fade curve Iron's Spells and Better Combat use.
    pub const IN_OUT_SINE: Ease = Ease::Curve(Family::Sine, Direction::InOut);

    pub fn parse(name: &str) -> Option<Ease> {
        let lower: String = name
            .chars()
            .filter(|c| !matches!(c, '_' | '-') && !c.is_whitespace())
            .collect::<String>()
            .to_ascii_lowercase();
        let n = lower.strip_prefix("ease").unwrap_or(&lower);
        match n {
            "linear" | "catmullrom" => return Some(Ease::Linear),
            "constant" => return Some(Ease::Constant),
            "step" => return Some(Ease::Step),
            _ => {}
        }
        let (direction, rest) = if let Some(rest) = n.strip_prefix("inout") {
            (Direction::InOut, rest)
        } else if let Some(rest) = n.strip_prefix("in") {
            (Direction::In, rest)
        } else if let Some(rest) = n.strip_prefix("out") {
            (Direction::Out, rest)
        } else {
            return None;
        };
        let family = match rest {
            "sine" => Family::Sine,
            "quad" | "quadratic" => Family::Quad,
            "cubic" => Family::Cubic,
            "quart" => Family::Quart,
            "quint" => Family::Quint,
            "expo" | "exponential" => Family::Expo,
            "circ" | "circle" => Family::Circ,
            "back" => Family::Back,
            "elastic" => Family::Elastic,
            "bounce" => Family::Bounce,
            _ => return None,
        };
        Some(Ease::Curve(family, direction))
    }

    /// The eased progress for `t` in 0..=1; `arg` is the key's easing argument.
    pub fn apply(self, t: f32, arg: Option<f32>) -> f32 {
        match self {
            Ease::Linear => t,
            Ease::Constant => {
                if t >= 1.0 {
                    1.0
                } else {
                    0.0
                }
            }
            Ease::Step => {
                if t < arg.unwrap_or(0.5) {
                    0.0
                } else {
                    1.0
                }
            }
            Ease::Curve(family, direction) => match direction {
                Direction::In => in_form(family, t, arg),
                Direction::Out => 1.0 - in_form(family, 1.0 - t, arg),
                Direction::InOut => {
                    if t < 0.5 {
                        in_form(family, t * 2.0, arg) / 2.0
                    } else {
                        1.0 - in_form(family, 2.0 - t * 2.0, arg) / 2.0
                    }
                }
            },
        }
    }
}

fn in_form(family: Family, t: f32, arg: Option<f32>) -> f32 {
    match family {
        Family::Sine => 1.0 - (t * PI / 2.0).cos(),
        Family::Quad => t * t,
        Family::Cubic => t * t * t,
        Family::Quart => t.powi(4),
        Family::Quint => t.powi(5),
        Family::Expo => 2.0_f32.powf(10.0 * (t - 1.0)),
        Family::Circ => 1.0 - (1.0 - t * t).max(0.0).sqrt(),
        Family::Back => {
            let n = arg.map_or(1.70158, |a| a * 1.70158);
            t * t * ((n + 1.0) * t - n)
        }
        Family::Elastic => {
            let n = arg.unwrap_or(1.0);
            1.0 - (t * PI / 2.0).cos().powi(3) * (t * n * PI).cos()
        }
        Family::Bounce => 1.0 - bounce_out(1.0 - t),
    }
}

fn bounce_out(t: f32) -> f32 {
    const N: f32 = 7.5625;
    const D: f32 = 2.75;
    if t < 1.0 / D {
        N * t * t
    } else if t < 2.0 / D {
        let t = t - 1.5 / D;
        N * t * t + 0.75
    } else if t < 2.5 / D {
        let t = t - 2.25 / D;
        N * t * t + 0.9375
    } else {
        let t = t - 2.625 / D;
        N * t * t + 0.984_375
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_spelling() {
        let inout_quad = Some(Ease::Curve(Family::Quad, Direction::InOut));
        assert_eq!(Ease::parse("EASEINOUTQUAD"), inout_quad);
        assert_eq!(Ease::parse("easeInOutQuad"), inout_quad);
        assert_eq!(
            Ease::parse("INQUAD"),
            Some(Ease::Curve(Family::Quad, Direction::In))
        );
        assert_eq!(
            Ease::parse("easeOutElastic"),
            Some(Ease::Curve(Family::Elastic, Direction::Out))
        );
        assert_eq!(Ease::parse("linear"), Some(Ease::Linear));
        assert_eq!(Ease::parse("step"), Some(Ease::Step));
        assert_eq!(Ease::parse("nonsense"), None);
    }

    #[test]
    fn curves_hit_their_endpoints() {
        for name in ["easeInOutSine", "easeInQuad", "easeOutCubic", "easeInOutQuint"] {
            let ease = Ease::parse(name).unwrap();
            assert!(ease.apply(0.0, None).abs() < 1e-5, "{name} at 0");
            assert!((ease.apply(1.0, None) - 1.0).abs() < 1e-5, "{name} at 1");
        }
        let elastic = Ease::parse("easeInElastic").unwrap();
        assert!(elastic.apply(0.0, Some(2.0)).abs() < 1e-5);
        assert!((elastic.apply(1.0, Some(2.0)) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn known_values() {
        let inout_quad = Ease::parse("EASEINOUTQUAD").unwrap();
        assert!((inout_quad.apply(0.25, None) - 0.125).abs() < 1e-6);
        assert!((inout_quad.apply(0.75, None) - 0.875).abs() < 1e-6);
        assert_eq!(Ease::Constant.apply(0.99, None), 0.0);
        assert_eq!(Ease::Step.apply(0.6, None), 1.0);
        assert_eq!(Ease::Step.apply(0.6, Some(0.7)), 0.0);
    }
}
