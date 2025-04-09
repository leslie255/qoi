#![allow(dead_code)]

use std::{
    io::{self, BufWriter, Cursor, Write},
    iter::Peekable,
    path::Path,
};

use crate::{Channels, Header, qoi_hash};

pub fn encode(
    header: Header,
    pixels: impl Iterator<Item = [u8; 4]>,
    output: &mut impl Write,
) -> io::Result<()> {
    let mut pixels = pixels.peekable();
    let mut encoder = Encoder::new(header, output);
    encoder.encode_header()?;
    while pixels.peek().is_some() {
        encoder.encode_chunk(&mut pixels)?;
    }
    encoder.finish()?;
    Ok(())
}

pub fn encode_from_slice(header: Header, slice: &[u8], output: &mut impl Write) -> io::Result<()> {
    match header.channels {
        Channels::Rgb => {
            let pixels = array_chunks::<_, 3>(slice)
                .copied()
                .map(|[r, g, b]| [r, g, b, 255]);
            encode(header, pixels, output)
        }
        Channels::Rgba => {
            let pixels = array_chunks::<_, 4>(slice).copied();
            encode(header, pixels, output)
        }
    }
}

pub fn encode_from_slice_to_vec(header: Header, slice: &[u8]) -> io::Result<Vec<u8>> {
    let mut encoded_data = Vec::new();
    encode_from_slice(header, slice, &mut Cursor::new(&mut encoded_data))?;
    Ok(encoded_data)
}

pub fn encode_from_slice_to_file(
    header: Header,
    slice: &[u8],
    path: impl AsRef<Path>,
) -> io::Result<()> {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .read(false)
        .truncate(true)
        .create(true)
        .append(false)
        .open(path)?;
    encode_from_slice(header, slice, &mut BufWriter::new(file))
}

/// Stable implementation of `array_chunks`.
fn array_chunks<T, const N: usize>(slice: &[T]) -> impl Iterator<Item = &[T; N]> {
    slice.chunks_exact(N).map(|slice| {
        debug_assert!(slice.len() == N);
        let ptr = slice.as_ptr() as *const [T; N];
        unsafe { &*ptr }
    })
}

