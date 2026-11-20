//! Template fixo com shaping e rasterização das três faces oficiais da Inter.

use crate::ImagingError;
use cosmic_text::{
    Align, Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Weight, Wrap,
};
use image::{Rgb, RgbImage, RgbaImage};
use serde::{Deserialize, Serialize};
use stories_editorial::Draft;

pub const WIDTH: u32 = 1080;
pub const HEIGHT: u32 = 1920;
pub const TEMPLATE: &str = "story-v1";
const TEXT_WIDTH: f32 = 952.;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Layout {
    pub title_size: f32,
    pub summary_size: f32,
    pub title_lines: usize,
    pub summary_lines: usize,
    pub image_height: u32,
    pub template: String,
}
pub struct RenderedStory {
    pub pixels: RgbImage,
    pub jpeg: Vec<u8>,
    pub layout: Layout,
}
pub struct Renderer {
    fonts: FontSystem,
    cache: SwashCache,
    resizer: fast_image_resize::Resizer,
}

struct Block {
    buffer: Buffer,
    lines: usize,
}

impl Renderer {
    pub fn new() -> Result<Self, ImagingError> {
        let mut db = fontdb::Database::new();
        for bytes in [
            include_bytes!("../../../assets/fonts/Inter-Regular.otf").as_slice(),
            include_bytes!("../../../assets/fonts/Inter-Medium.otf").as_slice(),
            include_bytes!("../../../assets/fonts/Inter-SemiBold.otf").as_slice(),
        ] {
            db.load_font_data(bytes.to_vec());
        }
        if db.faces().count() != 3 {
            return Err(ImagingError::Render);
        }
        db.set_sans_serif_family("Inter");
        Ok(Self {
            fonts: FontSystem::new_with_locale_and_db("pt-BR".into(), db),
            cache: SwashCache::new(),
            resizer: fast_image_resize::Resizer::new(),
        })
    }

    fn block(
        &mut self,
        text: &str,
        size: f32,
        line_height: f32,
        weight: Weight,
        width: f32,
        max_lines: usize,
    ) -> Result<Block, ImagingError> {
        if text.trim().is_empty() || size <= 0. || text.chars().any(|c| (c.is_control() && c != '\n')||matches!(c,'\u{061c}'|'\u{200b}'..='\u{200f}'|'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}'|'\u{feff}'))
        {
            return Err(ImagingError::NeedsRewrite);
        }
        let mut buffer = Buffer::new(&mut self.fonts, Metrics::new(size, size * line_height));
        buffer.set_size(Some(width), None);
        buffer.set_wrap(Wrap::Word);
        buffer.set_text(
            text,
            &Attrs::new().family(Family::Name("Inter")).weight(weight),
            Shaping::Advanced,
            Some(Align::Center),
        );
        buffer.shape_until_scroll(&mut self.fonts, false);
        let lines = buffer.layout_runs().count();
        if lines == 0
            || lines > max_lines
            || buffer.layout_runs().any(|run| run.line_w > width + 0.01)
        {
            return Err(ImagingError::NeedsRewrite);
        }
        if buffer
            .layout_runs()
            .any(|run| run.glyphs.iter().any(|g| g.glyph_id == 0))
        {
            return Err(ImagingError::MissingGlyph);
        }
        Ok(Block { buffer, lines })
    }

    fn fitted(
        &mut self,
        text: &str,
        size: f32,
        line_height: f32,
        weight: Weight,
        max_lines: usize,
    ) -> Result<(Block, f32), ImagingError> {
        for reduction in 0..=10 {
            let scaled = size * (1. - reduction as f32 / 100.);
            match self.block(text, scaled, line_height, weight, TEXT_WIDTH, max_lines) {
                Ok(block) => return Ok((block, scaled)),
                Err(ImagingError::NeedsRewrite) => {}
                Err(error) => return Err(error),
            }
        }
        Err(ImagingError::NeedsRewrite)
    }

