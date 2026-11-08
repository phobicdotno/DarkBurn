//! The drawing and selection tools: which one is active and its state between frames.

/// The active canvas tool.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    #[default]
    Select,
    Line,
    Rect,
    Ellipse,
    Polygon,
}

impl Tool {
    /// The command that picks this tool.
    pub fn command(self) -> &'static str {
        match self {
            Tool::Select => "tools.select",
            Tool::Line => "tools.line",
            Tool::Rect => "tools.rect",
            Tool::Ellipse => "tools.ellipse",
            Tool::Polygon => "tools.polygon",
        }
    }
}
