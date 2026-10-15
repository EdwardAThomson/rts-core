//! A setting pack's UI skin (`theme/theme.json`): nine-slice frames, emblems, glyphs, mouse cursors and baked fonts
//! (`theme/fonts/fonts.json`). Both engines' HUDs and menus draw with it; which frames and glyphs a HUD asks for is
//! the engine's business, so nothing here names one. Moved from the Classic engine's renderer (`rts-engine`,
//! `crates/classic-render/src/skin.rs`); its `classic-tools` art tool and `art/fonts/bake.py` draw the generic pack's skin.
//!
//! Packs are read in order, the pack's own first: each frame, emblem, glyph and cursor comes from the first pack
//! that lists it, and the fonts from the first pack that has any, so a pack can bring only its emblems and keep the
//! generic frames. Whatever no pack has falls back to plain fills and the built-in pixel font, so a pack with no
//! theme still plays.
//!
//! `SkinFiles` is the decoded files, with no GPU; `Skin` is the same on the GPU, ready to draw.

use std::collections::BTreeMap;

use rts_core::json::{self, Value};

use crate::{Files, Font, Gpu, Rect, SpriteBatch, TexId};

/// Where a pack's skin is indexed.
pub const THEME_INDEX: &str = "theme/theme.json";

/// A text style. Each pack font bakes every style at UI scales 1 to 3.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Style {
    /// Labels on icons, health numbers.
    Small,
    /// Body text: names, the message feed, tooltips.
    Body,
    /// Headings and numbers: credits, power, the clock.
    Heading,
    /// Menu titles.
    Title,
}

impl Style {
    pub const ALL: [Style; 4] = [Style::Small, Style::Body, Style::Heading, Style::Title];

    pub fn id(self) -> &'static str {
        match self {
            Style::Small => "small",
            Style::Body => "body",
            Style::Heading => "heading",
            Style::Title => "title",
        }
    }

    /// The built-in pixel font's scale for this style at UI scale 1.
    fn builtin(self) -> f32 {
        match self {
            Style::Small => 1.0,
            Style::Body | Style::Heading => 2.0,
            Style::Title => 4.0,
        }
    }
}

/// A button's state, in the order of the button strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonState {
    Normal = 0,
    Hover = 1,
    Pressed = 2,
    Disabled = 3,
}

/// A decoded picture: width, height, RGBA pixels.
#[derive(Clone, Debug)]
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct FrameArt {
    pub image: Image,
    /// One frame's size; a strip holds one frame per state.
    pub frame: (u32, u32),
    /// Corner size in pixels.
    pub slice: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glyph {
    pub src: Rect,
    /// Where the glyph's box sits: from the pen, and from the line's top, in atlas pixels.
    pub left: f32,
    pub top: f32,
    /// How far the pen moves after it, in atlas pixels.
    pub advance: f32,
}

#[derive(Clone, Debug)]
pub struct FontAtlas {
    pub image: Image,
    /// The UI scale it was baked for.
    pub scale: u32,
    pub line: f32,
    pub ascent: f32,
    pub glyphs: BTreeMap<char, Glyph>,
}

impl FontAtlas {
    fn glyph(&self, c: char) -> Option<&Glyph> {
        self.glyphs.get(&c).or_else(|| self.glyphs.get(&'?'))
    }

    /// Width of `text` in this atlas's pixels.
    fn width(&self, text: &str) -> f32 {
        text.chars().filter_map(|c| self.glyph(c)).map(|g| g.advance).sum()
    }
}

#[derive(Clone, Debug)]
pub struct CursorArt {
    pub image: Image,
    /// Twice the size, for screens at twice the density.
    pub image_2x: Option<Image>,
    /// In pixels of `image`.
    pub hotspot: (u32, u32),
}

/// A pack's skin as decoded files.
#[derive(Clone, Debug, Default)]
pub struct SkinFiles {
    pub frames: BTreeMap<String, FrameArt>,
    pub fonts: BTreeMap<(Style, u32), FontAtlas>,
    pub emblems: BTreeMap<String, Image>,
    pub cursors: BTreeMap<String, CursorArt>,
    /// The HUD's glyphs by id: white art the HUD tints, and the same twice the size when the pack has it.
    pub glyphs: BTreeMap<String, (Image, Option<Image>)>,
    /// How the menus look: `"glass"` for the glass panels and buttons, else the plain ones. The pack's own choice
    /// wins over the generic pack's (`"menus"` in `theme.json`).
    pub menus: Option<String>,
}

fn image(files: &Files, path: &str) -> Result<Image, String> {
    let (w, h, rgba) = decode_png(&files.read(path)?, &files.name(path))?;
    Ok(Image { w, h, rgba })
}

/// A PNG's width, height and RGBA pixels, whatever colour type it was saved in. `name` is for messages.
pub fn decode_png(bytes: &[u8], name: &str) -> Result<(u32, u32, Vec<u8>), String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|e| format!("{name}: {e}"))?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("PNG too large")?];
    let info = reader.next_frame(&mut buf).map_err(|e| format!("{name}: {e}"))?;
    let px = &buf[..info.buffer_size()];
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => px.to_vec(),
        png::ColorType::Rgb => px.chunks_exact(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => px.chunks_exact(2).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
        png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err(format!("{name}: palette not expanded")),
    };
    Ok((info.width, info.height, rgba))
}