    fn draw(
        &mut self,
        target: &mut RgbImage,
        block: &mut Block,
        x: i32,
        y: i32,
        color: Color,
    ) -> Result<(), ImagingError> {
        let mut outside = false;
        block.buffer.draw(
            &mut self.fonts,
            &mut self.cache,
            color,
            |left, top, width, height, ink| {
                for dy in 0..height {
                    for dx in 0..width {
                        let px = x + left + dx as i32;
                        let py = y + top + dy as i32;
                        if px < 0
                            || py < 0
                            || px >= target.width() as i32
                            || py >= target.height() as i32
                        {
                            outside = true;
                            continue;
                        }
                        let pixel = target.get_pixel_mut(px as u32, py as u32);
                        let alpha = u32::from(ink.a());
                        for (channel, value) in pixel.0.iter_mut().zip([ink.r(), ink.g(), ink.b()])
                        {
                            *channel = ((u32::from(value) * alpha
                                + u32::from(*channel) * (255 - alpha)
                                + 127)
                                / 255) as u8;
                        }
                    }
                }
            },
        );
        if outside {
            return Err(ImagingError::NeedsRewrite);
        }
        Ok(())
    }

    pub fn original_art(&mut self, draft: &Draft) -> Result<RgbaImage, ImagingError> {
        let mut canvas = tiny_skia::Pixmap::new(1200, 900).ok_or(ImagingError::Render)?;
        canvas.fill(tiny_skia::Color::from_rgba8(12, 18, 26, 255));
        let mut paint = tiny_skia::Paint::default();
        for row in 0..9 {
            for column in 0..12 {
                let alpha = if (row + column) % 3 == 0 { 55 } else { 22 };
                paint.set_color_rgba8(46, 233, 164, alpha);
                let rect = tiny_skia::Rect::from_xywh(
                    column as f32 * 100. + 18.,
                    row as f32 * 100. + 18.,
                    64.,
                    64.,
                )
                .ok_or(ImagingError::Render)?;
                canvas.fill_rect(rect, &paint, tiny_skia::Transform::identity(), None);
            }
        }
        paint.set_color_rgba8(12, 18, 26, 240);
        canvas.fill_rect(
            tiny_skia::Rect::from_xywh(64., 170., 1072., 560.).ok_or(ImagingError::Render)?,
            &paint,
            tiny_skia::Transform::identity(),
            None,
        );
        let rgba = RgbaImage::from_raw(1200, 900, canvas.take()).ok_or(ImagingError::Render)?;
        let mut rgb = image::DynamicImage::ImageRgba8(rgba).into_rgb8();
        let mut brand = self.block("BREAKING BYTES", 30., 1.2, Weight::MEDIUM, 1072., 1)?;
        self.draw(&mut rgb, &mut brand, 64, 110, Color::rgb(46, 233, 164))?;
        let mut headline = self.block(&draft.title, 56., 1.15, Weight::SEMIBOLD, 1008., 5)?;
        let headline_height = headline.lines as f32 * 64.4;
        self.draw(
            &mut rgb,
            &mut headline,
            96,
            (450. - headline_height / 2.) as i32,
            Color::rgb(255, 255, 255),
        )?;
        let mut caption = self.block(
            "TECNOLOGIA EM TEMPO REAL",
            24.,
            1.2,
            Weight::MEDIUM,
            1072.,
            1,
        )?;
        self.draw(&mut rgb, &mut caption, 64, 770, Color::rgb(138, 138, 138))?;
        Ok(image::DynamicImage::ImageRgb8(rgb).into_rgba8())
    }

