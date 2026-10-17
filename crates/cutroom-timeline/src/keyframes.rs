//! Keyframes: values of a clip's picture settings that change over time.
//!
//! Each setting (`Prop`) is animated on its own. A setting with no keyframes
//! keeps the clip's ordinary value; with keyframes it follows them, moving in
//! a straight line from one to the next and holding the first and last value
//! outside them. Times are measured from the start of the clip.

use crate::model::Transform;
use crate::time::Ticks;
use serde::{Deserialize, Serialize};

pub const WARP_LEN: usize = 8 + crate::model::MESH_SIDE * crate::model::MESH_SIDE * 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Prop {
    X,
    Y,
    Scale,
    Rotation,
    Opacity,
    SkewX,
    SkewY,
    PerspX,
    PerspY,
    /// Text clips: the size of the letters.
    TextSize,
    /// Text clips: the space between letters.
    TextSpacing,
    /// One number of the clip's bend (corner pin and mesh warp): 0..8 are the
    /// pin's, 8..40 the mesh's. They are always keyed together, as one element.
    Warp(u8),
}

impl Prop {
    pub const ALL: [Prop; 11] = [
        Prop::X,
        Prop::Y,
        Prop::Scale,
        Prop::Rotation,
        Prop::Opacity,
        Prop::SkewX,
        Prop::SkewY,
        Prop::PerspX,
        Prop::PerspY,
        Prop::TextSize,
        Prop::TextSpacing,
    ];
    /// The settings of "Position & Size" (every clip with a picture has them).
    pub const TRANSFORM: [Prop; 9] = [
        Prop::X,
        Prop::Y,
        Prop::Scale,
        Prop::Rotation,
        Prop::Opacity,
        Prop::SkewX,
        Prop::SkewY,
        Prop::PerspX,
        Prop::PerspY,
    ];
    /// All the numbers of the bend (corner pin and mesh warp), keyed as one.
    pub const WARP: [Prop; WARP_LEN] = {
        let mut all = [Prop::X; WARP_LEN];
        let mut i = 0;
        while i < WARP_LEN {
            all[i] = Prop::Warp(i as u8);
            i += 1;
        }
        all
    };

    /// Every setting that can have keyframes.
    pub fn each() -> impl Iterator<Item = Prop> {
        Prop::ALL.into_iter().chain(Prop::WARP)
    }

    /// Settings of the clip's picture (everything but the text's own).
    pub fn is_picture(self) -> bool {
        !self.is_text()
    }

    /// A stable order for sorting keyframes.
    pub fn order(self) -> u16 {
        match self {
            Prop::X => 0,
            Prop::Y => 1,
            Prop::Scale => 2,
            Prop::Rotation => 3,
            Prop::Opacity => 4,
            Prop::SkewX => 5,
            Prop::SkewY => 6,
            Prop::PerspX => 7,
            Prop::PerspY => 8,
            Prop::TextSize => 9,
            Prop::TextSpacing => 10,
            Prop::Warp(i) => 100 + u16::from(i),
        }
    }

    /// The settings of a text clip's "Text" section.
    pub const TEXT: [Prop; 2] = [Prop::TextSize, Prop::TextSpacing];

    pub fn is_text(self) -> bool {
        matches!(self, Prop::TextSize | Prop::TextSpacing)
    }

    /// The value in a clip's text (None for settings that are not text's).
    pub fn get_text(self, t: &crate::text::TextSpec) -> Option<i32> {
        match self {
            Prop::TextSize => Some(t.size),
            Prop::TextSpacing => Some(t.spacing),
            _ => None,
        }
    }

    pub fn set_text(self, t: &mut crate::text::TextSpec, value: i32) {
        match self {
            Prop::TextSize => t.size = value,
            Prop::TextSpacing => t.spacing = value,
            _ => {}
        }
    }

    /// The value in a transform (0 for text settings, which are not in one).
    pub fn get(self, t: &Transform) -> i32 {
        match self {
            Prop::SkewX => t.skew_x,
            Prop::SkewY => t.skew_y,
            Prop::PerspX => t.persp_x,
            Prop::PerspY => t.persp_y,
            Prop::TextSize | Prop::TextSpacing => 0,
            Prop::Warp(i) => {
                let i = usize::from(i);
                if i < 8 {
                    t.warp.pin[i]
                } else {
                    t.warp.mesh.get(i - 8).copied().unwrap_or(0)
                }
            }
            Prop::X => t.x,
            Prop::Y => t.y,
            Prop::Scale => t.scale,
            Prop::Rotation => t.rotation,
            Prop::Opacity => t.opacity,
        }
    }

