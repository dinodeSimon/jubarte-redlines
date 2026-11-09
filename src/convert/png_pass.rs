// SPDX-FileCopyrightText: 2026 Jandira Technologies, LLC
//
// SPDX-License-Identifier: AGPL-3.0-only

//! PNG pictures in the PDF without holding their pixels.
//!
//! A PDF reads a PNG's own compressed image data: `/FlateDecode` with
//! `/DecodeParms << /Predictor 15 … >>` undoes the PNG row filters. So an
//! 8-bit, non-interlaced PNG without transparency (gray, RGB or a palette)
//! goes into the PDF as its IDAT bytes, never decoded ([`PngPass`]); layout
//! needs only its size. A PNG with transparency is decoded a row at a time
//! straight into the color samples and the `/SMask` plane ([`rows_with_alpha`]),
//! never through a full RGBA image and a copy of it.
//!
//! Both are taken only when the file is certainly one the `image` crate
//! would decode the same way: every chunk's CRC, the chunk order, every row's
//! filter byte, every palette index and the exact inflated length are checked
//! here, and the `image` crate's default allocation limit is kept. Anything
//! else goes the way it always went (`image::load_from_memory`).

use std::io::Read;

use super::pdf::Pixels;

const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
/// The `image` crate's default allocation limit (`Limits::default`): a
/// picture it would refuse to decode is not passed through either.
const IMAGE_ALLOC_LIMIT: u64 = 512 * 1024 * 1024;

/// The PDF color space of a passed-through PNG.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PngColor {
    Gray,
    Rgb,
    /// A palette of RGB triples, `/Indexed /DeviceRGB`.
    Indexed(Vec<u8>),
}

/// An 8-bit, non-interlaced PNG without transparency, kept as its file: the
/// PDF is given its IDAT bytes as they are.
#[derive(Clone, Debug)]
pub(crate) struct PngPass {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) color: PngColor,
    /// The whole PNG file, shared by every paint of the picture.
    pub(crate) source: Pixels,
    /// Where each IDAT chunk's data is in `source`, in order.
    idat: Vec<(usize, usize)>,
}

impl PngPass {
    /// The components a pixel has in the compressed data (`/Colors`).
    pub(crate) fn colors(&self) -> u8 {
        match self.color {
            PngColor::Rgb => 3,
            PngColor::Gray | PngColor::Indexed(_) => 1,
        }
    }

    /// The image data as the PDF stream takes it: the IDAT chunks' data, joined.
    pub(crate) fn data(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.idat.iter().map(|(_, len)| len).sum());
        for &(at, len) in &self.idat {
            out.extend_from_slice(&self.source[at..at + len]);
        }
        out
    }
}

/// One chunk: its type and where its data is.
struct Chunk<'a> {
    kind: &'a [u8; 4],
    at: usize,
    data: &'a [u8],
}

/// The file's chunks, each with a correct CRC, up to and including IEND with
/// nothing after it; `None` for anything else.
fn chunks(bytes: &[u8]) -> Option<Vec<Chunk<'_>>> {
    if bytes.get(..8)? != SIGNATURE {
        return None;
    }
    let mut out = Vec::new();
    let mut at = 8;
    loop {
        let len = u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?) as usize;
        let kind: &[u8; 4] = bytes.get(at + 4..at + 8)?.try_into().ok()?;
        let data = bytes.get(at + 8..at.checked_add(8 + len)?)?;
        let crc = u32::from_be_bytes(bytes.get(at + 8 + len..at + 12 + len)?.try_into().ok()?);
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(kind);
        hasher.update(data);
        if hasher.finalize() != crc {
            return None;
        }
        out.push(Chunk {
            kind,
            at: at + 8,
            data,
        });
        at += 12 + len;
        if kind == b"IEND" {
            return (at == bytes.len()).then_some(out);
        }
    }
}