    pub fn render(
        &mut self,
        draft: &Draft,
        image: Option<&RgbaImage>,
        credit: &str,
    ) -> Result<RenderedStory, ImagingError> {
        if self.cache.image_cache.len() > 4096 {
            self.cache = SwashCache::new();
        }
        if draft.title.chars().count() > 160
            || draft.title.split_whitespace().count() > 8
            || !(2..=4).contains(&draft.summary.len())
            || draft.summary.iter().any(|line| {
                line.is_empty() || line.chars().count() > 100 || line.chars().any(char::is_control)
            })
            || draft.source.chars().count() > 80
            || credit.chars().count() > 100
            || !matches!(draft.kind.as_str(), "new" | "update" | "correction")
        {
            return Err(ImagingError::NeedsRewrite);
        }
        let (mut title, title_size) = self.fitted(&draft.title, 64., 1.1, Weight::SEMIBOLD, 3)?;
        let (mut summary, summary_size) =
            self.fitted(&draft.summary.join("\n"), 40., 1.35, Weight::NORMAL, 4)?;
        if summary.lines < 2 {
            return Err(ImagingError::NeedsRewrite);
        }
        let owned;
        let source = if let Some(image) = image {
            image
        } else {
            owned = self.original_art(draft)?;
            &owned
        };
        if source.width() < 1000
            || source.height() == 0
            || u64::from(source.width()) * u64::from(source.height()) > 16_000_000
        {
            return Err(ImagingError::Dimensions);
        }
        let image_height =
            ((u64::from(source.height()) * 760 / u64::from(source.width())).min(570) as u32).max(1);
        let mut scaled = RgbaImage::new(760, image_height);
        self.resizer
            .resize(
                source,
                &mut scaled,
                &fast_image_resize::ResizeOptions::new().fit_into_destination(None),
            )
            .map_err(|_| ImagingError::Render)?;
        let mut pixels = RgbImage::from_pixel(WIDTH, HEIGHT, Rgb([0, 0, 0]));
        for (x, y, source) in scaled.enumerate_pixels() {
            let dx = if x < 12 {
                12 - x
            } else {
                x.saturating_sub(747)
            };
            let dy = if y < 12 {
                12 - y
            } else {
                y.saturating_sub(image_height.saturating_sub(13))
            };
            if dx * dx + dy * dy > 144 {
                continue;
            }
            let alpha = u32::from(source.0[3]);
            let target = pixels.get_pixel_mut(160 + x, 300 + y);
            for (to, from) in target.0.iter_mut().zip(source.0.iter()) {
                *to = (u32::from(*from) * alpha / 255) as u8;
            }
        }
        let mut credit_block = self.block(credit, 20., 1.2, Weight::NORMAL, TEXT_WIDTH, 1)?;
        self.draw(
            &mut pixels,
            &mut credit_block,
            64,
            884,
            Color::rgb(138, 138, 138),
        )?;
        let label = match draft.kind.as_str() {
            "update" => Some("ATUALIZAÇÃO"),
            "correction" => Some("CORREÇÃO"),
            _ => None,
        };
        if let Some(label) = label {
            let mut block = self.block(label, 28., 1.1, Weight::MEDIUM, TEXT_WIDTH, 1)?;
            self.draw(&mut pixels, &mut block, 64, 928, Color::rgb(208, 208, 208))?;
        }
        self.draw(&mut pixels, &mut title, 64, 980, Color::rgb(255, 255, 255))?;
        self.draw(
            &mut pixels,
            &mut summary,
            64,
            1230,
            Color::rgb(208, 208, 208),
        )?;
        let mut source = self.block(
            &format!("Fonte: {}", draft.source),
            28.,
            1.2,
            Weight::NORMAL,
            TEXT_WIDTH,
            1,
        )?;
        let baseline = source
            .buffer
            .layout_runs()
            .next()
            .ok_or(ImagingError::Render)?
            .line_y;
        self.draw(
            &mut pixels,
            &mut source,
            64,
            (1640. - baseline).round() as i32,
            Color::rgb(138, 138, 138),
        )?;
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90)
            .encode_image(&pixels)
            .map_err(|_| ImagingError::Render)?;
        if jpeg.len() > 8 * 1024 * 1024 {
            return Err(ImagingError::Bytes);
        }
        Ok(RenderedStory {
            pixels,
            jpeg,
            layout: Layout {
                title_size,
                summary_size,
                title_lines: title.lines,
                summary_lines: summary.lines,
                image_height,
                template: TEMPLATE.into(),
            },
        })
    }
}

pub fn example_draft() -> Draft {
    Draft {
        title: "Rust lança versão 1.100".into(),
        summary: vec![
            "A versão estável chega com correções.".into(),
            "Os detalhes estão no anúncio oficial.".into(),
        ],
        source: "Rust Blog".into(),
        kind: "new".into(),
        entities: vec!["Rust".into()],
        language: "pt-BR".into(),
    }
}
