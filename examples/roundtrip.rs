//! Decodes a QOI image and then re-encode it back into `roundtripped.qoi`.

fn main() {
    let input_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| panic!("Expects one arguments as input file"));

    let (decoded_data, header) = qoi::decode_from_file_to_vec(input_path).unwrap();

    qoi::encode_from_slice_to_file(header, &decoded_data, "roundtripped.qoi").unwrap();
}
