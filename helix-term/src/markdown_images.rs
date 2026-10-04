use std::{collections::HashMap, hash::{Hash, Hasher}, path::PathBuf, sync::Arc};

use helix_core::{line_ending::line_end_char_index, visual_offset_from_anchor};
use helix_view::{graphics::Rect, DocumentId, Editor, ViewId};
use tui::image::{CellPixelSize, ImagePlacement, ImagePlacementKey, ImageSource, ImageSourceId, PixelRect};

/// Prepared local Markdown image layouts; source text is never modified.
pub(crate) struct MarkdownImageScene {
    next_source: u64,
    layouts: HashMap<ViewId, Vec<Layout>>,
    layout_keys: HashMap<ViewId, LayoutKey>,
    rasters: HashMap<RasterKey, Arc<ImageSource>>,
    revealed_source_lines: HashMap<ViewId, Option<usize>>,
    changed: Vec<ViewId>,
}

/// Cache only inputs that can change decoded raster dimensions or reservations.
#[derive(Clone, Copy, PartialEq, Eq)]
struct LayoutKey { document: DocumentId, version: i32, width: u16, cells: CellPixelSize }

/// Prepared rasters survive document edits when their file and fitting inputs are unchanged.
#[derive(Clone, PartialEq, Eq, Hash)]
struct RasterKey { path: PathBuf, width: u16, cells: CellPixelSize }

struct Layout {
    anchor: usize,
    rows: usize,
    prior_rows: usize,
    cols: u16,
    source: Arc<ImageSource>,
    key: u64,
}

impl Default for MarkdownImageScene {
    fn default() -> Self {
        Self { next_source: 0, layouts: HashMap::new(), layout_keys: HashMap::new(), rasters: HashMap::new(), revealed_source_lines: HashMap::new(), changed: Vec::new() }
    }
}

impl MarkdownImageScene {
    /// Rebuilds layouts only when document, width, or terminal pixel metrics change.
    pub(crate) fn prepare(&mut self, editor: &mut Editor, _area: Rect, cells: Option<CellPixelSize>) -> &[ViewId] {
        self.changed.clear();
        let mut prepared = HashMap::new();
        if let Some(cells) = cells {
            for (view, _) in editor.tree.views() {
                let Some(doc) = editor.documents.get(&view.doc) else { continue };
                let key = LayoutKey { document: doc.id(), version: doc.version(), width: view.inner_width(doc), cells };
                if self.layout_keys.get(&view.id) != Some(&key) {
                    prepared.insert(view.id, (key, self.build_layout(doc, view.id, key.width, cells)));
                }
            }
        }
        for (view, _) in editor.tree.views_mut() {
            let Some((key, layouts)) = prepared.remove(&view.id) else { continue };
            let rows = layouts.iter().map(|entry| (entry.anchor, entry.rows)).collect();
            view.virtual_lines = helix_view::annotations::virtual_lines::VirtualLineMap::new(key.document, key.version, rows);
            self.layout_keys.insert(view.id, key);
            self.layouts.insert(view.id, layouts);
            self.changed.push(view.id);
        }
        &self.changed
    }


