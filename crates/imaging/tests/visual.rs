use serde::Deserialize;
use stories_editorial::Draft;
use stories_imaging::{
    ImagingError,
    render::{Renderer, example_draft},
};

#[derive(Deserialize)]
struct Case {
    name: String,
    draft: Draft,
}

#[test]
fn golden_stories_match_decoded_pixels_and_safe_areas() -> Result<(), Box<dyn std::error::Error>> {
    let cases: Vec<Case> = serde_json::from_str(include_str!("fixtures/stories.json"))?;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let mut renderer = Renderer::new()?;
    for case in cases {
        let result = renderer.render(&case.draft, None, "Arte original: Breaking Bytes")?;
        let golden = image::open(root.join(format!("{}.jpg", case.name)))?.into_rgb8();
        let actual = image::load_from_memory(&result.jpeg)?.into_rgb8();
        assert_eq!(actual.dimensions(), (1080, 1920));
        assert_eq!(golden.dimensions(), actual.dimensions());
        let differences = actual
            .pixels()
            .zip(golden.pixels())
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(differences, 0, "snapshot {}", case.name);
        let expected_layout: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join(format!("{}.json", case.name)))?)?;
        let actual_layout: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&result.layout)?)?;
        assert_eq!(actual_layout, expected_layout);
        insta::assert_json_snapshot!(format!("layout_{}", case.name), &result.layout);
        assert!(result.layout.title_lines <= 3);
        assert!((2..=4).contains(&result.layout.summary_lines));
        assert!((57.6..=64.).contains(&result.layout.title_size));
        assert!((36.0..=40.).contains(&result.layout.summary_size));
        if case.name == "shrink" {
            assert!(result.layout.title_size < 64.);
        }
        for (x, y, pixel) in result.pixels.enumerate_pixels() {
            if !(64..1016).contains(&x) || !(250..1670).contains(&y) {
                assert_eq!(pixel.0, [0, 0, 0], "{}: {x},{y}", case.name);
            }
        }
    }
    Ok(())
}

#[test]
fn overflow_missing_glyph_and_invalid_content_never_get_clipped()
-> Result<(), Box<dyn std::error::Error>> {
    let mut renderer = Renderer::new()?;
    let tiny = image::RgbaImage::new(999, 900);
    assert!(matches!(
        renderer.render(&example_draft(), Some(&tiny), "Arte original"),
        Err(ImagingError::Dimensions)
    ));
    let mut draft = example_draft();
    draft.title = "W".repeat(60);
    assert!(matches!(
        renderer.render(&draft, None, "Arte original"),
        Err(ImagingError::NeedsRewrite)
    ));
    draft.title = "Rust 🚀".into();
    assert!(matches!(
        renderer.render(&draft, None, "Arte original"),
        Err(ImagingError::MissingGlyph)
    ));
    draft.title = "Rust \u{202e}1.100".into();
    assert!(matches!(
        renderer.render(&draft, None, "Arte original"),
        Err(ImagingError::NeedsRewrite)
    ));
    draft = example_draft();
    draft.summary = vec!["linha".into()];
    assert!(renderer.render(&draft, None, "Arte original").is_err());
    draft = example_draft();
    draft.source = "Fonte\nmaliciosa".into();
    assert!(renderer.render(&draft, None, "Arte original").is_err());
    Ok(())
}

#[test]
fn licensed_photo_preserves_aspect_alpha_and_rounded_corners()
-> Result<(), Box<dyn std::error::Error>> {
    let mut renderer = Renderer::new()?;
    let photo = image::RgbaImage::from_pixel(1600, 900, image::Rgba([200, 100, 50, 128]));
    let story = renderer.render(
        &example_draft(),
        Some(&photo),
        "Imagem de teste / uso próprio",
    )?;
    assert_eq!(story.layout.image_height, 427);
    assert_eq!(story.pixels.get_pixel(160, 300).0, [0, 0, 0]);
    assert_eq!(story.pixels.get_pixel(919, 726).0, [0, 0, 0]);
    assert_eq!(story.pixels.get_pixel(540, 750).0, [0, 0, 0]);
    for (actual, expected) in story
        .pixels
        .get_pixel(540, 500)
        .0
        .iter()
        .zip([100u8, 50, 25])
    {
        assert!(actual.abs_diff(expected) <= 2);
    }
    assert!(!story.jpeg.windows(6).any(|bytes| bytes == b"Exif\0\0"));
    Ok(())
}
