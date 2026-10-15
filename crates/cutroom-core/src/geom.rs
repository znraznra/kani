//! Minimal 2D geometry used by the timeline logic, so it doesn't depend on a GUI toolkit.

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pos2 {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

pub const fn pos2(x: f32, y: f32) -> Pos2 {
    Pos2 { x, y }
}

pub const fn vec2(x: f32, y: f32) -> Vec2 {
    Vec2 { x, y }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub min: Pos2,
    pub max: Pos2,
}

impl Rect {
    pub fn from_min_size(min: Pos2, size: Vec2) -> Rect {
        Rect {
            min,
            max: pos2(min.x + size.x, min.y + size.y),
        }
    }

    pub fn left(&self) -> f32 {
        self.min.x
    }

    pub fn right(&self) -> f32 {
        self.max.x
    }

    pub fn top(&self) -> f32 {
        self.min.y
    }

    pub fn bottom(&self) -> f32 {
        self.max.y
    }

    pub fn width(&self) -> f32 {
        self.max.x - self.min.x
    }

    pub fn height(&self) -> f32 {
        self.max.y - self.min.y
    }
}
