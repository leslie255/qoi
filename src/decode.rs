use std::{
    error::Error,
    fmt::{self, Debug, Display},
    fs::OpenOptions,
    io::{self, BufReader, Cursor, Read, Write},
    path::Path,
};

use crate::{Channels, Colorspace, Header, qoi_hash};

pub fn decode(input: &mut impl Read, output: &mut impl Write) -> Result<Header, DecodeError> {
    let header = decode_header(input)?;
    let n_pixels = (header.width * header.height) as usize;
    let mut decoder = Decoder::new(header);
    while decoder.n_pixels < n_pixels {
        decoder.decode_chunk(input, output)?;
    }
    decoder.verify_eof_sequence(input)?;
    Ok(header)
}

/// Convenience function that calls `decode`.
pub fn decode_to_vec(input: &mut impl Read) -> Result<(Vec<u8>, Header), DecodeError> {
    let mut data = Vec::new();
    let mut cursor = Cursor::new(&mut data);
    let header = decode(input, &mut cursor).unwrap();
    Ok((data, header))
}

fn read_file(path: impl AsRef<Path>) -> io::Result<impl Read> {
    let file = OpenOptions::new()
        .read(true)
        .write(false)
        .create(false)
        .open(&path)?;
    Ok(BufReader::new(file))
}

/// Convenience function that calls `decode`.
pub fn decode_from_file(
    path: impl AsRef<Path>,
    output: &mut impl Write,
) -> Result<Header, DecodeError> {
    let mut reader = read_file(path)?;
    let header = decode(&mut reader, output).unwrap();
    Ok(header)
}

/// Convenience function that calls `decode`.
pub fn decode_from_file_to_vec(path: impl AsRef<Path>) -> Result<(Vec<u8>, Header), DecodeError> {
    let mut reader = read_file(path)?;
    decode_to_vec(&mut reader)
}

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

#[derive(Debug)]
pub enum DecodeError {
    IoError(io::Error),
    InvalidHeader,
    InvalidNumberOfChannels,
    InvalidColorspace,
    InvalidEofSequence,
}

impl From<io::Error> for DecodeError {
    fn from(v: io::Error) -> Self {
        Self::IoError(v)
    }
}

impl Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        Debug::fmt(self, f)
    }
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

fn msb2(x: u8) -> u8 {
    (x & 0b11000000) >> 6
}

fn lsb6(x: u8) -> u8 {
    x & 0b00111111
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Decoder {
    pub(crate) header: Header,
    pub(crate) index_array: [[u8; 4]; 64],
    pub(crate) previous_pixel: [u8; 4],
    /// Number of pixels already decoded.
    pub(crate) n_pixels: usize,
}

impl Decoder {
    pub(crate) fn new(header: Header) -> Self {
        Self {
            header,
            index_array: [[0u8; 4]; 64],
            previous_pixel: [0, 0, 0, 255],
            n_pixels: 0,
        }
    }

    /// Returns `true` if is end of byte stream.
    pub(crate) fn decode_chunk(
        &mut self,
        stream: &mut impl Read,
        output: &mut impl Write,
    ) -> Result<(), DecodeError> {
        let current_pixel = match stream.read_byte()? {
            // OP_RGB
            0b11111110 => {
                let [r, g, b] = stream.read_array()?;
                [r, g, b, self.previous_pixel[3]]
            }
            // OP_RGBA
            0b11111111 => stream.read_array::<4>()?,
            // OP_INDEX
            byte0 if msb2(byte0) == 0b00 => {
                let index = lsb6(byte0);
                self.index_array[index as usize]
            }
            // OP_DIFF
            byte0 if msb2(byte0) == 0b01 => {
                let dr = ((byte0 & 0b00_11_00_00) >> 4) as i8 - 2;
                let dg = ((byte0 & 0b00_00_11_00) >> 2) as i8 - 2;
                let db = (byte0 & 0b00_00_00_11) as i8 - 2;
                [
                    self.previous_pixel[0].wrapping_add_signed(dr),
                    self.previous_pixel[1].wrapping_add_signed(dg),
                    self.previous_pixel[2].wrapping_add_signed(db),
                    self.previous_pixel[3],
                ]
            }
            // OP_LUMA
            byte0 if msb2(byte0) == 0b10 => {
                let byte1 = stream.read_byte()?;
                let dg = lsb6(byte0) as i8 - 32;
                let dr_dg = ((byte1 & 0b11110000) >> 4) as i8 - 8;
                let db_dg = (byte1 & 0b00001111) as i8 - 8;
                [
                    self.previous_pixel[0].wrapping_add_signed(dr_dg + dg),
                    self.previous_pixel[1].wrapping_add_signed(dg),
                    self.previous_pixel[2].wrapping_add_signed(db_dg + dg),
                    self.previous_pixel[3],
                ]
            }
            // OP_RUN
            byte0 if msb2(byte0) == 0b11 => {
                let run = lsb6(byte0) + 1;
                for _ in 0..run {
                    match self.header.channels {
                        Channels::Rgb => output.write_all(&self.previous_pixel[0..=2])?,
                        Channels::Rgba => output.write_all(&self.previous_pixel)?,
                    }
                }
                self.n_pixels += run as usize;
                self.index_array[qoi_hash(self.previous_pixel)] = self.previous_pixel;
                return Ok(());
            }
            _ => unreachable!(),
        };
        self.index_array[qoi_hash(current_pixel)] = current_pixel;
        self.previous_pixel = current_pixel;
        self.n_pixels += 1;
        match self.header.channels {
            Channels::Rgb => output.write_all(&self.previous_pixel[0..=2])?,
            Channels::Rgba => output.write_all(&self.previous_pixel)?,
        }
        Ok(())
    }

    pub(crate) fn verify_eof_sequence(&mut self, bytes: &mut impl Read) -> Result<(), DecodeError> {
        if bytes.read_array::<7>()? != [0; 7] {
            return Err(DecodeError::InvalidEofSequence);
        }
        if bytes.read_byte()? != 1 {
            return Err(DecodeError::InvalidEofSequence);
        }
        Ok(())
    }
}