fn pair(v: Option<&Value>) -> Option<(i64, i64)> {
    let a = v?.as_array()?;
    Some((a.first()?.as_int()?, a.get(1)?.as_int()?))
}

impl SkinFiles {
    /// Read the skins of `packs`, the pack's own first. A pack with no `theme/theme.json` adds nothing; a broken
    /// one is an error.
    pub fn load(packs: &[&Files]) -> Result<SkinFiles, String> {
        let mut theme = SkinFiles::default();
        for files in packs {
            let Ok(text) = files.read_text(THEME_INDEX) else { continue };
            let doc = json::parse(&text).map_err(|e| format!("{}: {e:?}", files.name(THEME_INDEX)))?;
            let entries = |key: &str| doc.get(key).and_then(Value::as_object).unwrap_or(&[]);
            let file = |e: &Value, key: &str| e.get(key).and_then(Value::as_str).map(str::to_string);
            for (id, e) in entries("frames") {
                if theme.frames.contains_key(id) {
                    continue;
                }
                let path = file(e, "file").ok_or_else(|| format!("{THEME_INDEX}: frame {id} has no file"))?;
                let picture = image(files, &path)?;
                let (fw, fh) = pair(e.get("frame")).unwrap_or((picture.w as i64, picture.h as i64));
                let slice = e.get("slice").and_then(Value::as_int).unwrap_or(0);
                if fw <= 0 || fh <= 0 || slice < 0 || 2 * slice > fw.min(fh) {
                    return Err(format!("{THEME_INDEX}: frame {id}: slice {slice} doesn't fit {fw}x{fh}"));
                }
                let frame = (fw as u32, fh as u32);
                theme.frames.insert(id.clone(), FrameArt { image: picture, frame, slice: slice as u32 });
            }
            for (id, e) in entries("emblems") {
                if !theme.emblems.contains_key(id) {
                    let path = file(e, "file_2x").or_else(|| file(e, "file")).ok_or("emblem without a file")?;
                    theme.emblems.insert(id.clone(), image(files, &path)?);
                }
            }
            for (id, e) in entries("glyphs") {
                if !theme.glyphs.contains_key(id) {
                    let path = file(e, "file").ok_or_else(|| format!("{THEME_INDEX}: glyph {id} has no file"))?;
                    let big = file(e, "file_2x").map(|p| image(files, &p)).transpose()?;
                    theme.glyphs.insert(id.clone(), (image(files, &path)?, big));
                }
            }
            for (id, e) in entries("cursors") {
                if theme.cursors.contains_key(id) {
                    continue;
                }
                let path = file(e, "file").ok_or_else(|| format!("{THEME_INDEX}: cursor {id} has no file"))?;
                let picture = image(files, &path)?;
                let image_2x = file(e, "file_2x").map(|p| image(files, &p)).transpose()?;
                let (hx, hy) = pair(e.get("hotspot")).unwrap_or((0, 0));
                if hx < 0 || hy < 0 || hx >= picture.w as i64 || hy >= picture.h as i64 {
                    return Err(format!("{THEME_INDEX}: cursor {id}: hotspot ({hx}, {hy}) is off its picture"));
                }
                theme
                    .cursors
                    .insert(id.clone(), CursorArt { image: picture, image_2x, hotspot: (hx as u32, hy as u32) });
            }
            if theme.menus.is_none() {
                theme.menus = doc.get("menus").and_then(Value::as_str).map(str::to_string);
            }
            if theme.fonts.is_empty()
                && let Some(index) = doc.get("fonts").and_then(Value::as_str)
            {
                theme.fonts = load_fonts(files, index)?;
            }
        }
        Ok(theme)
    }
}

