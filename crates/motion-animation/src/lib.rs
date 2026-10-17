//! Deterministic animation evaluation over project ticks.
//!
//! Keyframe times are expressed in source-relative ticks. The caller is
//! responsible for mapping the sequence playhead to source time before calling
//! this crate.

use motion_core::{
    project::{Clip, Interpolation, Keyframe, Transform, TransformKeyframes, Vec2},
    time::Tick,
};

/// Evaluate a clip's transform at a source-relative time.
///
/// Each animated component is evaluated independently. Components without
/// keyframes retain the clip's base transform. Keyframes outside the animated
/// range hold the nearest endpoint value; an empty keyframe list leaves the
/// base value unchanged.
pub fn evaluate_transform(clip: &Clip, source_time: Tick) -> Transform {
    evaluate_transform_keyframes(&clip.transform, &clip.keyframes, source_time)
}

/// Evaluate keyframes over a base transform. This is public to make the
/// interpolation behavior useful to callers that already hold the two model
/// values separately.
pub fn evaluate_transform_keyframes(
    base: &Transform,
    keyframes: &TransformKeyframes,
    source_time: Tick,
) -> Transform {
    Transform {
        position: evaluate(&keyframes.position, source_time, base.position),
        anchor: base.anchor,
        scale: evaluate(&keyframes.scale, source_time, base.scale),
        rotation_degrees: evaluate(&keyframes.rotation_degrees, source_time, base.rotation_degrees),
        opacity: evaluate(&keyframes.opacity, source_time, base.opacity),
    }
}

trait Lerp: Copy {
    fn lerp(self, other: Self, amount: f64) -> Self;
}

impl Lerp for f32 {
    fn lerp(self, other: Self, amount: f64) -> Self {
        self + (other - self) * amount as f32
    }
}

impl Lerp for f64 {
    fn lerp(self, other: Self, amount: f64) -> Self {
        self + (other - self) * amount
    }
}

impl Lerp for Vec2 {
    fn lerp(self, other: Self, amount: f64) -> Self {
        Vec2 {
            x: self.x + (other.x - self.x) * amount,
            y: self.y + (other.y - self.y) * amount,
        }
    }
}

fn evaluate<T: Lerp>(keyframes: &[Keyframe<T>], time: Tick, base: T) -> T {
    if keyframes.is_empty() {
        return base;
    }

    // Selecting by time rather than relying on vector order keeps evaluation
    // deterministic even while an editor is assembling an unvalidated model.
    let mut previous: Option<&Keyframe<T>> = None;
    let mut next: Option<&Keyframe<T>> = None;
    for keyframe in keyframes {
        if keyframe.time <= time
            && previous.is_none_or(|candidate| keyframe.time > candidate.time)
        {
            previous = Some(keyframe);
        }
        if keyframe.time > time && next.is_none_or(|candidate| keyframe.time < candidate.time) {
            next = Some(keyframe);
        }
    }

    let Some(left) = previous else {
        return next.expect("nonempty keyframes have a first keyframe").value;
    };
    let Some(right) = next else {
        return left.value;
    };

    let span = i128::from(right.time.0) - i128::from(left.time.0);
    if span <= 0 {
        return left.value;
    }
    let elapsed = i128::from(time.0) - i128::from(left.time.0);
    let amount = elapsed as f64 / span as f64;
    left.value.lerp(right.value, apply_curve(left.interpolation, amount))
}

fn apply_curve(interpolation: Interpolation, amount: f64) -> f64 {
    let t = amount.clamp(0.0, 1.0);
    match interpolation {
        Interpolation::Linear => t,
        Interpolation::EaseIn => t * t,
        Interpolation::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
        Interpolation::EaseInOut => t * t * (3.0 - 2.0 * t),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(time: i64, value: f64, interpolation: Interpolation) -> Keyframe<f64> {
        Keyframe {
            time: Tick(time),
            value,
            interpolation,
        }
    }

    #[test]
    fn interpolation_curves_have_expected_midpoints() {
        let keys = [
            (Interpolation::Linear, 5.0),
            (Interpolation::EaseIn, 2.5),
            (Interpolation::EaseOut, 7.5),
            (Interpolation::EaseInOut, 5.0),
        ];
        for (curve, expected) in keys {
            let value = evaluate(&[key(0, 0.0, curve), key(10, 10.0, curve)], Tick(5), -1.0);
            assert!((value - expected).abs() < 1e-9);
        }
    }

    #[test]
    fn keyframes_hold_endpoints_and_use_source_relative_ticks() {
        let frames = [
            key(100, 4.0, Interpolation::Linear),
            key(200, 8.0, Interpolation::Linear),
        ];
        assert_eq!(evaluate(&frames, Tick(0), -1.0), 4.0);
        assert_eq!(evaluate(&frames, Tick(150), -1.0), 6.0);
        assert_eq!(evaluate(&frames, Tick(250), -1.0), 8.0);
    }

    #[test]
    fn empty_keyframes_preserve_the_base_value() {
        assert_eq!(evaluate::<f64>(&[], Tick(12), 3.25), 3.25);
    }

    #[test]
    fn unsorted_keys_are_evaluated_by_timestamp() {
        let frames = [
            key(20, 20.0, Interpolation::Linear),
            key(0, 0.0, Interpolation::Linear),
            key(10, 10.0, Interpolation::Linear),
        ];
        assert_eq!(evaluate(&frames, Tick(5), -1.0), 5.0);
    }

    #[test]
    fn tick_span_subtraction_handles_extreme_values() {
        let frames = [
            key(i64::MIN, 0.0, Interpolation::Linear),
            key(i64::MAX, 1.0, Interpolation::Linear),
        ];
        assert_eq!(evaluate(&frames, Tick(0), -1.0), 0.5);
    }

    #[test]
    fn transform_evaluation_preserves_anchor() {
        let base = Transform {
            anchor: Vec2 { x: 12.0, y: -3.0 },
            ..Transform::default()
        };
        let evaluated = evaluate_transform_keyframes(
            &base,
            &TransformKeyframes::default(),
            Tick(42),
        );
        assert_eq!(evaluated.anchor, base.anchor);
    }
}
