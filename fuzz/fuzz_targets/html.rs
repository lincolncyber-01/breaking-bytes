#![no_main]
//! HTML é dado; o alvo não segue URL, canonical ou instrução da matéria.
use libfuzzer_sys::fuzz_target;
fuzz_target!(|data: &str| {
    if data.len() <= 2 * 1024 * 1024 {
        let _ = stories_normalize::plain_text(data, 4000);
        if let Ok(base) = url::Url::parse("https://example.com/news") {
            let _ = stories_normalize::html_metadata(&base, data);
            let _ = stories_normalize::article_revision(&base, data);
        }
    }
});
