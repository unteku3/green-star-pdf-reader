use serde::{Deserialize, Serialize};

/// The final output of the layout engine: a flat list of drawing commands
/// with absolute coordinates, ready for platform renderers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayList {
    pub items: Vec<DisplayItem>,
    pub width: f64,
    pub height: f64,
    pub depth: f64,
}

/// A single drawing instruction with absolute position.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DisplayItem {
    /// System-font fallback text; all bundled font glyphs arrive as paths.
    Text {
        x: f64,
        y: f64,
        scale: f64,
        text: String,
        color: Color,
    },
    /// Draw a horizontal line (fraction bars, overlines, etc.).
    Line {
        x: f64,
        y: f64,
        width: f64,
        thickness: f64,
        color: Color,
        /// If true, render as a dashed line (for \hdashline).
        #[serde(default)]
        dashed: bool,
    },
    /// Draw a filled rectangle (\colorbox backgrounds).
    Rect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        color: Color,
    },
    /// Draw an arbitrary SVG-style path (radical signs, large delimiters).
    Path {
        x: f64,
        y: f64,
        commands: Vec<PathCommand>,
        fill: bool,
        color: Color,
    },
}

impl DisplayList {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            width: 0.0,
            height: 0.0,
            depth: 0.0,
        }
    }

    pub fn total_height(&self) -> f64 {
        self.height + self.depth
    }
}

impl Default for DisplayList {
    fn default() -> Self {
        Self::new()
    }
}

/// SVG-style path commands for drawing glyph outlines, radical signs,
/// large delimiters, etc.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PathCommand {
    MoveTo {
        x: f64,
        y: f64,
    },
    LineTo {
        x: f64,
        y: f64,
    },
    CubicTo {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        x: f64,
        y: f64,
    },
    QuadTo {
        x1: f64,
        y1: f64,
        x: f64,
        y: f64,
    },
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}
impl Color {
    pub const BLACK: Self = Self {
        r: 0.,
        g: 0.,
        b: 0.,
        a: 1.,
    };
}
impl DisplayList {
    pub fn memory_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.items.capacity() * std::mem::size_of::<DisplayItem>()
            + self
                .items
                .iter()
                .map(|item| match item {
                    DisplayItem::Text { text, .. } => text.capacity(),
                    DisplayItem::Path { commands, .. } => {
                        commands.capacity() * std::mem::size_of::<PathCommand>()
                    }
                    _ => 0,
                })
                .sum::<usize>()
    }
}

impl DisplayList {
    /// Bounds both retained memory and geometry received from a native provider.
    pub fn validate(&self) -> Result<(), String> {
        let good = |values: &[f64]| values.iter().all(|v| v.is_finite() && v.abs() <= 4096.);
        if !good(&[self.width, self.height, self.depth])
            || self.width < 0.
            || self.total_height() < 0.
            || self.items.len() > 8192
            || self.memory_bytes() > 8 * 1024 * 1024
        {
            return Err("drawing exceeds bounds".into());
        }
        let mut command_count = 0;
        for item in &self.items {
            let (valid, color) = match item {
                DisplayItem::Text {
                    x,
                    y,
                    scale,
                    text,
                    color,
                } => (
                    good(&[*x, *y, *scale]) && *scale >= 0. && text.chars().count() == 1,
                    color,
                ),
                DisplayItem::Line {
                    x,
                    y,
                    width,
                    thickness,
                    color,
                    ..
                } => (
                    good(&[*x, *y, *width, *thickness]) && *width >= 0. && *thickness >= 0.,
                    color,
                ),
                DisplayItem::Rect {
                    x,
                    y,
                    width,
                    height,
                    color,
                } => (
                    good(&[*x, *y, *width, *height]) && *width >= 0. && *height >= 0.,
                    color,
                ),
                DisplayItem::Path {
                    x,
                    y,
                    commands,
                    color,
                    ..
                } => {
                    command_count += commands.len();
                    let valid = good(&[*x, *y])
                        && commands.iter().all(|c| match c {
                            PathCommand::MoveTo { x, y } | PathCommand::LineTo { x, y } => {
                                good(&[*x, *y])
                            }
                            PathCommand::QuadTo { x1, y1, x, y } => good(&[*x1, *y1, *x, *y]),
                            PathCommand::CubicTo {
                                x1,
                                y1,
                                x2,
                                y2,
                                x,
                                y,
                            } => good(&[*x1, *y1, *x2, *y2, *x, *y]),
                            PathCommand::Close => true,
                        });
                    (valid, color)
                }
            };
            if !valid
                || ![color.r, color.g, color.b, color.a]
                    .iter()
                    .all(|c| c.is_finite() && (0. ..=1.).contains(c))
            {
                return Err("invalid drawing geometry".into());
            }
        }
        if command_count > 200_000 {
            return Err("too many path commands".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_remote_geometry() {
        let mut drawing = DisplayList::default();
        drawing.width = f64::NAN;
        assert!(drawing.validate().is_err());
        drawing.width = 1.;
        drawing.items.push(DisplayItem::Text {
            x: 0.,
            y: 0.,
            scale: 1.,
            text: "too much text".into(),
            color: Color::BLACK,
        });
        assert!(drawing.validate().is_err());
        drawing.items.clear();
        drawing.items.push(DisplayItem::Path {
            x: 0.,
            y: 0.,
            commands: vec![PathCommand::LineTo {
                x: f64::INFINITY,
                y: 0.,
            }],
            fill: true,
            color: Color::BLACK,
        });
        assert!(drawing.validate().is_err());
    }
}
