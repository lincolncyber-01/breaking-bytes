use chrono::Utc;
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use proptest::prelude::*;
use serde_json::json;
use std::io::Cursor;
use stories_imaging::{
    ImagingError,
    assets::Catalog,
    validate::{Limits, decode},
};

fn png(width: u32, height: u32) -> Result<Vec<u8>, image::ImageError> {
    let mut output = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(RgbaImage::from_pixel(
        width,
        height,
        Rgba([20, 30, 40, 255]),
    ))
    .write_to(&mut output, ImageFormat::Png)?;
    Ok(output.into_inner())
}

#[test]
fn magic_dimensions_allocation_and_corruption_are_checked_before_use()
-> Result<(), Box<dyn std::error::Error>> {
    let image = png(1200, 900)?;
    assert_eq!(
        decode(&image, "image/png; binary", Limits::default())?.dimensions(),
        (1200, 900)
    );
    assert!(matches!(
        decode(&image, "image/jpeg", Limits::default()),
        Err(ImagingError::Mime)
    ));
    assert!(matches!(
        decode(&image, "image/svg+xml", Limits::default()),
        Err(ImagingError::Mime)
    ));
    assert!(matches!(
        decode(
            &image,
            "image/png",
            Limits {
                max_bytes: 1024,
                max_pixels: 16_000_000
            }
        ),
        Err(ImagingError::Bytes)
    ));
    assert!(matches!(
        decode(
            &image,
            "image/png",
            Limits {
                max_bytes: 8 * 1024 * 1024,
                max_pixels: 1_000_000
            }
        ),
        Err(ImagingError::Dimensions)
    ));
    assert!(matches!(
        decode(&png(999, 900)?, "image/png", Limits::default()),
        Err(ImagingError::Dimensions)
    ));
    assert!(decode(&image[..32], "image/png", Limits::default()).is_err());
    // Um IHDR com CRC válido pode anunciar uma bomba mesmo com corpo muito pequeno.
    let mut bomb = image.clone();
    bomb[16..20].copy_from_slice(&12000u32.to_be_bytes());
    bomb[20..24].copy_from_slice(&12000u32.to_be_bytes());
    let mut crc = !0u32;
    for byte in &bomb[12..29] {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb88320
            } else {
                crc >> 1
            };
        }
    }
    bomb[29..33].copy_from_slice(&(!crc).to_be_bytes());
    assert!(matches!(
        decode(&bomb, "image/png", Limits::default()),
        Err(ImagingError::Dimensions)
    ));
    Ok(())
}

#[test]
fn exif_orientation_is_applied_then_metadata_is_discarded() -> Result<(), Box<dyn std::error::Error>>
{
    let image = image::RgbImage::from_pixel(1200, 1100, image::Rgb([10, 20, 30]));
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 80).encode_image(&image)?;
    let exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
    let mut oriented = vec![0xff, 0xd8, 0xff, 0xe1];
    oriented.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
    oriented.extend_from_slice(exif);
    oriented.extend_from_slice(&jpeg[2..]);
    assert_eq!(
        decode(&oriented, "image/jpeg", Limits::default())?.dimensions(),
        (1100, 1200)
    );
    Ok(())
}

fn manifest() -> serde_json::Value {
    json!([{"url":"https://images.example.com/rust.png","role":"primary","article_url":"https://example.com/news","entities":["Rust"],"credit":"Equipe Rust","license":"owned","rights_url":"https://example.com/license","blake3":blake3::hash(b"image").to_hex().to_string(),"review":{"reviewer":"Revisor de teste","reviewed_at":Utc::now()-chrono::Duration::days(1),"watermark_free":true,"relevant":true,"generic":false}}])
}

