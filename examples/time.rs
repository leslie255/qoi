//! Times a decoding and encoding.

use std::{io::Cursor, time::Instant};

fn main() {
    let input_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| panic!("Expects one arguments as input file"));

    let file_data = std::fs::read(&input_path).unwrap();
    let before_decode = Instant::now();
    let (decoded_data, header) = qoi::decode_to_vec(&mut Cursor::new(&file_data)).unwrap();
    let after_decode = Instant::now();
    let encoded_data = qoi::encode_from_slice_to_vec(header, &decoded_data).unwrap();
    let after_encode = Instant::now();

    let decode_time = after_decode - before_decode;
    let encode_time = after_encode - after_decode;

    println!("decode time: {} seconds", decode_time.as_secs_f64());
    println!("encode time: {} seconds", encode_time.as_secs_f64());

    std::fs::write("roundtripped.qoi", &encoded_data).unwrap();
}
