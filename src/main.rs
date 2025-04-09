#![feature(array_chunks)]

use std::{
    fs::File,
    io::{self, BufWriter, Write as _},
    path::Path,
};

pub mod decode;
pub mod encode;

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

fn save_data(path: impl AsRef<Path>, data: &[u8]) -> io::Result<()> {
    let file = File::options()
        .read(false)
        .write(true)
        .append(false)
        .create(true)
        .truncate(true)
        .open(path)?;
    let mut writer = BufWriter::new(file);
    writer.write_all(data)
}

fn main() {
    let input_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| panic!("Expect 1 argument for input file"));

    let (data, header) = decode::decode_from_file_to_vec(&input_path).unwrap();

    let mut encoded_data = Vec::new();
    match header.channels {
        Channels::Rgb => {
            let pixels = data
                .array_chunks::<3>()
                .copied()
                .map(|x| [x[0], x[1], x[2], 255]);
            encode::encode(header, pixels, &mut encoded_data);
        }
        Channels::Rgba => {
            let pixels = data.array_chunks::<4>().copied();
            encode::encode(header, pixels, &mut encoded_data);
        }
    }

    save_data("roundtripped.qoi", &encoded_data).unwrap();
}