#[test]
fn license_review_domain_relevance_and_exact_hash_are_required()
-> Result<(), Box<dyn std::error::Error>> {
    let value = manifest();
    let entities = vec!["Rust".into()];
    let catalog = Catalog::parse(
        &serde_json::to_vec(&value)?,
        vec!["images.example.com".into()],
    )?;
    let assets = catalog.candidates("https://example.com/news", &entities, None, Utc::now());
    assert_eq!(assets.len(), 1);
    assets[0].verify_bytes(b"image")?;
    assert!(assets[0].verify_bytes(b"changed").is_err());
    assert!(
        catalog
            .candidates(
                "https://example.com/unrelated",
                &["Linux".into()],
                None,
                Utc::now()
            )
            .is_empty()
    );
    let unlisted = Catalog::parse(&serde_json::to_vec(&value)?, vec![])?;
    assert!(
        unlisted
            .candidates("https://example.com/news", &entities, None, Utc::now())
            .is_empty()
    );
    for change in 0..5 {
        let mut value = value.clone();
        match change {
            0 => value[0]["review"]["watermark_free"] = json!(false),
            1 => value[0]["review"]["relevant"] = json!(false),
            2 => value[0]["review"]["generic"] = json!(true),
            3 => {
                value[0]["review"]["reviewed_at"] = json!(Utc::now() - chrono::Duration::days(181))
            }
            _ => value[0]["review"]["reviewed_at"] = json!(Utc::now() + chrono::Duration::days(1)),
        }
        let catalog = Catalog::parse(
            &serde_json::to_vec(&value)?,
            vec!["images.example.com".into()],
        )?;
        assert!(
            catalog
                .candidates("https://example.com/news", &entities, None, Utc::now())
                .is_empty()
        );
    }
    for change in 0..4 {
        let mut value = value.clone();
        match change {
            0 => value[0]["url"] = json!("https://images.example.com/file?token=secret"),
            1 => value[0]["license"] = json!("unknown"),
            2 => value[0]["blake3"] = json!("invalid"),
            _ => value[0]["credit"] = json!("credit\ninvalid"),
        }
        assert!(Catalog::parse(&serde_json::to_vec(&value)?, vec![]).is_err());
    }
    Ok(())
}

#[test]
fn source_preference_and_og_identity_are_enforced() -> Result<(), Box<dyn std::error::Error>> {
    use stories_imaging::assets::Role;
    let value = manifest();
    let mut assets = Vec::new();
    for role in ["library", "og", "primary", "press_kit"] {
        let mut asset = value[0].clone();
        asset["role"] = json!(role);
        asset["url"] = json!(format!("https://images.example.com/{role}.png"));
        assets.push(asset);
    }
    let catalog = Catalog::parse(
        &serde_json::to_vec(&assets)?,
        vec!["images.example.com".into()],
    )?;
    let candidates = catalog.candidates(
        "https://example.com/news",
        &["Rust".into()],
        Some("https://images.example.com/og.png"),
        Utc::now(),
    );
    assert_eq!(
        candidates.iter().map(|a| a.role).collect::<Vec<_>>(),
        vec![Role::PressKit, Role::Primary, Role::Og, Role::Library]
    );
    let candidates = catalog.candidates(
        "https://example.com/news",
        &["Rust".into()],
        Some("https://images.example.com/different.png"),
        Utc::now(),
    );
    assert!(candidates.iter().all(|a| a.role != Role::Og));
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]
    #[test]
    fn arbitrary_bytes_do_not_panic_and_obey_limits(bytes in proptest::collection::vec(any::<u8>(),0..2048)) {
        if let Ok(image)=decode(&bytes,"image/png",Limits::default()) {prop_assert!(image.width()>=1000);prop_assert!(u64::from(image.width())*u64::from(image.height())<=16_000_000);}
    }
    #[test]
    fn any_content_change_invalidates_the_review(bytes in proptest::collection::vec(any::<u8>(),1..2048)) {
        let value=manifest();let catalog=Catalog::parse(&serde_json::to_vec(&value).map_err(|e|TestCaseError::fail(e.to_string()))?,vec!["images.example.com".into()]).map_err(|e|TestCaseError::fail(e.to_string()))?;
        let assets=catalog.candidates("https://example.com/news",&["Rust".into()],None,Utc::now());
        prop_assume!(bytes!=b"image");prop_assert!(assets[0].verify_bytes(&bytes).is_err());
    }
}
