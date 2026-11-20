//! Imagens licenciadas e renderização determinística, executadas fora do runtime.

pub mod assets;
pub mod render;
pub mod validate;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ImagingError {
    #[error("configuração de imagens inválida")]
    Configuration,
    #[error("arquivo de imagem excede o limite de bytes")]
    Bytes,
    #[error("MIME declarado não corresponde ao formato permitido")]
    Mime,
    #[error("dimensões ou alocação excedem os limites")]
    Dimensions,
    #[error("imagem inválida ou não pôde ser decodificada")]
    Decode,
    #[error("imagem não possui licença, revisão ou integridade válidas")]
    Provenance,
    #[error("texto excede o template mesmo após redução de 10%; exige reescrita")]
    NeedsRewrite,
    #[error("a fonte Inter não contém todos os glifos necessários")]
    MissingGlyph,
    #[error("não foi possível inicializar ou renderizar o template")]
    Render,
}

impl ImagingError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Configuration => "image_configuration",
            Self::Bytes => "image_bytes",
            Self::Mime => "image_mime",
            Self::Dimensions => "image_dimensions",
            Self::Decode => "image_decode",
            Self::Provenance => "image_provenance",
            Self::NeedsRewrite => "render_needs_rewrite",
            Self::MissingGlyph => "render_missing_glyph",
            Self::Render => "render_failure",
        }
    }
}