/// The atlases a fonts index lists, by style and UI scale.
fn load_fonts(files: &Files, index: &str) -> Result<BTreeMap<(Style, u32), FontAtlas>, String> {
    let doc = json::parse(&files.read_text(index)?).map_err(|e| format!("{}: {e:?}", files.name(index)))?;
    let mut out = BTreeMap::new();
    for a in doc.get("atlases").and_then(Value::as_array).unwrap_or(&[]) {
        let name = a.get("style").and_then(Value::as_str).unwrap_or("");
        let Some(style) = Style::ALL.into_iter().find(|s| s.id() == name) else { continue };
        let scale = a.get("scale").and_then(Value::as_int).unwrap_or(1).max(1) as u32;
        let path = a.get("file").and_then(Value::as_str).ok_or_else(|| format!("{index}: atlas without a file"))?;
        let mut glyphs = BTreeMap::new();
        for g in a.get("glyphs").and_then(Value::as_array).unwrap_or(&[]) {
            let n: Vec<i64> = g.as_array().unwrap_or(&[]).iter().filter_map(Value::as_int).collect();
            let [code, x, y, w, h, left, top, advance] = n[..] else {
                return Err(format!("{index}: {name} glyph {g:?} is not 8 numbers"));
            };
            let Some(c) = char::from_u32(code as u32) else { continue };
            let src = Rect::new(x as f32, y as f32, w as f32, h as f32);
            glyphs.insert(c, Glyph { src, left: left as f32, top: top as f32, advance: advance as f32 / 64.0 });
        }
        let line = a.get("line").and_then(Value::as_int).unwrap_or(0) as f32;
        let ascent = a.get("ascent").and_then(Value::as_int).unwrap_or(0) as f32;
        out.insert((style, scale), FontAtlas { image: image(files, path)?, scale, line, ascent, glyphs });
    }
    Ok(out)
}

/// The nine pieces of a frame drawn into `dst`: (source in the frame, destination). Corners keep their size times
/// `scale`, shrunk to fit a small `dst`; edges stretch along their length; the middle stretches both ways.
pub fn nine_slice(frame: (f32, f32), slice: f32, dst: Rect, scale: f32) -> Vec<(Rect, Rect)> {
    let (fw, fh) = frame;
    let c = (slice * scale).round().min((dst.w / 2.0).floor()).min((dst.h / 2.0).floor()).max(0.0);
    let xs = [0.0, slice, fw - slice, fw];
    let ys = [0.0, slice, fh - slice, fh];
    let (x0, y0) = (dst.x.round(), dst.y.round());
    let (x1, y1) = ((dst.x + dst.w).round(), (dst.y + dst.h).round());
    let dx = [x0, x0 + c, x1 - c, x1];
    let dy = [y0, y0 + c, y1 - c, y1];
    let mut out = Vec::with_capacity(9);
    for j in 0..3 {
        for i in 0..3 {
            let src = Rect::new(xs[i], ys[j], xs[i + 1] - xs[i], ys[j + 1] - ys[j]);
            let d = Rect::new(dx[i], dy[j], dx[i + 1] - dx[i], dy[j + 1] - dy[j]);
            if src.w > 0.0 && src.h > 0.0 && d.w > 0.0 && d.h > 0.0 {
                out.push((src, d));
            }
        }
    }
    out
}

/// The atlas scale to draw `ui` with, and how much to stretch it: the nearest baked scale at or below.
fn atlas_scale(ui: f32, baked: impl Iterator<Item = u32>) -> Option<(u32, f32)> {
    let want = ui.round().max(1.0) as u32;
    let mut scales: Vec<u32> = baked.collect();
    scales.sort_unstable();
    let pick = scales.iter().rev().find(|&&s| s <= want).or(scales.first()).copied()?;
    Some((pick, ui / pick as f32))
}

struct GpuFrame {
    tex: TexId,
    frame: (f32, f32),
    slice: f32,
}

struct GpuFont {
    tex: TexId,
    atlas: FontAtlas,
}