/// The IHDR fields: width, height, bit depth, color type, interlace.
fn header(chunks: &[Chunk<'_>]) -> Option<(u32, u32, u8, u8, u8)> {
    let first = chunks.first()?;
    if first.kind != b"IHDR" || first.data.len() != 13 {
        return None;
    }
    let d = first.data;
    let width = u32::from_be_bytes(d[0..4].try_into().ok()?);
    let height = u32::from_be_bytes(d[4..8].try_into().ok()?);
    // Compression and filter method 0 are the only ones PNG defines.
    (width > 0 && height > 0 && d[10] == 0 && d[11] == 0)
        .then_some((width, height, d[8], d[9], d[12]))
}

/// A PNG to pass through to the PDF, or `None` when it has transparency, is
/// not 8-bit, is interlaced, or is anything but certainly well formed.
pub(crate) fn pass_through(bytes: &Pixels) -> Option<PngPass> {
    let chunks = chunks(bytes)?;
    let (width, height, depth, color_type, interlace) = header(&chunks)?;
    if depth != 8 || interlace != 0 || chunks.iter().any(|c| c.kind == b"tRNS") {
        return None;
    }
    let palette = chunks.iter().find(|c| c.kind == b"PLTE");
    let color = match (color_type, palette) {
        (0, _) => PngColor::Gray,
        (2, _) => PngColor::Rgb,
        (3, Some(p)) if !p.data.is_empty() && p.data.len() % 3 == 0 && p.data.len() <= 768 => {
            PngColor::Indexed(p.data.to_vec())
        }
        _ => return None,
    };
    // The IDAT chunks follow one another, with nothing between them.
    let first = chunks.iter().position(|c| c.kind == b"IDAT")?;
    let count = chunks[first..]
        .iter()
        .take_while(|c| c.kind == b"IDAT")
        .count();
    if chunks[first + count..].iter().any(|c| c.kind == b"IDAT")
        || palette.is_some_and(|p| (p.at as isize) > (chunks[first].at as isize))
    {
        return None;
    }
    let channels: u64 = if color_type == 2 { 3 } else { 1 };
    // What the `image` crate would allocate to decode it (it decodes to RGB).
    if u64::from(width) * u64::from(height) * 3 > IMAGE_ALLOC_LIMIT {
        return None;
    }
    let idat: Vec<(usize, usize)> = chunks[first..first + count]
        .iter()
        .map(|c| (c.at, c.data.len()))
        .collect();
    let palette_len = match &color {
        PngColor::Indexed(p) => Some(p.len() / 3),
        _ => None,
    };
    let row = 1 + u64::from(width) * channels;
    let readable = Rows {
        idat: idat.iter().map(|&(at, len)| &bytes[at..at + len]).collect(),
    };
    rows_are_whole(readable, row, height, palette_len)?;
    Some(PngPass {
        width,
        height,
        color,
        source: bytes.clone(),
        idat,
    })
}

/// The IDAT chunks' data, read as one stream.
struct Rows<'a> {
    idat: Vec<&'a [u8]>,
}

impl Read for Rows<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        while let Some(first) = self.idat.first_mut() {
            if first.is_empty() {
                self.idat.remove(0);
                continue;
            }
            let n = first.len().min(buf.len());
            buf[..n].copy_from_slice(&first[..n]);
            *first = &first[n..];
            return Ok(n);
        }
        Ok(0)
    }
}

/// The compressed data inflates, a buffer at a time, to exactly `height` rows
/// of `row` bytes, each opening with a filter PNG defines (0 to 4; for a
/// palette, 0 only) and, for a palette, naming only entries it has.
fn rows_are_whole(idat: Rows<'_>, row: u64, height: u32, palette: Option<usize>) -> Option<()> {
    let mut inflate = flate2::read::ZlibDecoder::new(idat);
    let mut buf = vec![0u8; 64 * 1024];
    let total = row.checked_mul(u64::from(height))?;
    let mut seen: u64 = 0;
    loop {
        let n = inflate.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        for &byte in &buf[..n] {
            if seen >= total {
                return None;
            }
            let column = seen % row;
            // A filtered row's bytes are differences, not indices, so a palette
            // PNG passes through only when every row is unfiltered (filter 0)
            // and every index names an entry it has.
            let refused = match (column, palette) {
                (0, None) => byte > 4,
                (0, Some(_)) => byte != 0,
                (_, Some(entries)) => usize::from(byte) >= entries,
                (_, None) => false,
            };
            if refused {
                return None;
            }
            seen += 1;
        }
    }
    (seen == total).then_some(())
}