#[inline(always)]
pub(crate) fn u8_to_i8(x: u8) -> i8 {
    i8::from_ne_bytes([x])
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Encoder<W: Write> {
    pub(crate) header: Header,
    pub(crate) index_array: [[u8; 4]; 64],
    pub(crate) previous_pixel: [u8; 4],
    pub(crate) output: W,
    /// Temporary measure to avoid consecutive OP_INDEX encoding.
    pub(crate) last_op_was_index: bool,
}

impl<W: Write> Encoder<W> {
    pub(crate) fn new(header: Header, output: W) -> Self {
        Self {
            header,
            index_array: [[0; 4]; 64],
            previous_pixel: [0, 0, 0, 255],
            output,
            last_op_was_index: false,
        }
    }

    pub(crate) fn encode_header(&mut self) -> io::Result<()> {
        self.output.write_all(b"qoif")?;
        self.output.write_all(&self.header.width.to_be_bytes())?;
        self.output.write_all(&self.header.height.to_be_bytes())?;
        self.output
            .write_all(&(self.header.channels as u8).to_be_bytes())?;
        self.output
            .write_all(&(self.header.colorspace as u8).to_be_bytes())?;
        Ok(())
    }

    pub(crate) fn update_previous_pixel(&mut self, pixel: [u8; 4]) {
        self.previous_pixel = pixel;
        self.index_array[qoi_hash(pixel)] = pixel;
    }

    pub(crate) fn encode_chunk(
        &mut self,
        pixels: &mut Peekable<impl Iterator<Item = [u8; 4]>>,
    ) -> io::Result<()> {
        let pixel = pixels.next().unwrap();
        if let Some(byte) = self.try_run(pixel, pixels) {
            self.output.write_all(&[byte])?;
        } else if pixel[3] != self.previous_pixel[3] {
            // All other methods require currnet alpha = previous alpha.
            let bytes = self.encode_with_op_rgba(pixel);
            self.output.write_all(&bytes)?;
        } else if let Some(byte) = self.try_encode_with_op_index(pixel) {
            self.output.write_all(&[byte])?;
        } else if let Some(byte) = self.try_encode_with_op_diff(pixel) {
            self.output.write_all(&[byte])?;
        } else if let Some(bytes) = self.try_encode_with_op_luma(pixel) {
            self.output.write_all(&bytes)?;
        } else if let Some(bytes) = self.try_encode_with_op_rgb(pixel) {
            self.output.write_all(&bytes)?;
        } else {
            let bytes = self.encode_with_op_rgba(pixel);
            self.output.write_all(&bytes)?;
        }
        self.previous_pixel = pixel;
        self.index_array[qoi_hash(pixel)] = pixel;
        Ok(())
    }

    pub(crate) fn try_run(
        &mut self,
        pixel: [u8; 4],
        pixels: &mut Peekable<impl Iterator<Item = [u8; 4]>>,
    ) -> Option<u8> {
        if pixel != self.previous_pixel {
            return None;
        }
        let mut run_count = 1u8;
        while pixels.next_if_eq(&pixel).is_some() {
            run_count += 1;
            if run_count >= 62 {
                break;
            }
        }
        let byte = (0b11 << 6) | (run_count - 1);
        Some(byte)
    }

    pub(crate) fn try_encode_with_op_index(&mut self, pixel: [u8; 4]) -> Option<u8> {
        let index = qoi_hash(pixel);
        if self.index_array[index] == pixel {
            // index < 64, therefore the first 2 bits area always `00`.
            Some(index as u8)
        } else {
            None
        }
    }

    pub(crate) fn try_encode_with_op_diff(&mut self, pixel: [u8; 4]) -> Option<u8> {
        #[inline(always)]
        fn d(current: u8, previous: u8) -> Option<u8> {
            let d = u8_to_i8(current.wrapping_sub(previous));
            if (-2..=1).contains(&d) {
                Some((d + 2) as u8)
            } else {
                None
            }
        }
        let dr = d(pixel[0], self.previous_pixel[0])?;
        let dg = d(pixel[1], self.previous_pixel[1])?;
        let db = d(pixel[2], self.previous_pixel[2])?;
        let byte = (0b01 << 6) | (dr << 4) | (dg << 2) | db;
        Some(byte)
    }

    pub(crate) fn try_encode_with_op_luma(&mut self, pixel: [u8; 4]) -> Option<[u8; 2]> {
        let dr = u8_to_i8(pixel[0].wrapping_sub(self.previous_pixel[0]));
        let dg = u8_to_i8(pixel[1].wrapping_sub(self.previous_pixel[1]));
        let db = u8_to_i8(pixel[2].wrapping_sub(self.previous_pixel[2]));
        let drdg = dr.checked_sub(dg)?;
        let dbdg = db.checked_sub(dg)?;
        if (-32..=31).contains(&dg) && (-8..=7).contains(&drdg) && (-8..=7).contains(&dbdg) {
            let drdg_u4 = (drdg + 8) as u8;
            let dbdg_u4 = (dbdg + 8) as u8;
            let dg_u6 = (dg + 32) as u8;
            assert!((dg_u6 & 0b11000000) == 0);
            assert!((dbdg_u4 & 0b11110000) == 0);
            assert!((dbdg_u4 & 0b11110000) == 0);
            let byte0 = (0b10 << 6) | dg_u6;
            let byte1 = (drdg_u4 << 4) | dbdg_u4;
            Some([byte0, byte1])
        } else {
            None
        }
    }

    /// This OP would only fail for images with an alpha channel.
    pub(crate) fn try_encode_with_op_rgb(&mut self, pixel: [u8; 4]) -> Option<[u8; 4]> {
        (pixel[3] == self.previous_pixel[3]).then_some([0b11111110, pixel[0], pixel[1], pixel[2]])
    }

    /// This OP would never fail.
    pub(crate) fn encode_with_op_rgba(&mut self, pixel: [u8; 4]) -> [u8; 5] {
        [0b11111111, pixel[0], pixel[1], pixel[2], pixel[3]]
    }

    pub(crate) fn finish(mut self) -> io::Result<()> {
        self.output.write_all(&[0u8; 7])?;
        self.output.write_all(&[1])?;
        Ok(())
    }
}