/// The skin on the GPU.
pub struct Skin {
    frames: BTreeMap<String, GpuFrame>,
    fonts: BTreeMap<(Style, u32), GpuFont>,
    emblems: BTreeMap<String, (TexId, f32, f32)>,
    /// Each glyph at 1x and, when the pack has it, 2x: texture and size.
    glyphs: BTreeMap<String, Vec<(TexId, f32, f32)>>,
    builtin: Font,
    /// The cursors stay on the CPU: the window system draws them.
    pub cursors: BTreeMap<String, CursorArt>,
    /// Whether the menus draw as glass: the pack asked for it (or the player did, `--menus glass`) and the glass
    /// frames are there. Otherwise they use the plain panel and buttons.
    pub glass_menus: bool,
}

impl Skin {
    pub fn new(gpu: &Gpu, batch: &mut SpriteBatch, theme: SkinFiles) -> Skin {
        let mut up = |i: &Image| batch.texture(gpu, i.w, i.h, &i.rgba);
        let frames = theme
            .frames
            .iter()
            .map(|(id, f)| {
                let g =
                    GpuFrame { tex: up(&f.image), frame: (f.frame.0 as f32, f.frame.1 as f32), slice: f.slice as f32 };
                (id.clone(), g)
            })
            .collect();
        let emblems = theme.emblems.iter().map(|(id, i)| (id.clone(), (up(i), i.w as f32, i.h as f32))).collect();
        let glyphs = theme
            .glyphs
            .iter()
            .map(|(id, (one, two))| {
                let sizes = std::iter::once(one).chain(two).map(|i| (up(i), i.w as f32, i.h as f32)).collect();
                (id.clone(), sizes)
            })
            .collect();
        let fonts = theme.fonts.into_iter().map(|(k, atlas)| (k, GpuFont { tex: up(&atlas.image), atlas })).collect();
        let mut skin = Skin {
            frames,
            fonts,
            emblems,
            glyphs,
            builtin: Font::new(gpu, batch),
            cursors: theme.cursors,
            glass_menus: false,
        };
        skin.set_menus(theme.menus.as_deref().unwrap_or("plain"));
        skin
    }

    /// Choose the menus' look, `"glass"` or anything else for plain. Glass needs its frames; without them the menus
    /// stay plain.
    pub fn set_menus(&mut self, style: &str) {
        self.glass_menus = style == "glass"
            && ["menu_panel", "menu_panel_light", "menu_button", "menu_button_light"]
                .iter()
                .all(|id| self.frames.contains_key(*id));
    }

    /// Load and upload the skins of `packs`, the pack's own first.
    pub fn load(gpu: &Gpu, batch: &mut SpriteBatch, packs: &[&Files]) -> Result<Skin, String> {
        Ok(Skin::new(gpu, batch, SkinFiles::load(packs)?))
    }

    /// Whether the pack has frame `id`.
    pub fn has(&self, id: &str) -> bool {
        self.frames.contains_key(id)
    }

    /// Frame `id` (state `state` of a strip) over `r`, its corners at UI scale `ui`. Without that frame, a plain
    /// `fallback` fill with a lighter edge.
    pub fn frame(&self, batch: &mut SpriteBatch, id: &str, state: usize, r: Rect, ui: f32, fallback: [u8; 4]) {
        if !self.tinted(batch, id, state, r, ui, [255; 4]) {
            batch.fill(r, fallback);
            batch.outline(r, 1.0, [90, 96, 104, 255]);
        }
    }

    /// Frame `id` (state `state`) over `r`, multiplied by `tint`; the menus' glass light is white art coloured this
    /// way by an accent. Returns false, drawing nothing, when the pack has no such frame.
    pub fn tinted(&self, batch: &mut SpriteBatch, id: &str, state: usize, r: Rect, ui: f32, tint: [u8; 4]) -> bool {
        let Some(f) = self.frames.get(id) else { return false };
        let x0 = state as f32 * f.frame.0;
        for (src, dst) in nine_slice(f.frame, f.slice, r, ui) {
            batch.sprite(f.tex, Rect::new(src.x + x0, src.y, src.w, src.h), dst, tint);
        }
        true
    }

