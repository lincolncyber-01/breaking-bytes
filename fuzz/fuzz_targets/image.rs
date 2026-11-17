#![no_main]
//! Decodificação com limites de bytes e pixels, sem renderer ou acesso ao sistema.
use libfuzzer_sys::fuzz_target;
fuzz_target!(|data: &[u8]| {
    if data.len() <= 2 * 1024 * 1024 {
        for mime in ["image/jpeg", "image/png", "image/webp"] {
            let _ = stories_imaging::validate::decode(
                data,
                mime,
                stories_imaging::validate::Limits {
                    max_bytes: 2 * 1024 * 1024,
                    max_pixels: 16_000_000,
                },
            );
        }
    }
});
