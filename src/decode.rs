use std::{
    error::Error,
    fs::OpenOptions,
    io::{self, BufReader, Cursor, Read, Write},
    path::Path,
};

use crate::{Channels, Colorspace, Header, qoi_hash};

pub(crate) trait ReadExt: Read {
    fn read_byte(&mut self) -> io::Result<u8> {
        let mut result = [0u8; 1];
        self.read_exact(&mut result)?;
        Ok(result[0])
    }

    fn read_array<const N: usize>(&mut self) -> io::Result<[u8; N]> {
        let mut result = [0u8; N];
        self.read_exact(&mut result)?;
        Ok(result)
    }
}

impl<R: Read> ReadExt for R {}

#[derive(Debug, derive_more::Display, derive_more::From)]
pub enum DecodeError {
    IoError(io::Error),
    InvalidHeader,
    InvalidNumberOfChannels,
    InvalidColorspace,
    InvalidEofSequence,
}

impl Error for DecodeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        if let Self::IoError(e) = self {
            Some(e)
        } else {
            None
        }
    }
}

pub(crate) fn decode_header(stream: &mut impl Read) -> Result<Header, DecodeError> {
    if stream.read_array::<4>()? != *b"qoif" {
        return Err(DecodeError::InvalidHeader);
    }
    let result = Header {
        width: u32::from_be_bytes(stream.read_array::<4>()?),
        height: u32::from_be_bytes(stream.read_array::<4>()?),
        channels: Channels::from_byte(stream.read_byte()?).ok_or(DecodeError::InvalidColorspace)?,
        colorspace: Colorspace::from_byte(stream.read_byte()?)
            .ok_or(DecodeError::InvalidColorspace)?,
    };
    Ok(result)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Chunk {
    Rgb([u8; 3]),
    Rgba([u8; 4]),
    Index(u8),
    Diff {
        dr: i8,
        dg: i8,
        db: i8,
    },
    Luma {
        dg: i8,
        /// dr - dg.
        dr_dg: i8,
        /// db - dg.
        db_dg: i8,
    },
    Run(u8),
}

pub(crate) fn read_chunk(bytes: &mut impl Read) -> Result<Chunk, DecodeError> {
    fn msb2(x: u8) -> u8 {
        (x & 0b11000000) >> 6
    }

    fn lsb6(x: u8) -> u8 {
        x & 0b00111111
    }
    match bytes.read_byte()? {
        0b11111110 => Ok(Chunk::Rgb(bytes.read_array::<3>()?)),
        0b11111111 => Ok(Chunk::Rgba(bytes.read_array::<4>()?)),
        byte0 if msb2(byte0) == 0b00 => Ok(Chunk::Index(lsb6(byte0))),
        byte0 if msb2(byte0) == 0b01 => Ok(Chunk::Diff {
            dr: ((byte0 & 0b00_11_00_00) >> 4) as i8 - 2,
            dg: ((byte0 & 0b00_00_11_00) >> 2) as i8 - 2,
            db: (byte0 & 0b00_00_00_11) as i8 - 2,
        }),
        byte0 if msb2(byte0) == 0b10 => {
            let byte1 = bytes.read_byte()?;
            Ok(Chunk::Luma {
                dg: lsb6(byte0) as i8 - 32,
                dr_dg: ((byte1 & 0b11110000) >> 4) as i8 - 8,
                db_dg: (byte1 & 0b00001111) as i8 - 8,
            })
        }
        byte0 if msb2(byte0) == 0b11 => Ok(Chunk::Run(lsb6(byte0) + 1)),
        _ => unreachable!(),
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct DecoderState {
    pub(crate) header: Header,
    pub(crate) index_array: [[u8; 4]; 64],
    pub(crate) previous_pixel: [u8; 4],
    /// Number of pixels already decoded.
    pub(crate) n_pixels: usize,
}

impl DecoderState {
    pub(crate) fn new(header: Header) -> Self {
        Self {
            header,
            index_array: [[0u8; 4]; 64],
            previous_pixel: [0, 0, 0, 255],
            n_pixels: 0,
        }
    }
}

/// Returns `true` if is end of byte stream.
pub(crate) fn decode_chunk(
    state: &mut DecoderState,
    stream: &mut impl Read,
    output: &mut impl Write,
) -> Result<(), DecodeError> {
    let chunk = read_chunk(stream)?;
    let current_pixel: [u8; 4] = match chunk {
        Chunk::Rgb(rgb) => [rgb[0], rgb[1], rgb[2], state.previous_pixel[3]],
        Chunk::Rgba(rgba) => rgba,
        Chunk::Index(index) => state.index_array[index as usize],
        Chunk::Diff { dr, dg, db } => [
            state.previous_pixel[0].wrapping_add_signed(dr),
            state.previous_pixel[1].wrapping_add_signed(dg),
            state.previous_pixel[2].wrapping_add_signed(db),
            state.previous_pixel[3],
        ],
        Chunk::Luma { dg, dr_dg, db_dg } => [
            state.previous_pixel[0].wrapping_add_signed(dr_dg + dg),
            state.previous_pixel[1].wrapping_add_signed(dg),
            state.previous_pixel[2].wrapping_add_signed(db_dg + dg),
            state.previous_pixel[3],
        ],
        Chunk::Run(run) => {
            for _ in 0..run {
                match state.header.channels {
                    Channels::Rgb => output.write_all(&state.previous_pixel[0..=2])?,
                    Channels::Rgba => output.write_all(&state.previous_pixel)?,
                }
            }
            state.n_pixels += run as usize;
            return Ok(());
        }
    };
    let index = qoi_hash(current_pixel);
    state.index_array[index] = current_pixel;
    state.previous_pixel = current_pixel;
    state.n_pixels += 1;
    match state.header.channels {
        Channels::Rgb => output.write_all(&state.previous_pixel[0..=2])?,
        Channels::Rgba => output.write_all(&state.previous_pixel)?,
    }
    Ok(())
}

pub(crate) fn verify_eof_sequence(bytes: &mut impl Read) -> Result<(), DecodeError> {
    if bytes.read_array::<7>()? != [0; 7] {
        return Err(DecodeError::InvalidEofSequence);
    }
    if bytes.read_byte()? != 1 {
        return Err(DecodeError::InvalidEofSequence);
    }
    Ok(())
}

pub fn decode(input: &mut impl Read, output: &mut impl Write) -> Result<Header, DecodeError> {
    let header = decode_header(input)?;
    let n_pixels = (header.width * header.height) as usize;
    let mut decoder_state = DecoderState::new(header);
    while decoder_state.n_pixels < n_pixels {
        decode_chunk(&mut decoder_state, input, output)?;
    }
    verify_eof_sequence(input)?;
    Ok(header)
}

pub fn decode_to_vec(input: &mut impl Read) -> Result<(Vec<u8>, Header), DecodeError> {
    let mut data = Vec::new();
    let mut cursor = Cursor::new(&mut data);
    let header = decode(input, &mut cursor).unwrap();
    Ok((data, header))
}

pub fn decode_from_data(data: &[u8]) -> Result<(Vec<u8>, Header), DecodeError> {
    let mut output = Vec::new();
    let header = decode(&mut Cursor::new(data), &mut Cursor::new(&mut output)).unwrap();
    Ok((output, header))
}

pub fn decode_from_file(
    path: impl AsRef<Path>,
    output: &mut impl Write,
) -> Result<Header, DecodeError> {
    let file = OpenOptions::new()
        .read(true)
        .write(false)
        .create(false)
        .open(&path)
        .unwrap();
    let mut reader = BufReader::new(file);
    let header = decode(&mut reader, output).unwrap();
    Ok(header)
}

pub fn decode_from_file_to_vec(path: impl AsRef<Path>) -> Result<(Vec<u8>, Header), DecodeError> {
    let file = OpenOptions::new()
        .read(true)
        .write(false)
        .create(false)
        .open(&path)
        .unwrap();
    let mut reader = BufReader::new(file);
    let mut data = Vec::new();
    let mut cursor = Cursor::new(&mut data);
    let header = decode(&mut reader, &mut cursor).unwrap();
    Ok((data, header))
}
