#![feature(array_chunks)]

mod decode;
mod encode;

pub use decode::*;
pub use encode::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Header {
    pub width: u32,
    pub height: u32,
    pub channels: Channels,
    pub colorspace: Colorspace,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Channels {
    #[default]
    Rgb = 3,
    Rgba = 4,
}

impl Channels {
    pub(crate) fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            3 => Some(Self::Rgb),
            4 => Some(Self::Rgba),
            _ => None,
        }
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Colorspace {
    #[default]
    /// sRGB with linear alpha.
    Srgb = 0,
    /// All channels linear.
    Rgb = 1,
}

impl Colorspace {
    pub(crate) fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Srgb),
            1 => Some(Self::Rgb),
            _ => None,
        }
    }
}

/// The hashing function used by QOI.
pub(crate) fn qoi_hash(rgba: [u8; 4]) -> usize {
    let [r, g, b, a] = rgba.map(usize::from);
    (r * 3 + g * 5 + b * 7 + a * 11) % 64
}
