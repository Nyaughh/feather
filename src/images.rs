//! Every image the app shows is loaded here.
//!
//! egui's stock loaders give up easily, and a README pays for it with a red
//! warning icon: an SVG badge whose address does not end in `.svg`, a GIF (egui
//! tags those addresses with a frame number the file loader then cannot find),
//! `logo.png?raw=true`, a path from the repository root, or a link to the
//! picture's page on GitHub rather than to the picture. This loader works out
//! where the image really is, goes by what the data is rather than what the
//! address looks like, decodes off the UI thread, and, when an image truly
//! cannot be had, shows a quiet placeholder instead of an error.

use eframe::egui::{
    self, ColorImage, Vec2,
    load::{BytesPoll, ImageLoadResult, ImageLoader, ImagePoll, SizeHint},
    mutex::Mutex,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Marks a display size carried in an image's address. See [`sized`].
const SIZE_MARK: &str = "#feather:";

/// Appends the size an `<img>` tag asked for to its address, where the loader
/// will find it. Markdown itself has no way to say how big an image should be.
pub fn sized(address: &str, width: Option<u32>, height: Option<u32>) -> String {
    match (width, height) {
        (Some(w), Some(h)) => format!("{address}{SIZE_MARK}w={w},h={h}"),
        (Some(w), None) => format!("{address}{SIZE_MARK}w={w}"),
        (None, Some(h)) => format!("{address}{SIZE_MARK}h={h}"),
        (None, None) => address.to_owned(),
    }
}

/// The size an address asks for, in points, and the address without it.
fn wanted_size(uri: &str) -> (&str, Option<u32>, Option<u32>) {
    let Some((address, mark)) = uri.rsplit_once(SIZE_MARK) else {
        return (uri, None, None);
    };
    let (mut width, mut height) = (None, None);
    for part in mark.split(',') {
        match part.split_once('=') {
            Some(("w", v)) => width = v.parse().ok().filter(|v| *v > 0),
            Some(("h", v)) => height = v.parse().ok().filter(|v| *v > 0),
            _ => {}
        }
    }
    (address, width, height)
}

/// What addresses are resolved against: the open repository.
#[derive(Default)]
struct Scope {
    root: PathBuf,
    /// `owner/name` on GitHub, lowercase, when the repository lives there.
    github: Option<String>,
}

impl Scope {
    /// Turns the address a document gave into one that can be fetched.
    fn resolve(&self, uri: &str) -> String {
        if let Some(path) = uri.strip_prefix("file://") {
            return format!("file://{}", self.local(path));
        }
        if let Some(found) = self.github_file(uri) {
            return found;
        }
        uri.to_owned()
    }

    fn local(&self, path: &str) -> String {
        // "?raw=true" and the like mean something to GitHub, not to a disk.
        let path = if Path::new(path).exists() {
            path
        } else {
            path.split(['?', '#']).next().unwrap_or(path)
        };
        // The viewer puts the document's folder in front of every relative
        // address, so one that began with "/" now has "//" in it. GitHub reads
        // those from the top of the repository.
        match path.split_once("//") {
            Some((_, rest)) if !self.root.as_os_str().is_empty() => {
                self.root.join(rest).display().to_string()
            }
            _ => path.to_owned(),
        }
    }

    /// A link to a file on GitHub, as the file itself: straight from disk when
    /// it is in this repository (which also works for private ones), otherwise
    /// from GitHub's raw file host.
    fn github_file(&self, uri: &str) -> Option<String> {
        let rest = uri
            .strip_prefix("https://")
            .or_else(|| uri.strip_prefix("http://"))?;
        let rest = rest.split(['?', '#']).next()?;
        let (host, path) = rest.split_once('/')?;
        let mut parts = path.splitn(3, '/');
        let (owner, name, tail) = (parts.next()?, parts.next()?, parts.next()?);
        let (page, tail) = match host {
            "github.com" | "www.github.com" => {
                let (kind, tail) = tail.split_once('/')?;
                if kind != "blob" && kind != "raw" {
                    return None;
                }
                (kind == "blob", tail)
            }
            "raw.githubusercontent.com" => (false, tail),
            _ => return None,
        };
        let tail = tail.strip_prefix("refs/heads/").unwrap_or(tail);
        let slug = format!("{owner}/{name}").to_lowercase();
        if self.github.as_deref() == Some(slug.as_str()) {
            // `tail` is "<branch>/<path>", and a branch name can contain
            // slashes, so try every split.
            let mut candidate = tail;
            while let Some((_, shorter)) = candidate.split_once('/') {
                let decoded = percent_decode(shorter);
                let file = self.root.join(decoded.as_ref());
                if file.is_file() {
                    return Some(format!("file://{}", file.display()));
                }
                candidate = shorter;
            }
        }
        page.then(|| format!("https://raw.githubusercontent.com/{owner}/{name}/{tail}"))
    }
}

/// `%20` and friends, as the bytes they stand for.
fn percent_decode(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains('%') {
        return text.into();
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push((high * 16 + low) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned().into()
}

enum Raster {
    Decoding,
    Ready(Arc<ColorImage>),
    /// Could not be fetched or decoded. Drawn as the placeholder.
    Failed,
}

pub struct Loader {
    scope: Mutex<Scope>,
    /// Photos and screenshots, by address.
    rasters: Arc<Mutex<HashMap<String, Raster>>>,
    /// SVGs, which are drawn afresh for each size they are shown at.
    vectors: Mutex<HashMap<(String, SizeHint), Arc<ColorImage>>>,
    placeholders: Mutex<HashMap<SizeHint, Arc<ColorImage>>>,
    options: resvg::usvg::Options<'static>,
}

const PLACEHOLDER: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="150" height="28">
    <rect x="0.5" y="0.5" width="149" height="27" rx="6" fill="#888" fill-opacity="0.1" stroke="#888" stroke-opacity="0.35"/>
    <text x="75" y="18" text-anchor="middle" font-size="12" fill="#8a8a8a">Image unavailable</text>
</svg>"##;

impl Loader {
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
            scope: Mutex::default(),
            rasters: Arc::default(),
            vectors: Mutex::default(),
            placeholders: Mutex::default(),
            options,
        }
    }

    /// Tells the loader which repository is open, so it can find images that
    /// are addressed from its root or by their GitHub link.
    pub fn set_repository(&self, root: &Path, github: Option<String>) {
        *self.scope.lock() = Scope {
            root: root.to_owned(),
            github: github.map(|slug| slug.to_lowercase()),
        };
        // What an address resolves to may have changed.
        self.rasters.lock().clear();
        self.vectors.lock().clear();
    }

    fn placeholder(&self, size_hint: SizeHint) -> ImageLoadResult {
        let mut cache = self.placeholders.lock();
        let image = match cache.get(&size_hint) {
            Some(image) => image.clone(),
            None => {
                let image = egui_extras::image::load_svg_bytes_with_size(
                    PLACEHOLDER.as_bytes(),
                    size_hint,
                    &self.options,
                )
                .unwrap_or_else(|_| ColorImage::filled([1, 1], egui::Color32::TRANSPARENT));
                let image = Arc::new(image);
                cache.insert(size_hint, image.clone());
                image
            }
        };
        Ok(ImagePoll::Ready { image })
    }

    fn fail(&self, uri: &str, size_hint: SizeHint) -> ImageLoadResult {
        self.rasters.lock().insert(uri.to_owned(), Raster::Failed);
        self.placeholder(size_hint)
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

/// The pixel size to decode to and the point size to show at, given the size
/// the document asked for (if any) and the largest texture the GPU takes.
fn fit(
    original: [u32; 2],
    width: Option<u32>,
    height: Option<u32>,
    pixels_per_point: f32,
    max_side: u32,
) -> ([u32; 2], Option<Vec2>) {
    let [w, h] = [original[0].max(1) as f32, original[1].max(1) as f32];
    let shown = match (width, height) {
        (Some(width), Some(height)) => Some(Vec2::new(width as f32, height as f32)),
        (Some(width), None) => Some(Vec2::new(width as f32, width as f32 * h / w)),
        (None, Some(height)) => Some(Vec2::new(height as f32 * w / h, height as f32)),
        (None, None) => None,
    };
    // Never enlarge: decode to what the screen will show, or less.
    let scale = shown.map_or(1.0, |shown| {
        (shown.x * pixels_per_point / w).max(shown.y * pixels_per_point / h)
    });
    let scale = scale
        .min(max_side as f32 / w)
        .min(max_side as f32 / h)
        .min(1.0);
    let pixels = [
        ((w * scale).round() as u32).max(1),
        ((h * scale).round() as u32).max(1),
    ];
    (pixels, shown)
}

fn decode(
    bytes: &[u8],
    width: Option<u32>,
    height: Option<u32>,
    pixels_per_point: f32,
    max_side: u32,
) -> Result<ColorImage, String> {
    let image = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    let original = [image.width(), image.height()];
    let (pixels, shown) = fit(original, width, height, pixels_per_point, max_side);
    let image = if pixels == original {
        image
    } else {
        image.resize_exact(pixels[0], pixels[1], image::imageops::FilterType::Triangle)
    };
    let rgba = image.to_rgba8();
    let mut image =
        ColorImage::from_rgba_unmultiplied([pixels[0] as usize, pixels[1] as usize], &rgba);
    if let Some(shown) = shown {
        image.source_size = shown;
    }
    Ok(image)
}

impl ImageLoader for Loader {
    fn id(&self) -> &str {
        "feather::images::Loader"
    }

    fn load(&self, ctx: &egui::Context, uri: &str, size_hint: SizeHint) -> ImageLoadResult {
        // egui adds "#<frame>" to the address of anything that could be animated.
        let uri = egui::decode_animated_image_uri(uri).map_or(uri, |(uri, _)| uri);
        match self.rasters.lock().get(uri) {
            Some(Raster::Ready(image)) => {
                return Ok(ImagePoll::Ready {
                    image: image.clone(),
                });
            }
            Some(Raster::Decoding) => return Ok(ImagePoll::Pending { size: None }),
            Some(Raster::Failed) => return self.placeholder(size_hint),
            None => {}
        }
        let vector_key = (uri.to_owned(), size_hint);
        if let Some(image) = self.vectors.lock().get(&vector_key) {
            return Ok(ImagePoll::Ready {
                image: image.clone(),
            });
        }

        let (address, width, height) = wanted_size(uri);
        let source = self.scope.lock().resolve(address);
        let (bytes, mime) = match ctx.try_load_bytes(&source) {
            Ok(BytesPoll::Ready { bytes, mime, .. }) => (bytes, mime),
            Ok(BytesPoll::Pending { size }) => return Ok(ImagePoll::Pending { size }),
            Err(_) => return self.fail(uri, size_hint),
        };

        if looks_like_svg(&source, mime.as_deref(), &bytes) {
            let hint = match (width, height) {
                (Some(width), _) => {
                    SizeHint::Width((width as f32 * ctx.pixels_per_point()).round() as u32)
                }
                (None, Some(height)) => {
                    SizeHint::Height((height as f32 * ctx.pixels_per_point()).round() as u32)
                }
                (None, None) => size_hint,
            };
            return match egui_extras::image::load_svg_bytes_with_size(&bytes, hint, &self.options) {
                Ok(mut image) => {
                    if width.is_some() || height.is_some() {
                        // Show it at the size asked for, however many pixels that took.
                        let [w, h] = image.size;
                        image.source_size = Vec2::new(w as f32, h as f32) / ctx.pixels_per_point();
                    }
                    let image = Arc::new(image);
                    self.vectors.lock().insert(vector_key, image.clone());
                    Ok(ImagePoll::Ready { image })
                }
                Err(_) => self.fail(uri, size_hint),
            };
        }

        // Decoding a large screenshot takes long enough to notice: not on this thread.
        self.rasters.lock().insert(uri.to_owned(), Raster::Decoding);
        let rasters = self.rasters.clone();
        let key = uri.to_owned();
        let ctx = ctx.clone();
        let pixels_per_point = ctx.pixels_per_point();
        let max_side = ctx.input(|i| i.max_texture_side).min(8192) as u32;
        std::thread::spawn(move || {
            let result = decode(&bytes, width, height, pixels_per_point, max_side);
            let mut rasters = rasters.lock();
            // Unless it was forgotten in the meantime.
            if let Some(slot) = rasters.get_mut(&key) {
                *slot = match result {
                    Ok(image) => Raster::Ready(Arc::new(image)),
                    Err(_) => Raster::Failed,
                };
            }
            drop(rasters);
            ctx.request_repaint();
        });
        Ok(ImagePoll::Pending { size: None })
    }

    fn forget(&self, uri: &str) {
        let same = |cached: &str| cached == uri || wanted_size(cached).0 == uri;
        self.rasters.lock().retain(|cached, _| !same(cached));
        self.vectors.lock().retain(|(cached, _), _| !same(cached));
    }

    fn forget_all(&self) {
        self.rasters.lock().clear();
        self.vectors.lock().clear();
    }

    fn byte_size(&self) -> usize {
        let size = |image: &ColorImage| image.pixels.len() * size_of::<egui::Color32>();
        let rasters: usize = self
            .rasters
            .lock()
            .values()
            .map(|raster| match raster {
                Raster::Ready(image) => size(image),
                Raster::Decoding | Raster::Failed => 0,
            })
            .sum();
        let vectors: usize = self.vectors.lock().values().map(|image| size(image)).sum();
        rasters + vectors
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
        let loader = Loader::new();
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

    #[test]
    fn finds_images_the_way_github_does() {
        let dir = std::env::temp_dir().join(format!("feather-images-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("docs/my shots")).unwrap();
        std::fs::write(dir.join("docs/logo.png"), b"x").unwrap();
        std::fs::write(dir.join("docs/my shots/a.png"), b"x").unwrap();
        let scope = Scope {
            root: dir.clone(),
            github: Some("nyaughh/feather".to_owned()),
        };
        // Windows joins paths with a backslash and opens either kind, so the
        // two are the same address there.
        let root = dir.display().to_string();
        let file = |rel: &str| format!("file://{root}/{rel}").replace('\\', "/");
        let resolve = |uri: &str| resolve(uri).replace('\\', "/");

        // Plain relative paths are left alone.
        assert_eq!(resolve(&file("docs/logo.png")), file("docs/logo.png"));
        // Query strings are dropped.
        assert_eq!(
            resolve(&file("docs/logo.png?raw=true")),
            file("docs/logo.png")
        );
        // "/docs/logo.png" written in docs/guide.md means the repository's docs folder.
        assert_eq!(
            resolve(&format!("file://{root}/docs//docs/logo.png")),
            file("docs/logo.png")
        );
        // Links to this repository on GitHub come from disk, whatever the branch.
        for link in [
            "https://github.com/Nyaughh/feather/blob/main/docs/logo.png",
            "https://github.com/Nyaughh/feather/raw/feat/new-ui/docs/logo.png?raw=true",
            "https://raw.githubusercontent.com/nyaughh/feather/refs/heads/main/docs/logo.png",
        ] {
            assert_eq!(resolve(link), file("docs/logo.png"), "{link}");
        }
        assert_eq!(
            resolve("https://github.com/Nyaughh/feather/blob/main/docs/my%20shots/a.png"),
            file("docs/my shots/a.png")
        );
        // Someone else's file page becomes the file.
        assert_eq!(
            scope.resolve("https://github.com/rust-lang/rust/blob/master/logo.png?raw=true"),
            "https://raw.githubusercontent.com/rust-lang/rust/master/logo.png"
        );
        // Everything else passes through.
        for other in [
            "https://img.shields.io/badge/a-b-green",
            "https://github.com/user-attachments/assets/1234",
            "https://raw.githubusercontent.com/rust-lang/rust/master/logo.png",
        ] {
            assert_eq!(scope.resolve(other), other);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sizes_from_img_tags_survive_the_trip_through_markdown() {
        assert_eq!(wanted_size("file:///a.png"), ("file:///a.png", None, None));
        let address = sized("https://x.dev/a.png", Some(120), None);
        assert_eq!(
            wanted_size(&address),
            ("https://x.dev/a.png", Some(120), None)
        );
        let address = sized("a.gif", Some(40), Some(20));
        // egui's frame number comes after ours and is taken off first.
        let framed = format!("{address}#0");
        let (unframed, _) = egui::decode_animated_image_uri(&framed).unwrap();
        assert_eq!(wanted_size(unframed), ("a.gif", Some(40), Some(20)));
    }

    #[test]
    fn decodes_to_the_size_shown_and_never_enlarges() {
        // A 1000px logo shown 100 points wide on a 2x screen needs 200 pixels.
        let (pixels, shown) = fit([1000, 500], Some(100), None, 2.0, 8192);
        assert_eq!(pixels, [200, 100]);
        assert_eq!(shown, Some(Vec2::new(100.0, 50.0)));
        // A small image asked to be large keeps its pixels and is stretched.
        let (pixels, shown) = fit([50, 50], None, Some(200), 1.0, 8192);
        assert_eq!(pixels, [50, 50]);
        assert_eq!(shown, Some(Vec2::new(200.0, 200.0)));
        // No size asked for: as it is, unless the GPU cannot take it.
        assert_eq!(fit([640, 480], None, None, 2.0, 8192), ([640, 480], None));
        assert_eq!(fit([16384, 1024], None, None, 1.0, 8192).0, [8192, 512]);
    }

    #[test]
    fn decodes_a_gif_and_reports_garbage() {
        // The smallest valid GIF: one transparent pixel.
        let gif: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff!\xf9\x04\x01\x00\x00\x00\x00,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02D\x01\x00;";
        let image = decode(gif, None, None, 1.0, 8192).expect("valid gif");
        assert_eq!(image.size, [1, 1]);
        assert!(decode(b"<html>Not found</html>", None, None, 1.0, 8192).is_err());
    }
}
