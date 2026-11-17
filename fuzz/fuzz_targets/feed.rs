#![no_main]
//! Parsers não executam rede; entradas são limitadas como no coletor real.
use libfuzzer_sys::fuzz_target;
fuzz_target!(|data: &[u8]| {
    if data.len() <= 2 * 1024 * 1024 {
        let _ = stories_collectors::parsers::feed(data, 200);
        let _ = stories_collectors::parsers::sitemap(data, 200);
        let _ = stories_collectors::parsers::github(data, 200);
        let _ = stories_collectors::parsers::status(data, 200);
        let _ = stories_collectors::parsers::hackernews(data);
    }
});
