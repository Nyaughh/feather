//! SVG images, including the ones the stock loader turns away.
//!
//! egui's own SVG loader only accepts addresses that end in `.svg`. README
//! badges usually don't (`https://img.shields.io/badge/...`), so they showed
//! up as error icons. This loader goes by what the data is rather than what
//! the address looks like, and it can draw the text inside an SVG, which is
//! most of what a badge is.

use eframe::egui::{
    self, ColorImage,
    load::{BytesPoll, ImageLoadResult, ImageLoader, ImagePoll, LoadError, SizeHint},
    mutex::Mutex,
};
use std::collections::HashMap;
use std::sync::Arc;

type Cached = Result<Arc<ColorImage>, String>;

pub struct SvgLoader {
    cache: Mutex<HashMap<(String, SizeHint), Cached>>,
    options: resvg::usvg::Options<'static>,
}

impl SvgLoader {
    pub fn new() -> Self {
        let mut options = resvg::usvg::Options::default();
        // Text is drawn with the app's own font. Loading every font on the
        // system instead would add a noticeable pause at startup.
        let fonts = options.fontdb_mut();
        fonts.load_font_data(include_bytes!("../assets/fonts/Inter-Regular.ttf").to_vec());
        fonts.load_font_data(include_bytes!("../assets/fonts/Inter-SemiBold.ttf").to_vec());
        for generic in [
            resvg::usvg::fontdb::Database::set_sans_serif_family,
            resvg::usvg::fontdb::Database::set_serif_family,
            resvg::usvg::fontdb::Database::set_monospace_family,
            resvg::usvg::fontdb::Database::set_cursive_family,
            resvg::usvg::fontdb::Database::set_fantasy_family,
        ] {
            generic(fonts, "Inter");
        }
        options.font_family = "Inter".to_owned();
        Self {
            cache: Mutex::new(HashMap::new()),
            options,
        }
    }
}

/// Whether these bytes are an SVG document, whatever the address says.
fn looks_like_svg(uri: &str, mime: Option<&str>, bytes: &[u8]) -> bool {
    if egui::load::has_extension(uri, "svg") || mime.is_some_and(|m| m.contains("svg")) {
        return true;
    }
    // Skip a byte-order mark and whitespace, then an XML prolog or comments.
    let head = &bytes[..bytes.len().min(1024)];
    let text = String::from_utf8_lossy(head);
    let text = text.trim_start_matches('\u{feff}').trim_start();
    text.starts_with("<svg")
        || (text.starts_with("<?xml") || text.starts_with("<!--")) && text.contains("<svg")
}

impl ImageLoader for SvgLoader {
    fn id(&self) -> &str {
        "feather::SvgLoader"
    }

    fn load(&self, ctx: &egui::Context, uri: &str, size_hint: SizeHint) -> ImageLoadResult {
        let key = (uri.to_owned(), size_hint);
        if let Some(cached) = self.cache.lock().get(&key) {
            return match cached {
                Ok(image) => Ok(ImagePoll::Ready {
                    image: image.clone(),
                }),
                Err(error) => Err(LoadError::Loading(error.clone())),
            };
        }
        match ctx.try_load_bytes(uri) {
            Ok(BytesPoll::Ready { bytes, mime, .. }) => {
                if !looks_like_svg(uri, mime.as_deref(), &bytes) {
                    return Err(LoadError::NotSupported);
                }
                let result =
                    egui_extras::image::load_svg_bytes_with_size(&bytes, size_hint, &self.options)
                        .map(Arc::new);
                self.cache.lock().insert(key, result.clone());
                match result {
                    Ok(image) => Ok(ImagePoll::Ready { image }),
                    Err(error) => Err(LoadError::Loading(error)),
                }
            }
            Ok(BytesPoll::Pending { size }) => Ok(ImagePoll::Pending { size }),
            Err(error) => Err(error),
        }
    }

    fn forget(&self, uri: &str) {
        self.cache.lock().retain(|(cached, _), _| cached != uri);
    }

    fn forget_all(&self) {
        self.cache.lock().clear();
    }

    fn byte_size(&self) -> usize {
        self.cache
            .lock()
            .values()
            .map(|cached| match cached {
                Ok(image) => image.pixels.len() * size_of::<egui::Color32>(),
                Err(error) => error.len(),
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BADGE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="90" height="20" role="img">
        <rect width="90" height="20" rx="3" fill="#555"/>
        <text x="45" y="14" fill="#fff" text-anchor="middle" font-family="Verdana,Geneva,DejaVu Sans,sans-serif" font-size="11">Next.js</text>
    </svg>"##;

    #[test]
    fn recognises_svg_by_content_not_address() {
        let badge = "https://img.shields.io/badge/Next.js-15-black";
        assert!(looks_like_svg(
            badge,
            Some("image/svg+xml;charset=utf-8"),
            b""
        ));
        assert!(looks_like_svg(badge, None, BADGE.as_bytes()));
        assert!(looks_like_svg(
            badge,
            None,
            b"<?xml version=\"1.0\"?>\n<svg></svg>"
        ));
        assert!(looks_like_svg("file:///a/logo.svg", None, b""));
        assert!(!looks_like_svg(badge, Some("image/png"), b"\x89PNG\r\n"));
        assert!(!looks_like_svg(
            badge,
            None,
            b"<html><body>Not found</body></html>"
        ));
    }

    #[test]
    fn draws_badge_text_with_the_bundled_font() {
        let loader = SvgLoader::new();
        let render = |svg: &str| {
            egui_extras::image::load_svg_bytes_with_size(
                svg.as_bytes(),
                SizeHint::default(),
                &loader.options,
            )
            .expect("valid svg")
        };
        let with_text = render(BADGE);
        let without_text = render(&BADGE.replace("Next.js", ""));
        assert_eq!(with_text.size, [90, 20]);
        // The label is asked for in fonts we don't have; it must still be drawn.
        assert_ne!(with_text.pixels, without_text.pixels);
    }
}
