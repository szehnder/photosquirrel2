//! TIFF fixtures (tiff-support change): layouts and directory placement, checked with the `tiff` crate.

use std::io::Cursor;

use ps_fixtures::tiff::Placement;
use ps_fixtures::tiff_image::{Alpha, Colour, Compression, Layout, Sample, TiffSpec, tiff_image};
use tiff::decoder::{Decoder, DecodingResult};
use tiff::tags::Tag;

fn u32_le(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

#[test]
fn scenario_test_corpus_end_directory_tiff_generated() {
    let base = TiffSpec::new(300, 200, 1);
    let variants = [
        base.clone(),
        TiffSpec {
            layout: Layout::Tiles { size: 64 },
            ..base.clone()
        },
        TiffSpec {
            planar: true,
            sample: Sample::U16,
            ..base.clone()
        },
        TiffSpec {
            sample: Sample::F32,
            colour: Colour::Rgba(Alpha::Opaque),
            ..base.clone()
        },
        TiffSpec {
            colour: Colour::Cmyk,
            compression: Compression::Lzw,
            ..base.clone()
        },
        TiffSpec {
            colour: Colour::Gray,
            sample: Sample::U16,
            compression: Compression::Deflate,
            predictor: true,
            ..base.clone()
        },
        TiffSpec {
            colour: Colour::YCbCrJpeg,
            layout: Layout::Tiles { size: 128 },
            ..base.clone()
        },
        TiffSpec {
            pages: vec![(64, 48)],
            levels: vec![(150, 100)],
            ..base.clone()
        },
    ];
    for (i, spec) in variants.iter().enumerate() {
        let t = tiff_image(spec);
        let ifd0 = u32_le(&t, 4) as u64;
        let mut dec = Decoder::new(Cursor::new(&t)).expect("independent decoder");
        assert_eq!(dec.dimensions().expect("dims"), (300, 200), "variant {i}");
        let (off_tag, len_tag) = if dec.find_tag(Tag::TileOffsets).expect("tag").is_some() {
            (Tag::TileOffsets, Tag::TileByteCounts)
        } else {
            (Tag::StripOffsets, Tag::StripByteCounts)
        };
        let offs = dec.get_tag_u64_vec(off_tag).expect("offsets");
        let lens = dec.get_tag_u64_vec(len_tag).expect("lengths");
        let data_end = offs
            .iter()
            .zip(&lens)
            .map(|(o, l)| o + l)
            .max()
            .unwrap_or(0);
        assert!(
            ifd0 >= data_end,
            "variant {i}: IFD0 at {ifd0}, data ends at {data_end}"
        );
        // Every chunk decodes.
        for c in 0..offs.len() as u32 {
            dec.read_chunk(c).expect("chunk decodes");
        }
    }
    // The samples are the reference picture's (8-bit RGB, first strip, first row).
    let t = tiff_image(&base);
    let mut dec = Decoder::new(Cursor::new(&t)).expect("decoder");
    let DecodingResult::U8(row) = dec.read_chunk(0).expect("strip") else {
        panic!("8-bit samples")
    };
    assert_eq!(&row[..300 * 3], &base.reference()[..300 * 3]);
    // Directories first puts IFD0 right after the header.
    let first = tiff_image(&TiffSpec {
        placement: Placement::DirectoriesFirst,
        ..base
    });
    assert_eq!(u32_le(&first, 4), 8);
}
