use criterion::Criterion;
use std::{error::Error, hint::black_box};
use stories_imaging::render::{Renderer, example_draft};

fn main() -> Result<(), Box<dyn Error>> {
    let mut criterion = Criterion::default().configure_from_args();
    let mut renderer = Renderer::new()?;
    let draft = example_draft();
    renderer.render(&draft, None, "Arte original: Breaking Bytes")?;
    let mut failed = false;
    criterion.bench_function("story_original_art_jpeg90", |b| {
        b.iter(|| {
            let result = renderer.render(black_box(&draft), None, "Arte original: Breaking Bytes");
            failed |= result.is_err();
            black_box(result)
        })
    });
    let image = image::RgbaImage::from_fn(4000, 4000, |x, y| {
        image::Rgba([
            ((x * 13 + y * 7) % 256) as u8,
            (((x / 8) ^ (y / 8)) * 17 % 256) as u8,
            ((x * y + x * 3) % 256) as u8,
            255,
        ])
    });
    criterion.bench_function("story_decoded_image_16mp_jpeg90", |b| {
        b.iter(|| {
            let result = renderer.render(
                black_box(&draft),
                Some(black_box(&image)),
                "Imagem sintética de benchmark",
            );
            failed |= result.is_err();
            black_box(result)
        })
    });
    criterion.final_summary();
    if failed {
        return Err("render falhou durante o benchmark".into());
    }
    Ok(())
}