/// An 8-bit, non-interlaced PNG with transparency, decoded a row at a time
/// into its color samples (RGB) and its alpha plane: what the PDF writer
/// takes as the image and its `/SMask`. `None` for any other PNG, which goes
/// the `image` crate's way.
pub(crate) fn rows_with_alpha(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>, Vec<u8>)> {
    let chunks = chunks(bytes)?;
    let (width, height, depth, color_type, interlace) = header(&chunks)?;
    let transparent = matches!(color_type, 4 | 6) || chunks.iter().any(|c| c.kind == b"tRNS");
    if depth != 8 || interlace != 0 || !transparent {
        return None;
    }
    if u64::from(width) * u64::from(height) * 4 > IMAGE_ALLOC_LIMIT {
        return None;
    }
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND);
    decoder.set_limits(png::Limits {
        bytes: IMAGE_ALLOC_LIMIT as usize,
    });
    let mut reader = decoder.read_info().ok()?;
    let (out_color, out_depth) = reader.output_color_type();
    if out_depth != png::BitDepth::Eight {
        return None;
    }
    let n = (width as usize) * (height as usize);
    let mut rgb = Vec::with_capacity(n * 3);
    let mut alpha = Vec::with_capacity(n);
    while let Some(row) = reader.next_row().ok()? {
        let data = row.data();
        match out_color {
            png::ColorType::Rgba => {
                for px in data.as_chunks::<4>().0 {
                    rgb.extend_from_slice(&px[..3]);
                    alpha.push(px[3]);
                }
            }
            png::ColorType::GrayscaleAlpha => {
                for px in data.as_chunks::<2>().0 {
                    rgb.extend_from_slice(&[px[0], px[0], px[0]]);
                    alpha.push(px[1]);
                }
            }
            _ => return None,
        }
    }
    (rgb.len() == n * 3 && alpha.len() == n).then_some((width, height, rgb, alpha))
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    /// `pass_through` for a file's bytes.
    fn pass(bytes: &[u8]) -> Option<PngPass> {
        pass_through(&std::sync::Arc::new(bytes.to_vec()))
    }

    /// A PNG of these chunks after IHDR, written byte by byte (so a test says
    /// exactly what the file holds), its image data the given filtered rows.
    fn png_file(
        (width, height, depth, color_type, interlace): (u32, u32, u8, u8, u8),
        before_idat: &[(&[u8; 4], Vec<u8>)],
        rows: &[u8],
    ) -> Vec<u8> {
        let chunk = |out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]| {
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            out.extend_from_slice(kind);
            out.extend_from_slice(data);
            let mut crc = crc32fast::Hasher::new();
            crc.update(kind);
            crc.update(data);
            out.extend_from_slice(&crc.finalize().to_be_bytes());
        };
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[depth, color_type, 0, 0, interlace]);
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(rows).unwrap();
        let idat = z.finish().unwrap();
        let mut out = SIGNATURE.to_vec();
        chunk(&mut out, b"IHDR", &ihdr);
        for (kind, data) in before_idat {
            chunk(&mut out, kind, data);
        }
        // Two IDAT chunks, so the joining of chunks is exercised.
        let (a, b) = idat.split_at(idat.len() / 2);
        chunk(&mut out, b"IDAT", a);
        chunk(&mut out, b"IDAT", b);
        chunk(&mut out, b"IEND", &[]);
        out
    }

    /// Rows of `width` pixels of `channels` bytes, each filtered with the
    /// filter its index picks (0 to 4 in turn), from samples made by `pixel`.
    fn filtered(
        width: u32,
        height: u32,
        channels: usize,
        pixel: impl Fn(u32, u32, usize) -> u8,
    ) -> Vec<u8> {
        let row_len = width as usize * channels;
        let mut prev = vec![0u8; row_len];
        let mut out = Vec::new();
        for y in 0..height {
            let raw: Vec<u8> = (0..row_len)
                .map(|i| pixel(i as u32 / channels as u32, y, i % channels))
                .collect();
            let filter = (y % 5) as u8;
            out.push(filter);
            for i in 0..row_len {
                let a = if i >= channels { raw[i - channels] } else { 0 };
                let b = prev[i];
                let c = if i >= channels { prev[i - channels] } else { 0 };
                let predicted = match filter {
                    0 => 0,
                    1 => a,
                    2 => b,
                    3 => ((u16::from(a) + u16::from(b)) / 2) as u8,
                    _ => paeth(a, b, c),
                };
                out.push(raw[i].wrapping_sub(predicted));
            }
            prev = raw;
        }
        out
    }

    fn paeth(a: u8, b: u8, c: u8) -> u8 {
        let p = i16::from(a) + i16::from(b) - i16::from(c);
        let (pa, pb, pc) = (
            (p - i16::from(a)).abs(),
            (p - i16::from(b)).abs(),
            (p - i16::from(c)).abs(),
        );
        if pa <= pb && pa <= pc {
            a
        } else if pb <= pc {
            b
        } else {
            c
        }
    }

    /// What a PDF reader makes of a passed-through PNG's stream (`/FlateDecode`
    /// with `/Predictor 15`): inflate, then undo each row's filter. As RGB.
    fn as_pdf_reader_reads(png: &PngPass) -> Vec<u8> {
        let mut inflated = Vec::new();
        flate2::read::ZlibDecoder::new(&png.data()[..])
            .read_to_end(&mut inflated)
            .unwrap();
        let channels = usize::from(png.colors());
        let row_len = png.width as usize * channels;
        let mut prev = vec![0u8; row_len];
        let mut samples = Vec::new();
        for row in inflated.chunks(row_len + 1) {
            let mut cur = vec![0u8; row_len];
            for i in 0..row_len {
                let a = if i >= channels { cur[i - channels] } else { 0 };
                let b = prev[i];
                let c = if i >= channels { prev[i - channels] } else { 0 };
                let predicted = match row[0] {
                    0 => 0,
                    1 => a,
                    2 => b,
                    3 => ((u16::from(a) + u16::from(b)) / 2) as u8,
                    _ => paeth(a, b, c),
                };
                cur[i] = row[1 + i].wrapping_add(predicted);
            }
            samples.extend_from_slice(&cur);
            prev = cur;
        }
        match &png.color {
            PngColor::Rgb => samples,
            PngColor::Gray => samples.iter().flat_map(|&g| [g, g, g]).collect(),
            PngColor::Indexed(p) => samples
                .iter()
                .flat_map(|&i| p[usize::from(i) * 3..usize::from(i) * 3 + 3].to_vec())
                .collect(),
        }
    }

    /// What the `image` crate decodes, as RGB: what the PDF held before.
    fn as_image_decodes(bytes: &[u8]) -> Vec<u8> {
        image::load_from_memory(bytes)
            .unwrap()
            .into_rgb8()
            .into_raw()
    }

    #[test]
    fn a_large_rgb_png_goes_through_undecoded_and_reads_as_the_image_crate_decodes_it() {
        // 3000 x 2000 RGB: 18 MB of samples, which the PDF used to hold
        // decoded (and copied); passed through, the picture is its file.
        let (w, h) = (3000, 2000);
        let rows = filtered(w, h, 3, |x, y, c| {
            ((x * (c as u32 + 1) + y * 3) % 251) as u8
        });
        let bytes = png_file((w, h, 8, 2, 0), &[], &rows);
        let png = pass(&bytes).expect("an 8-bit RGB PNG passes through");
        assert_eq!((png.width, png.height, png.colors()), (w, h, 3));
        assert_eq!(png.source.len(), bytes.len());
        assert!(png.source.len() < (w * h * 3) as usize / 4);
        assert_eq!(as_pdf_reader_reads(&png), as_image_decodes(&bytes));
        let Some(super::super::ImageKind::Png(kept)) = super::super::decode_image(bytes.clone())
        else {
            panic!("the picture is kept as its file, not decoded");
        };
        assert_eq!(&kept.source[..], &bytes[..]);
    }

    #[test]
    fn gray_and_palette_pngs_go_through_in_their_own_color_spaces() {
        let gray = png_file(
            (7, 5, 8, 0, 0),
            &[],
            &filtered(7, 5, 1, |x, y, _| (x * 30 + y * 7) as u8),
        );
        let png = pass(&gray).unwrap();
        assert_eq!(png.color, PngColor::Gray);
        assert_eq!(as_pdf_reader_reads(&png), as_image_decodes(&gray));

        let palette = vec![0, 0, 0, 255, 0, 0, 0, 128, 255, 250, 250, 250];
        let unfiltered: Vec<u8> = (0..4u32)
            .flat_map(|y| std::iter::once(0).chain((0..6u32).map(move |x| ((x + y) % 4) as u8)))
            .collect();
        let indexed = png_file((6, 4, 8, 3, 0), &[(b"PLTE", palette.clone())], &unfiltered);
        let png = pass(&indexed).unwrap();
        assert_eq!(png.color, PngColor::Indexed(palette));
        assert_eq!(as_pdf_reader_reads(&png), as_image_decodes(&indexed));
    }

    #[test]
    fn anything_but_a_certainly_whole_opaque_8_bit_png_takes_the_old_way() {
        let rgb = |w, h| filtered(w, h, 3, |x, y, c| (x + y + c as u32) as u8);
        let fine = png_file((4, 3, 8, 2, 0), &[], &rgb(4, 3));
        assert!(pass(&fine).is_some());
        let refused = |what: &str, bytes: &[u8]| assert!(pass(bytes).is_none(), "{what}");
        refused(
            "a transparent color",
            &png_file(
                (4, 3, 8, 2, 0),
                &[(b"tRNS", vec![0, 1, 0, 2, 0, 3])],
                &rgb(4, 3),
            ),
        );
        refused(
            "an alpha channel",
            &png_file((4, 3, 8, 6, 0), &[], &filtered(4, 3, 4, |_, _, _| 9)),
        );
        refused(
            "16-bit",
            &png_file((4, 3, 16, 2, 0), &[], &filtered(4, 3, 6, |_, _, _| 9)),
        );
        refused("interlaced", &png_file((4, 3, 8, 2, 1), &[], &rgb(4, 3)));
        refused("a row short", &png_file((4, 3, 8, 2, 0), &[], &rgb(4, 2)));
        refused("a row over", &png_file((4, 3, 8, 2, 0), &[], &rgb(4, 4)));
        let mut bad_filter = rgb(4, 3);
        bad_filter[0] = 5;
        refused(
            "a filter PNG doesn't define",
            &png_file((4, 3, 8, 2, 0), &[], &bad_filter),
        );
        let mut bad_crc = fine.clone();
        let last = bad_crc.len() - 13;
        bad_crc[last] ^= 1;
        refused("a chunk whose CRC is wrong", &bad_crc);
        let mut trailing = fine.clone();
        trailing.push(0);
        refused("bytes after IEND", &trailing);
        refused("not a PNG", b"GIF89a");
        let palette = vec![0, 0, 0, 255, 255, 255];
        refused(
            "a filtered palette row",
            &png_file((2, 1, 8, 3, 0), &[(b"PLTE", palette.clone())], &[1, 0, 1]),
        );
        refused(
            "an index past the palette",
            &png_file((2, 1, 8, 3, 0), &[(b"PLTE", palette)], &[0, 0, 2]),
        );
        refused(
            "a palette PNG with no palette",
            &png_file((2, 1, 8, 3, 0), &[], &[0, 0, 0]),
        );
        // What the `image` crate takes, the PDF still gets, the old way.
        assert!(matches!(
            super::super::decode_image(png_file((4, 3, 8, 2, 1), &[], &rgb(4, 3))),
            None | Some(super::super::ImageKind::Rgb { .. })
        ));
    }

    #[test]
    fn a_picture_shown_in_many_places_is_decoded_once_for_its_layout() {
        use super::super::{DecodedPictures, ImageKind, decode_image};
        let bytes = png_file(
            (6, 4, 8, 6, 0),
            &[],
            &filtered(6, 4, 4, |x, y, c| (x * 9 + y + c as u32) as u8),
        );
        let samples = |kind: Option<ImageKind>| match kind {
            Some(ImageKind::Rgb { bytes, .. }) => bytes,
            _ => panic!("a transparent PNG decodes to samples"),
        };
        // Outside a layout nothing is kept: each decode is its own.
        let (a, b) = (
            samples(decode_image(bytes.clone())),
            samples(decode_image(bytes.clone())),
        );
        assert!(!std::sync::Arc::ptr_eq(&a, &b));
        {
            let _layout = DecodedPictures::open();
            let first = samples(decode_image(bytes.clone()));
            let again = samples(decode_image(bytes.clone()));
            assert!(std::sync::Arc::ptr_eq(&first, &again));
            assert_eq!(first, a);
            // A layout inside it (a header laid out apart) keeps its own, and
            // gives the outer one back when it ends.
            {
                let _inner = DecodedPictures::open();
                assert!(!std::sync::Arc::ptr_eq(
                    &samples(decode_image(bytes.clone())),
                    &first
                ));
            }
            assert!(std::sync::Arc::ptr_eq(
                &samples(decode_image(bytes.clone())),
                &first
            ));
        }
        // The layout's pictures are let go when it ends.
        assert!(!std::sync::Arc::ptr_eq(
            &samples(decode_image(bytes.clone())),
            &a
        ));
    }

    #[test]
    fn a_transparent_png_decoded_a_row_at_a_time_matches_the_image_crate() {
        let check = |what: &str, bytes: Vec<u8>| {
            let (w, h, rgb, alpha) = rows_with_alpha(&bytes).unwrap_or_else(|| panic!("{what}"));
            let rgba = image::load_from_memory(&bytes).unwrap().into_rgba8();
            assert_eq!((w, h), rgba.dimensions(), "{what}");
            let (want_rgb, want_alpha): (Vec<[u8; 3]>, Vec<u8>) = rgba
                .pixels()
                .map(|p| ([p.0[0], p.0[1], p.0[2]], p.0[3]))
                .unzip();
            assert_eq!(rgb, want_rgb.concat(), "{what}");
            assert_eq!(alpha, want_alpha, "{what}");
        };
        check(
            "RGBA",
            png_file(
                (5, 4, 8, 6, 0),
                &[],
                &filtered(5, 4, 4, |x, y, c| (x * 40 + y * 9 + c as u32 * 50) as u8),
            ),
        );
        check(
            "gray and alpha",
            png_file(
                (5, 4, 8, 4, 0),
                &[],
                &filtered(5, 4, 2, |x, y, c| (x * 50 + y + c as u32 * 99) as u8),
            ),
        );
        check(
            "RGB with a transparent color",
            png_file(
                (3, 2, 8, 2, 0),
                &[(b"tRNS", vec![0, 1, 0, 2, 0, 3])],
                &filtered(3, 2, 3, |x, _, c| (x + c as u32) as u8),
            ),
        );
        check(
            "a palette with transparent entries",
            png_file(
                (3, 1, 8, 3, 0),
                &[
                    (b"PLTE", vec![10, 20, 30, 40, 50, 60]),
                    (b"tRNS", vec![0, 128]),
                ],
                &[0, 0, 1, 0],
            ),
        );
        assert!(
            rows_with_alpha(&png_file(
                (3, 2, 8, 2, 0),
                &[],
                &filtered(3, 2, 3, |_, _, _| 1)
            ))
            .is_none()
        );
    }
}