    pub fn set(self, t: &mut Transform, value: i32) {
        match self {
            Prop::X => t.x = value,
            Prop::Y => t.y = value,
            Prop::Scale => t.scale = value,
            Prop::Rotation => t.rotation = value,
            Prop::Opacity => t.opacity = value,
            Prop::SkewX => t.skew_x = value,
            Prop::SkewY => t.skew_y = value,
            Prop::PerspX => t.persp_x = value,
            Prop::PerspY => t.persp_y = value,
            Prop::TextSize | Prop::TextSpacing => {}
            Prop::Warp(i) => {
                let i = usize::from(i);
                if i < 8 {
                    t.warp.pin[i] = value;
                } else if let Some(m) = t.warp.mesh.get_mut(i - 8) {
                    *m = value;
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keyframe {
    /// Measured from the start of the clip.
    pub time: Ticks,
    pub prop: Prop,
    pub value: i32,
}

/// The keyframes of one setting, earliest first.
pub fn of_prop(keys: &[Keyframe], prop: Prop) -> Vec<Keyframe> {
    let mut v: Vec<Keyframe> = keys.iter().copied().filter(|k| k.prop == prop).collect();
    v.sort_by_key(|k| k.time);
    v
}

/// The animated value of `prop` at `at` (from the clip start), or `None` if
/// it has no keyframes.
pub fn value_at(keys: &[Keyframe], prop: Prop, at: Ticks) -> Option<i32> {
    let v = of_prop(keys, prop);
    let first = v.first()?;
    let last = v.last()?;
    if at <= first.time {
        return Some(first.value);
    }
    if at >= last.time {
        return Some(last.value);
    }
    let i = v.partition_point(|k| k.time <= at);
    let (a, b) = (v[i - 1], v[i]);
    let span = (b.time - a.time).0 as f64;
    let f = (at - a.time).0 as f64 / span;
    Some((f64::from(a.value) + (f64::from(b.value) - f64::from(a.value)) * f).round() as i32)
}

/// Is there a keyframe of `prop` within `tolerance` of `at`?
pub fn key_near(keys: &[Keyframe], prop: Prop, at: Ticks, tolerance: Ticks) -> Option<usize> {
    keys.iter()
        .position(|k| k.prop == prop && (k.time - at).abs() <= tolerance)
}

/// Put a keyframe at `at`, replacing one within `tolerance` of it.
pub fn upsert(keys: &mut Vec<Keyframe>, prop: Prop, at: Ticks, value: i32, tolerance: Ticks) {
    match key_near(keys, prop, at, tolerance) {
        Some(i) => keys[i].value = value,
        None => keys.push(Keyframe {
            time: at,
            prop,
            value,
        }),
    }
    keys.sort_by_key(|k| (k.prop.order(), k.time));
}

/// The keyframes of the two halves of a clip cut at `at` (from its start):
/// each half gets a keyframe at the cut so the motion carries on unbroken.
pub fn split(keys: &[Keyframe], at: Ticks) -> (Vec<Keyframe>, Vec<Keyframe>) {
    let (mut left, mut right) = (Vec::new(), Vec::new());
    for prop in Prop::each() {
        let Some(value) = value_at(keys, prop, at) else {
            continue;
        };
        for k in of_prop(keys, prop) {
            if k.time < at {
                left.push(k);
            } else if k.time > at {
                right.push(Keyframe {
                    time: k.time - at,
                    ..k
                });
            }
        }
        left.push(Keyframe {
            time: at,
            prop,
            value,
        });
        right.push(Keyframe {
            time: Ticks::ZERO,
            prop,
            value,
        });
    }
    left.sort_by_key(|k| (k.prop.order(), k.time));
    right.sort_by_key(|k| (k.prop.order(), k.time));
    (left, right)
}

/// The same keyframes with every time moved by `by` (for a clip whose start was trimmed).
pub fn shifted(keys: &[Keyframe], by: Ticks) -> Vec<Keyframe> {
    keys.iter()
        .map(|k| Keyframe {
            time: k.time + by,
            ..*k
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(secs: f64) -> Ticks {
        Ticks::from_seconds_f64(secs)
    }

    fn k(secs: f64, prop: Prop, value: i32) -> Keyframe {
        Keyframe {
            time: s(secs),
            prop,
            value,
        }
    }

    #[test]
    fn values_move_in_straight_lines_and_hold_outside() {
        let keys = [k(1.0, Prop::X, 0), k(3.0, Prop::X, 200)];
        assert_eq!(value_at(&keys, Prop::X, s(0.0)), Some(0));
        assert_eq!(value_at(&keys, Prop::X, s(2.0)), Some(100));
        assert_eq!(value_at(&keys, Prop::X, s(2.5)), Some(150));
        assert_eq!(value_at(&keys, Prop::X, s(9.0)), Some(200));
        assert_eq!(value_at(&keys, Prop::Y, s(2.0)), None);
    }

    #[test]
    fn a_single_keyframe_holds_everywhere() {
        let keys = [k(2.0, Prop::Opacity, 400)];
        assert_eq!(value_at(&keys, Prop::Opacity, s(0.0)), Some(400));
        assert_eq!(value_at(&keys, Prop::Opacity, s(5.0)), Some(400));
    }

    #[test]
    fn upsert_replaces_a_nearby_keyframe_and_keeps_order() {
        let tol = Ticks(1000);
        let mut keys = vec![k(2.0, Prop::X, 10)];
        upsert(&mut keys, Prop::X, s(1.0), 5, tol);
        upsert(&mut keys, Prop::X, s(2.0), 99, tol);
        upsert(&mut keys, Prop::Y, s(2.0), 7, tol);
        assert_eq!(keys.len(), 3);
        assert_eq!(value_at(&keys, Prop::X, s(2.0)), Some(99));
        assert_eq!(
            key_near(&keys, Prop::X, s(2.0), tol).map(|i| keys[i].value),
            Some(99)
        );
    }

    #[test]
    fn splitting_keeps_the_motion_continuous_on_both_sides() {
        let keys = [k(0.0, Prop::X, 0), k(4.0, Prop::X, 400)];
        let (left, right) = split(&keys, s(1.0));
        // At the cut both sides are at 100.
        assert_eq!(value_at(&left, Prop::X, s(1.0)), Some(100));
        assert_eq!(value_at(&right, Prop::X, s(0.0)), Some(100));
        // The right half carries on to 400 after the 3 seconds that remain.
        assert_eq!(value_at(&right, Prop::X, s(3.0)), Some(400));
        assert_eq!(value_at(&right, Prop::X, s(1.5)), Some(250));
        // Settings without keyframes stay without.
        assert_eq!(value_at(&left, Prop::Y, s(0.5)), None);
    }

    #[test]
    fn the_bend_is_forty_numbers_keyed_together() {
        assert_eq!(Prop::WARP.len(), 40);
        let mut t = Transform::default();
        Prop::Warp(2).set(&mut t, 300);
        Prop::Warp(39).set(&mut t, -50);
        assert_eq!((t.warp.pin[2], t.warp.mesh[31]), (300, -50));
        assert_eq!((Prop::Warp(2).get(&t), Prop::Warp(39).get(&t)), (300, -50));
        let keys = [k(0.0, Prop::Warp(8), 0), k(2.0, Prop::Warp(8), 100)];
        assert_eq!(value_at(&keys, Prop::Warp(8), s(1.0)), Some(50));
        let (left, right) = split(&keys, s(1.0));
        assert_eq!(value_at(&left, Prop::Warp(8), s(1.0)), Some(50));
        assert_eq!(value_at(&right, Prop::Warp(8), s(0.0)), Some(50));
    }

    #[test]
    fn shifting_moves_every_time() {
        let keys = [k(1.0, Prop::X, 1), k(2.0, Prop::Y, 2)];
        let moved = shifted(&keys, s(-0.5));
        assert_eq!(moved[0].time, s(0.5));
        assert_eq!(moved[1].time, s(1.5));
    }
}
