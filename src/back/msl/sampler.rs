#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Coord {
    Normalized,
    Pixel,
}

impl Default for Coord {
    fn default() -> Self {
        Self::Normalized
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Address {
    Repeat,
    MirroredRepeat,
    ClampToEdge,
    ClampToZero,
    ClampToBorder,
}

impl Default for Address {
    fn default() -> Self {
        Self::ClampToEdge
    }
}

impl Address {
    pub fn as_str(&self) -> &'static str {
        match *self {
            Self::Repeat => "repeat",
            Self::MirroredRepeat => "mirrored_repeat",
            Self::ClampToEdge => "clamp_to_edge",
            Self::ClampToZero => "clamp_to_zero",
            Self::ClampToBorder => "clamp_to_border",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BorderColor {
    TransparentBlack,
    OpaqueBlack,
    OpaqueWhite,
}

impl Default for BorderColor {
    fn default() -> Self {
        Self::TransparentBlack
    }
}

impl BorderColor {
    pub fn as_str(&self) -> &'static str {
        match *self {
            Self::TransparentBlack => "transparent_black",
            Self::OpaqueBlack => "opaque_black",
            Self::OpaqueWhite => "opaque_white",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Filter {
    Nearest,
    Linear,
}

impl Filter {
    pub fn as_str(&self) -> &'static str {
        match *self {
            Self::Nearest => "nearest",
            Self::Linear => "linear",
        }
    }
}

impl Default for Filter {
    fn default() -> Self {
        Self::Nearest
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CompareFunc {
    Never,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Equal,
    NotEqual,
    Always,
}

impl Default for CompareFunc {
    fn default() -> Self {
        Self::Never
    }
}

impl CompareFunc {
    pub fn as_str(&self) -> &'static str {
        match *self {
            Self::Never => "never",
            Self::Less => "less",
            Self::LessEqual => "less_equal",
            Self::Greater => "greater",
            Self::GreaterEqual => "greater_equal",
            Self::Equal => "equal",
            Self::NotEqual => "not_equal",
            Self::Always => "always",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct InlineSampler {
    pub coord: Coord,
    pub address: [Address; 3],
    pub border_color: BorderColor,
    pub mag_filter: Filter,
    pub min_filter: Filter,
    pub mip_filter: Option<Filter>,
    pub compare_func: CompareFunc,
}
