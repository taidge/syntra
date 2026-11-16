use super::*;
use std::io::Write;
use zip::write::SimpleFileOptions;

fn package(part: &str, bytes: &[u8]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file(part, SimpleFileOptions::default())
        .unwrap();
    writer.write_all(bytes).unwrap();
    writer.finish().unwrap().into_inner()
}

fn relation(target: &str) -> Relationship {
    Relationship {
        kind: "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image".into(),
        target: target.into(),
        external: false,
    }
}

#[test]
fn oversized_jpeg_is_optimized_once_and_each_reuse_consumes_inline_budget() {
    let mut state = 7u32;
    let pixels = image::RgbImage::from_fn(2048, 1152, |_, _| {
        let mut channels = [0; 3];
        for channel in &mut channels {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *channel = (state >> 24) as u8;
        }
        image::Rgb(channels)
    });
    let mut source = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut source, 100)
        .encode_image(&image::DynamicImage::ImageRgb8(pixels))
        .unwrap();
    assert!(source.len() > MAX_INLINE_MEDIA_BYTES);
    let bytes = package("ppt/media/background.jpeg", &source);
    let mut archive = Archive::new(Cursor::new(bytes.as_slice())).unwrap();
    let mut importer = MediaImporter::new(1280.0, 720.0);
    let source_budget = importer.source_remaining;
    let inline_budget = importer.inline_remaining;
    let mut warnings = Vec::new();
    let rel = relation("ppt/media/background.jpeg");
    let first = media(&mut archive, &rel, &mut warnings, &mut importer).unwrap();
    assert!(warnings.is_empty());
    assert_eq!(importer.source_remaining, source_budget - source.len());
    assert_eq!(importer.inline_remaining, inline_budget - first.len());
    let decoded = STANDARD.decode(first.split_once(',').unwrap().1).unwrap();
    assert!(decoded.len() <= MAX_INLINE_MEDIA_BYTES);
    let image = image::load_from_memory(&decoded).unwrap();
    assert_eq!((image.width(), image.height()), (1280, 720));

    // Removing archive access proves reuse comes from the import cache.
    let empty_bytes = package("empty", b"");
    let mut empty = Archive::new(Cursor::new(empty_bytes.as_slice())).unwrap();
    let remaining_pixels = importer.pixels_remaining;
    let second = media(&mut empty, &rel, &mut warnings, &mut importer).unwrap();
    assert_eq!(first, second);
    assert_eq!(importer.cache.len(), 1);
    assert_eq!(importer.source_remaining, source_budget - source.len());
    assert_eq!(importer.pixels_remaining, remaining_pixels);
    assert_eq!(importer.inline_remaining, inline_budget - 2 * first.len());

    importer.inline_remaining = first.len() - 1;
    assert!(media(&mut empty, &rel, &mut warnings, &mut importer).is_none());
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("serialized editing-content budget"));
    assert!(!warnings[0].contains("unsupported"));
    assert!(!warnings[0].contains("missing"));
}

#[test]
fn raster_optimization_preserves_transparency() {
    let pixels = image::RgbaImage::from_pixel(64, 64, image::Rgba([100, 150, 200, 128]));
    let mut source = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(pixels)
        .write_to(&mut source, ImageFormat::Png)
        .unwrap();
    let mut importer = MediaImporter::new(1280.0, 720.0);
    importer.max_side = 32;
    let (mime, output) = importer.raster(source.into_inner()).unwrap();
    assert_eq!(mime, "image/png");
    let decoded = image::load_from_memory(&output).unwrap().to_rgba8();
    assert_eq!(decoded.dimensions(), (32, 32));
    assert_eq!(decoded.get_pixel(16, 16)[3], 128);
}

#[test]
fn compact_opaque_png_stays_lossless_after_resizing() {
    let pixels = image::RgbImage::from_pixel(64, 64, image::Rgb([91, 149, 203]));
    let mut source = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(pixels)
        .write_to(&mut source, ImageFormat::Png)
        .unwrap();
    let mut importer = MediaImporter::new(1280.0, 720.0);
    importer.max_side = 32;
    let (mime, output) = importer.raster(source.into_inner()).unwrap();
    assert_eq!(mime, "image/png");
    let decoded = image::load_from_memory(&output).unwrap().to_rgb8();
    assert_eq!(decoded.dimensions(), (32, 32));
    assert_eq!(decoded.get_pixel(16, 16).0, [91, 149, 203]);
}

#[test]
fn image_dimensions_and_total_decode_pixels_are_bounded() {
    let mut wide_png = Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(MAX_IMAGE_DIMENSION + 1, 1)
        .write_to(&mut wide_png, ImageFormat::Png)
        .unwrap();
    let mut importer = MediaImporter::new(1280.0, 720.0);
    assert!(
        importer
            .raster(wide_png.into_inner())
            .unwrap_err()
            .contains("safe decoding dimensions")
    );

    let mut normal_png = Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(64, 64)
        .write_to(&mut normal_png, ImageFormat::Png)
        .unwrap();
    importer.max_side = 32;
    importer.pixels_remaining = 0;
    assert!(
        importer
            .raster(normal_png.into_inner())
            .unwrap_err()
            .contains("total decoding-pixel budget")
    );
}

#[test]
fn media_failures_report_the_actual_cause() {
    let bytes = package("ppt/media/existing.png", b"invalid PNG");
    let mut archive = Archive::new(Cursor::new(bytes.as_slice())).unwrap();
    let mut importer = MediaImporter::new(1280.0, 720.0);
    let mut warnings = Vec::new();
    assert!(
        media(
            &mut archive,
            &relation("ppt/media/existing.png"),
            &mut warnings,
            &mut importer,
        )
        .is_none()
    );
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("format could not be decoded"));

    warnings.clear();
    assert!(
        media(
            &mut archive,
            &relation("ppt/media/missing.png"),
            &mut warnings,
            &mut importer,
        )
        .is_none()
    );
    assert!(warnings[0].contains("part is missing or unreadable"));

    warnings.clear();
    importer.source_remaining = 1;
    assert!(
        media(
            &mut archive,
            &relation("ppt/media/existing.png"),
            &mut warnings,
            &mut MediaImporter {
                cache: BTreeMap::new(),
                ..importer
            },
        )
        .is_none()
    );
    assert!(warnings[0].contains("total source-media budget"));
}