    /// Frame `id`'s first state stretched whole over `r`, but only the part inside `clip`, multiplied by `tint`: a
    /// sweep of light crossing a button stays on the button.
    pub fn clipped(&self, batch: &mut SpriteBatch, id: &str, r: Rect, clip: Rect, tint: [u8; 4]) {
        let Some(f) = self.frames.get(id) else { return };
        let (x0, y0) = (r.x.max(clip.x), r.y.max(clip.y));
        let (x1, y1) = ((r.x + r.w).min(clip.x + clip.w), (r.y + r.h).min(clip.y + clip.h));
        if x1 <= x0 || y1 <= y0 || r.w <= 0.0 || r.h <= 0.0 {
            return;
        }
        let (sx, sy) = (f.frame.0 / r.w, f.frame.1 / r.h);
        let src = Rect::new((x0 - r.x) * sx, (y0 - r.y) * sy, (x1 - x0) * sx, (y1 - y0) * sy);
        batch.sprite(f.tex, src, Rect::new(x0, y0, x1 - x0, y1 - y0), tint);
    }

    pub fn button(&self, batch: &mut SpriteBatch, r: Rect, state: ButtonState, ui: f32) {
        let fallback = match state {
            ButtonState::Normal => [58, 68, 80, 255],
            ButtonState::Hover => [72, 84, 102, 255],
            ButtonState::Pressed => [42, 50, 60, 255],
            ButtonState::Disabled => [42, 46, 52, 255],
        };
        self.frame(batch, "button", state as usize, r, ui, fallback);
    }

    /// Faction `id`'s emblem fitted into `r`, if the pack has one.
    pub fn emblem(&self, batch: &mut SpriteBatch, id: &str, r: Rect) -> bool {
        let Some(&(tex, w, h)) = self.emblems.get(id) else { return false };
        let fit = (r.w / w).min(r.h / h);
        let (dw, dh) = (w * fit, h * fit);
        batch.sprite(
            tex,
            Rect::new(0.0, 0.0, w, h),
            Rect::new(r.x + (r.w - dw) / 2.0, r.y + (r.h - dh) / 2.0, dw, dh),
            [255; 4],
        );
        true
    }

    /// Whether the pack has glyph `id`.
    pub fn has_glyph(&self, id: &str) -> bool {
        self.glyphs.contains_key(id)
    }

    /// Glyph `id` fitted into `r` and multiplied by `tint`, drawn from the largest picture no bigger than `r` (or
    /// the smallest) so it stays crisp. False, drawing nothing, when the pack has no such glyph.
    pub fn glyph(&self, batch: &mut SpriteBatch, id: &str, r: Rect, tint: [u8; 4]) -> bool {
        let Some(sizes) = self.glyphs.get(id) else { return false };
        let Some(&(tex, w, h)) = sizes.iter().rev().find(|(_, w, h)| *w <= r.w && *h <= r.h).or(sizes.first()) else {
            return false;
        };
        let fit = (r.w / w).min(r.h / h);
        // Whole multiples when the picture fits that way, so its pixels stay square.
        let fit = if fit >= 1.0 { fit.floor() } else { fit };
        let (dw, dh) = (w * fit, h * fit);
        let at = Rect::new((r.x + (r.w - dw) / 2.0).round(), (r.y + (r.h - dh) / 2.0).round(), dw, dh);
        batch.sprite(tex, Rect::new(0.0, 0.0, w, h), at, tint);
        true
    }

    fn font(&self, style: Style, ui: f32) -> Option<(&GpuFont, f32)> {
        let baked = self.fonts.keys().filter(|(s, _)| *s == style).map(|&(_, k)| k);
        let (k, stretch) = atlas_scale(ui, baked)?;
        Some((&self.fonts[&(style, k)], stretch))
    }

    /// Width of `text` in `style` at UI scale `ui`, in screen pixels.
    pub fn width(&self, style: Style, text: &str, ui: f32) -> f32 {
        match self.font(style, ui) {
            Some((f, stretch)) => (f.atlas.width(text) * stretch).ceil(),
            None => Font::width(&text.to_uppercase(), (style.builtin() * ui).round().max(1.0)),
        }
    }

    /// Height of a line of `style` at UI scale `ui`.
    pub fn line(&self, style: Style, ui: f32) -> f32 {
        match self.font(style, ui) {
            Some((f, stretch)) => (f.atlas.line * stretch).round(),
            None => Font::height((style.builtin() * ui).round().max(1.0)),
        }
    }

