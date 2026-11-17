//! Geração explícita das referências; os testes nunca as atualizam.
use serde::Deserialize;
use std::{error::Error, path::PathBuf};
use stories_editorial::Draft;
use stories_imaging::render::Renderer;

#[derive(Deserialize)]
struct Case {
    name: String,
    draft: Draft,
}

fn main() -> Result<(), Box<dyn Error>> {
    let destination = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("informe diretório de destino")?;
    std::fs::create_dir_all(&destination)?;
    let cases: Vec<Case> = serde_json::from_str(include_str!("../tests/fixtures/stories.json"))?;
    let mut renderer = Renderer::new()?;
    for case in cases {
        let story = renderer
            .render(&case.draft, None, "Arte original: Breaking Bytes")
            .map_err(|error| format!("{}: {error}", case.name))?;
        std::fs::write(destination.join(format!("{}.jpg", case.name)), story.jpeg)?;
        std::fs::write(
            destination.join(format!("{}.json", case.name)),
            serde_json::to_vec_pretty(&story.layout)?,
        )?;
    }
    Ok(())
}
