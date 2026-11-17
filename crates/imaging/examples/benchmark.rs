//! Medição de CPU e RSS: não mede rede nem resposta do LLM.
use serde_json::json;
use std::{error::Error, path::PathBuf, time::Instant};
use stories_imaging::render::{Renderer, example_draft};

fn main() -> Result<(), Box<dyn Error>> {
    let destination = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("informe o relatório JSON de destino")?;
    let samples = 50usize;
    let start = Instant::now();
    let mut renderer = Renderer::new()?;
    let initialization = start.elapsed().as_secs_f64() * 1000.;
    let draft = example_draft();
    let start = Instant::now();
    let first = renderer.render(&draft, None, "Arte original: Breaking Bytes")?;
    let cold = start.elapsed().as_secs_f64() * 1000.;
    let mut times = Vec::with_capacity(samples);
    for _ in 0..samples {
        let start = Instant::now();
        let result = renderer.render(&draft, None, "Arte original: Breaking Bytes")?;
        std::hint::black_box(result);
        times.push(start.elapsed().as_secs_f64() * 1000.);
    }
    times.sort_by(f64::total_cmp);
    let p50 = *times.get(24).ok_or("amostras insuficientes")?;
    let p95 = *times.get(47).ok_or("amostras insuficientes")?;
    let maximum = *times.last().ok_or("amostras insuficientes")?;
    let photo = image::RgbaImage::from_fn(4000, 4000, |x, y| {
        image::Rgba([
            ((x * 13 + y * 7) % 256) as u8,
            (((x / 8) ^ (y / 8)) * 17 % 256) as u8,
            ((x * y + x * 3) % 256) as u8,
            255,
        ])
    });
    let mut photo_times = Vec::new();
    for _ in 0..20 {
        let start = Instant::now();
        let result = renderer.render(&draft, Some(&photo), "Imagem sintética de benchmark")?;
        std::hint::black_box(result);
        photo_times.push(start.elapsed().as_secs_f64() * 1000.);
    }
    photo_times.sort_by(f64::total_cmp);
    let photo_p95 = *photo_times
        .get(18)
        .ok_or("amostras de imagem insuficientes")?;
    drop(photo);
    let status = std::fs::read_to_string("/proc/self/status")?;
    let peak = status
        .lines()
        .find(|line| line.starts_with("VmHWM:"))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|v| v.parse::<u64>().ok())
        .ok_or("RSS de pico indisponível")?;
    let cpu = std::fs::read_to_string("/proc/cpuinfo")?
        .lines()
        .find_map(|line| line.strip_prefix("model name\t: "))
        .unwrap_or("não identificado")
        .to_owned();
    let report = json!({"samples":samples,"scope":"CPU: arte original, shaping, resize, composição e JPEG90; sem rede/LLM","cpu":cpu,"architecture":std::env::consts::ARCH,"profile":"release","initialization_ms":initialization,"cold_render_ms":cold,"render_p50_ms":p50,"render_p95_ms":p95,"render_max_ms":maximum,"render_16mp_p95_ms":photo_p95,"peak_rss_kib":peak,"jpeg_bytes":first.jpeg.len(),"target_render_ms":150,"target_rss_mib":300,"passed":p95<150.&&photo_p95<150.&&peak<300*1024});
    if let Some(parent) = destination.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(destination, serde_json::to_vec_pretty(&report)?)?;
    println!("{report}");
    if p95 >= 150. || photo_p95 >= 150. || peak >= 300 * 1024 {
        return Err("benchmark excedeu a meta".into());
    }
    Ok(())
}