    /// Draw `text` in `style` with the top of its line at (x, y); returns its width.
    #[allow(clippy::too_many_arguments)]
    pub fn text(
        &self,
        batch: &mut SpriteBatch,
        style: Style,
        text: &str,
        x: f32,
        y: f32,
        ui: f32,
        colour: [u8; 4],
    ) -> f32 {
        let Some((f, stretch)) = self.font(style, ui) else {
            return self.builtin.draw(batch, text, x, y, (style.builtin() * ui).round().max(1.0), colour);
        };
        let mut pen = x;
        for c in text.chars() {
            let Some(g) = f.atlas.glyph(c) else { continue };
            if g.src.w > 0.0 {
                // Whole pixels, so glyphs stay crisp under the nearest-pixel sampler.
                let dst = Rect::new(
                    (pen + g.left * stretch).round(),
                    (y + g.top * stretch).round(),
                    g.src.w * stretch,
                    g.src.h * stretch,
                );
                batch.sprite(f.tex, g.src, dst, colour);
            }
            pen += g.advance * stretch;
        }
        (pen - x).ceil()
    }

    /// Cursor `id` and its hotspot: the larger picture on a screen at `ui` of 2 or more when the pack has one.
    pub fn cursor(&self, id: &str, ui: f32) -> Option<(&Image, (u32, u32))> {
        let c = self.cursors.get(id)?;
        match (&c.image_2x, ui >= 2.0) {
            (Some(i), true) => Some((i, (c.hotspot.0 * 2, c.hotspot.1 * 2))),
            _ => Some((&c.image, c.hotspot)),
        }
    }
}

/// The files a theme index names, for fetching in the browser: its frames, emblems, cursors, glyphs and fonts
/// index.
pub fn theme_files(index: &str) -> Vec<String> {
    let Ok(doc) = json::parse(index) else { return Vec::new() };
    let mut out = Vec::new();
    for section in ["frames", "emblems", "cursors", "glyphs"] {
        for (_, e) in doc.get(section).and_then(Value::as_object).unwrap_or(&[]) {
            for key in ["file", "file_2x"] {
                out.extend(e.get(key).and_then(Value::as_str).map(str::to_string));
            }
        }
    }
    out.extend(doc.get("fonts").and_then(Value::as_str).map(str::to_string));
    out
}