    /// Returns once when the cursor enters an image source line in a view.
    pub(crate) fn reveal_source_line(&mut self, view: ViewId, line: Option<usize>) -> bool {
        self.revealed_source_lines.insert(view, line) != Some(line) && line.is_some()
    }
    /// Decodes and pads each source once for the current layout cache key.
    fn build_layout(&mut self, doc: &helix_view::Document, view: ViewId, inner_width: u16, cells: CellPixelSize) -> Vec<Layout> {
        if doc.language_name() != Some("markdown") || inner_width == 0 { return Vec::new() }
        let text = doc.text().to_string();
        let base = doc.path().and_then(|path| path.parent()).map(PathBuf::from).unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let mut layouts = Vec::new();
        let mut stacked_rows = 0;
        let mut previous_anchor = None;
        for (start, destination) in markdown_images(&text) {
            let path = base.join(destination);
            let raster_key = RasterKey { path: path.clone(), width: inner_width, cells };
            let source = if let Some(source) = self.rasters.get(&raster_key) {
                Arc::clone(source)
            } else {
                let Some(image) = image::ImageReader::open(&path).ok().and_then(|reader| reader.with_guessed_format().ok()).and_then(|reader| reader.decode().ok()) else { continue };
                let image = image.to_rgba8();
                let max_width = u32::from(inner_width) * u32::from(cells.width);
                let (width, height) = image.dimensions();
                let image = if width > max_width {
                    let height = ((u64::from(height) * u64::from(max_width) + u64::from(width) / 2) / u64::from(width)) as u32;
                    image::imageops::resize(&image, max_width, height.max(1), image::imageops::FilterType::Triangle)
                } else { image };
                let (width, height) = image.dimensions();
                if width == 0 || height == 0 { continue }
                let cols = width.div_ceil(u32::from(cells.width));
                let rows = height.div_ceil(u32::from(cells.height));
                let canvas_width = cols * u32::from(cells.width);
                let canvas_height = rows * u32::from(cells.height);
                let Some(bytes) = usize::try_from(canvas_width).ok().and_then(|w| usize::try_from(canvas_height).ok()?.checked_mul(w)?.checked_mul(4)) else { continue };
                let mut canvas = vec![0; bytes];
                for y in 0..height {
                    let src = usize::try_from(y * width * 4).expect("image dimensions fit usize");
                    let dst = usize::try_from(y * canvas_width * 4).expect("image dimensions fit usize");
                    let len = usize::try_from(width * 4).expect("image dimensions fit usize");
                    canvas[dst..dst + len].copy_from_slice(&image.as_raw()[src..src + len]);
                }
                self.next_source = self.next_source.checked_add(1).expect("image source ID overflow");
                let source = Arc::new(ImageSource::new(ImageSourceId(self.next_source), canvas_width, canvas_height, canvas.into_boxed_slice()).expect("prepared RGBA canvas"));
                self.rasters.insert(raster_key, Arc::clone(&source));
                source
            };
            let cols = source.width / u32::from(cells.width);
            let rows = source.height / u32::from(cells.height);
            if cols == 0 || rows == 0 { continue }
            let char_start = text[..start].chars().count();
            let line = doc.text().char_to_line(char_start);
            let anchor = line_end_char_index(&doc.text().slice(..), line);
            if previous_anchor != Some(anchor) {
                stacked_rows = 0;
                previous_anchor = Some(anchor);
            }
            let prior_rows = stacked_rows;
            stacked_rows += rows as usize;
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            view.hash(&mut hasher); start.hash(&mut hasher);
            layouts.push(Layout { anchor, rows: rows as usize, prior_rows, cols: cols as u16, source, key: hasher.finish() });
        }
        layouts
    }

    /// Projects cached layouts; virtual rows from earlier source lines shift later previews.
    pub(crate) fn project(&self, editor: &Editor, _occlusions: &[Rect], out: &mut Vec<ImagePlacement>) {
        out.clear();
        for (view, _) in editor.tree.views() {
            let Some(doc) = editor.documents.get(&view.doc) else { continue };
            let Some(layouts) = self.layouts.get(&view.id) else { continue };
            let inner = view.inner_area(doc);
            let view_offset = doc.view_offset(view.id);
            let text = doc.text().slice(..);
            let text_format = doc.text_format(inner.width, None);
            let annotations = view.text_annotations(doc, None);
            for entry in layouts {
                let Ok((position, _)) = visual_offset_from_anchor(
                    text, view_offset.anchor, entry.anchor, &text_format, &annotations, usize::MAX,
                ) else { continue };
                // Formatter coordinates keep image placement in lockstep with rendered text.
                let y = i32::from(inner.y) + position.row as i32 + 1 + entry.prior_rows as i32 - view_offset.vertical_offset as i32;
                // Clip in cell space, then derive the crop from the original padded raster.
                let viewport_top = i32::from(inner.y);
                let viewport_bottom = i32::from(inner.y + inner.height);
                let image_bottom = y + entry.rows as i32;
                if image_bottom <= viewport_top || y >= viewport_bottom { continue }
                let clipped_top = viewport_top.saturating_sub(y) as usize;
                let Some(remaining_rows) = entry.rows.checked_sub(clipped_top) else { continue };
                let cell_y = y.max(viewport_top) as u16;
                let height = remaining_rows.min((viewport_bottom - i32::from(cell_y)) as usize) as u16;
                if height == 0 { continue }
                let cell_height = entry.source.height / entry.rows as u32;
                out.push(ImagePlacement { key: ImagePlacementKey(entry.key, 0), source: Arc::clone(&entry.source), crop: PixelRect { x: 0, y: clipped_top as u32 * cell_height, width: entry.source.width, height: u32::from(height) * cell_height }, cell_rect: Rect::new(inner.x, cell_y, entry.cols, height) });
            }
        }
    }
}

/// Extracts ordinary inline image destinations without treating wiki embeds as images.
fn markdown_images(text: &str) -> Vec<(usize, &str)> {
    let mut images = Vec::new();
    let mut offset = 0;
    let mut fenced = false;
    for line in text.split_inclusive('\n') {
        if line.trim_start().starts_with("```") { fenced = !fenced; }
        if !fenced && !line.contains("<!--") {
            let mut rest = line;
            while let Some(index) = rest.find("![") {
                let start = offset + line.len() - rest.len() + index;
                let Some(alt_end) = rest[index + 2..].find("](") else { break };
                let path_start = index + alt_end + 4;
                let Some(path_end) = rest[path_start..].find(')') else { break };
                let path = &rest[path_start..path_start + path_end];
                if !path.is_empty() && !path.contains("://") { images.push((start, path)); }
                rest = &rest[path_start + path_end + 1..];
            }
        }
        offset += line.len();
    }
    images
}