/// The atlases a fonts index names, for fetching in the browser.
pub fn font_files(index: &str) -> Vec<String> {
    let Ok(doc) = json::parse(index) else { return Vec::new() };
    doc.get("atlases")
        .and_then(Value::as_array)
        .unwrap_or(&[])
        .iter()
        .filter_map(|a| a.get("file").and_then(Value::as_str).map(str::to_string))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nine_slice_keeps_corners_and_covers_the_rect() {
        let parts = nine_slice((24.0, 24.0), 8.0, Rect::new(10.0, 20.0, 100.0, 50.0), 2.0);
        assert_eq!(parts.len(), 9);
        let (src, dst) = parts[0];
        assert_eq!((src.w, dst.w, dst.h), (8.0, 16.0, 16.0), "a corner keeps its size times the scale");
        let area: f32 = parts.iter().map(|(_, d)| d.w * d.h).sum();
        assert_eq!(area, 100.0 * 50.0, "the pieces tile the rectangle exactly");
        // Too small for the corners: they shrink to half the rectangle and the middle disappears.
        let tiny = nine_slice((24.0, 24.0), 8.0, Rect::new(0.0, 0.0, 10.0, 30.0), 1.0);
        assert!(tiny.iter().all(|(_, d)| d.w <= 5.0 || d.w == 0.0));
    }

    #[test]
    fn fonts_use_the_nearest_baked_scale_at_or_below() {
        let baked = || [1, 2, 3].into_iter();
        assert_eq!(atlas_scale(1.0, baked()), Some((1, 1.0)));
        assert_eq!(atlas_scale(2.0, baked()), Some((2, 1.0)));
        assert_eq!(atlas_scale(4.0, baked()), Some((3, 4.0 / 3.0)));
        assert_eq!(atlas_scale(0.75, baked()), Some((1, 0.75)));
        assert_eq!(atlas_scale(1.0, std::iter::empty()), None);
    }

    /// A PNG of `w` by `h` pixels, all `colour`.
    fn png(w: u32, h: u32, colour: [u8; 4]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let data: Vec<u8> = (0..w * h).flat_map(|_| colour).collect();
            enc.write_header().unwrap().write_image_data(&data).unwrap();
        }
        out
    }

    fn pack(label: &str, files: &[(&str, Vec<u8>)]) -> Files {
        let files = files.iter().map(|(p, b)| (p.to_string(), b.clone())).collect();
        Files::Memory { label: label.into(), files }
    }

    #[test]
    fn packs_are_read_own_first_and_fall_back_to_the_next() {
        let own = pack(
            "own",
            &[
                (
                    "theme/theme.json",
                    br#"{"frames": {"panel": {"file": "p.png", "frame": [8, 8], "slice": 3}},
                    "glyphs": {"menu": {"file": "g.png"}}}"#
                        .to_vec(),
                ),
                ("p.png", png(8, 8, [255, 0, 0, 255])),
                ("g.png", png(4, 4, [255; 4])),
            ],
        );
        let generic = pack(
            "generic",
            &[
                ("theme/theme.json", br#"{"menus": "glass",
                    "frames": {"panel": {"file": "q.png", "slice": 2}, "button": {"file": "b.png", "frame": [6, 4], "slice": 1}},
                    "cursors": {"move": {"file": "c.png", "hotspot": [1, 2]}},
                    "fonts": "fonts.json"}"#
                    .to_vec()),
                ("q.png", png(10, 10, [0, 255, 0, 255])),
                ("b.png", png(24, 4, [0, 0, 255, 255])),
                ("c.png", png(4, 4, [255; 4])),
                ("fonts.json", br#"{"atlases": [{"style": "body", "scale": 1, "file": "f.png", "line": 9, "ascent": 7,
                    "glyphs": [[65, 0, 0, 3, 5, 0, 2, 256]]}]}"#
                    .to_vec()),
                ("f.png", png(4, 8, [255; 4])),
            ],
        );
        let theme = SkinFiles::load(&[&own, &generic]).unwrap();
        assert_eq!(theme.frames["panel"].image.rgba[..4], [255, 0, 0, 255], "the pack's own panel wins");
        assert_eq!((theme.frames["panel"].frame, theme.frames["panel"].slice), ((8, 8), 3));
        assert_eq!(theme.frames["button"].frame, (6, 4), "a frame only the generic pack has is kept");
        assert!(theme.glyphs.contains_key("menu"));
        assert_eq!(theme.cursors["move"].hotspot, (1, 2));
        assert_eq!(theme.menus.as_deref(), Some("glass"));
        let atlas = &theme.fonts[&(Style::Body, 1)];
        assert_eq!((atlas.line, atlas.ascent, atlas.glyphs[&'A'].advance), (9.0, 7.0, 4.0));
        assert_eq!(atlas.width("AA"), 8.0);
    }

    #[test]
    fn a_pack_without_a_theme_adds_nothing_and_a_broken_one_is_named() {
        assert!(SkinFiles::load(&[&pack("none", &[])]).unwrap().frames.is_empty());
        let broken = pack(
            "broken",
            &[
                ("theme/theme.json", br#"{"frames": {"panel": {"file": "p.png", "slice": 5}}}"#.to_vec()),
                ("p.png", png(8, 8, [255; 4])),
            ],
        );
        let e = SkinFiles::load(&[&broken]).unwrap_err();
        assert!(e.contains("panel") && e.contains("slice 5"), "{e}");
        let missing = pack("missing", &[("theme/theme.json", br#"{"glyphs": {"menu": {"file": "g.png"}}}"#.to_vec())]);
        assert!(SkinFiles::load(&[&missing]).unwrap_err().contains("missing/g.png"));
    }

    #[test]
    fn theme_files_lists_what_the_browser_must_fetch() {
        let index = r#"{"frames": {"panel": {"file": "theme/panel.png"}},
            "cursors": {"move": {"file": "theme/move.png", "file_2x": "theme/move@2x.png"}},
            "fonts": "theme/fonts/fonts.json"}"#;
        assert_eq!(
            theme_files(index),
            ["theme/panel.png", "theme/move.png", "theme/move@2x.png", "theme/fonts/fonts.json"]
        );
        assert_eq!(font_files(r#"{"atlases": [{"file": "a.png"}, {"file": "b.png"}]}"#), ["a.png", "b.png"]);
    }
}
