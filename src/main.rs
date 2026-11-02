use eframe::egui;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// macOS-style Quick Look preview application for Linux.
fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("Quick Look (Rust) - macOS Preview & Quick Look for Linux");
        println!("\nUsage:");
        println!("  quicklook [FILE_OR_DIRECTORY]");
        println!("\nControls:");
        println!("  Space / Esc / q  : Close window immediately");
        println!("  ◀ / Left / Up / k: Previous sibling file");
        println!("  ▶ / Right / Down / j: Next sibling file");
        println!("  + / - / 0        : Zoom in, zoom out, reset zoom");
        println!("  Enter / o        : Open file in default system application");
        println!("  Scroll Wheel     : Smooth zoom / scroll");
        println!("  Mouse Drag       : Pan image");
        return Ok(());
    }

    if args.iter().any(|a| a == "--version" || a == "-v") {
        println!("quicklook v{}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let initial_path = if args.len() > 1 && !args[1].starts_with('-') {
        PathBuf::from(&args[1])
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([960.0, 680.0])
            .with_min_inner_size([480.0, 360.0])
            .with_title("Quick Look")
            .with_decorations(true)
            .with_transparent(false),
        ..Default::default()
    };

    eframe::run_native(
        "Quick Look",
        options,
        Box::new(move |cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            configure_mac_style(&cc.egui_ctx);
            Ok(Box::new(QuickLookApp::new(initial_path)))
        }),
    )
}

fn configure_mac_style(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.window_fill = egui::Color32::from_rgb(28, 28, 30);
    visuals.panel_fill = egui::Color32::from_rgb(24, 24, 26);
    visuals.extreme_bg_color = egui::Color32::from_rgb(18, 18, 20);
    visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(36, 36, 40);
    visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(44, 44, 48);
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(60, 60, 66);
    visuals.widgets.active.bg_fill = egui::Color32::from_rgb(0, 122, 255); // macOS Accent Blue
    visuals.override_text_color = Some(egui::Color32::from_rgb(240, 240, 242));
    ctx.set_visuals(visuals);
}

#[derive(Debug)]
pub enum FileKind {
    Image {
        width: u32,
        height: u32,
        uri: String,
    },
    Code {
        content: String,
        language: String,
        lines: usize,
        truncated: bool,
    },
    Markdown {
        content: String,
        lines: usize,
    },
    Spreadsheet {
        rows: Vec<Vec<String>>,
        total_rows: usize,
        total_cols: usize,
    },
    Presentation {
        slides: Vec<SlideInfo>,
        current_slide: usize,
    },
    Document {
        paragraphs: Vec<DocParagraph>,
        word_count: usize,
    },
    Directory {
        entries: Vec<DirItem>,
        total_items: usize,
    },
    Archive {
        entries: Vec<String>,
        total_entries: usize,
    },
    Pdf {
        page_estimate: Option<usize>,
    },
    Media {
        kind: &'static str,
    },
    Hex {
        data: Vec<u8>,
        total_size: u64,
    },
    Error(String),
}

#[derive(Debug, Clone)]
pub struct SlideInfo {
    pub title: String,
    pub bullets: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DocParagraph {
    pub text: String,
    pub is_heading: bool,
}

#[derive(Debug, Clone)]
pub struct DirItem {
    pub name: String,
    pub is_dir: bool,
    pub size_str: String,
    pub modified_str: String,
}

pub struct QuickLookApp {
    current_path: PathBuf,
    siblings: Vec<PathBuf>,
    sibling_idx: usize,
    file_size: u64,
    file_kind: FileKind,
    zoom_scale: f32,
    pan_offset: egui::Vec2,
    search_filter: String,
}

impl QuickLookApp {
    pub fn new(target: PathBuf) -> Self {
        let abs_path = fs::canonicalize(&target).unwrap_or(target);
        let siblings = get_siblings(&abs_path);
        let sibling_idx = siblings.iter().position(|p| p == &abs_path).unwrap_or(0);

        let mut app = Self {
            current_path: abs_path,
            siblings,
            sibling_idx,
            file_size: 0,
            file_kind: FileKind::Error("Initializing...".into()),
            zoom_scale: 1.0,
            pan_offset: egui::Vec2::ZERO,
            search_filter: String::new(),
        };

        app.load_current_file();
        app
    }

    fn navigate(&mut self, delta: isize) {
        if self.siblings.is_empty() {
            return;
        }
        let len = self.siblings.len() as isize;
        let new_idx = ((self.sibling_idx as isize + delta).rem_euclid(len)) as usize;
        self.sibling_idx = new_idx;
        self.current_path = self.siblings[new_idx].clone();
        self.zoom_scale = 1.0;
        self.pan_offset = egui::Vec2::ZERO;
        self.search_filter.clear();
        self.load_current_file();
    }

    fn load_current_file(&mut self) {
        let path = &self.current_path;
        let metadata = match fs::metadata(path) {
            Ok(m) => m,
            Err(e) => {
                self.file_size = 0;
                self.file_kind = FileKind::Error(format!("Failed to read metadata: {e}"));
                return;
            }
        };

        self.file_size = metadata.len();

        if metadata.is_dir() {
            self.file_kind = load_directory(path);
            return;
        }

        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();

        // 1. Images
        match ext.as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "svg" | "tiff" | "tif" => {
                let (width, height) = image::image_dimensions(path).unwrap_or((0, 0));
                let uri = format!("file://{}", path.to_string_lossy());
                self.file_kind = FileKind::Image {
                    width,
                    height,
                    uri,
                };
                return;
            }
            _ => {}
        }

        // 2. Presentations (PowerPoint PPTX, ODP, PPT)
        if ext == "pptx" || ext == "odp" {
            self.file_kind = load_pptx(path);
            return;
        }
        if ext == "ppt" {
            self.file_kind = load_legacy_office(path, "PowerPoint Presentation");
            return;
        }

        // 3. Spreadsheets (Excel XLSX, ODS, XLS, CSV, TSV)
        if ext == "xlsx" || ext == "ods" {
            self.file_kind = load_xlsx(path);
            return;
        }
        if ext == "xls" {
            self.file_kind = load_legacy_office(path, "Excel Spreadsheet");
            return;
        }
        if ext == "csv" {
            if let Ok(content) = fs::read_to_string(path) {
                self.file_kind = load_csv_or_tsv(&content, ',');
                return;
            }
        }
        if ext == "tsv" {
            if let Ok(content) = fs::read_to_string(path) {
                self.file_kind = load_csv_or_tsv(&content, '\t');
                return;
            }
        }

        // 4. Word Documents (DOCX, ODT, DOC, RTF)
        if ext == "docx" || ext == "odt" {
            self.file_kind = load_docx(path);
            return;
        }
        if ext == "doc" || ext == "rtf" {
            self.file_kind = load_legacy_office(path, "Word Document");
            return;
        }

        // 5. Archives (ZIP detection)
        if ext == "zip" || ext == "jar" || ext == "war" {
            self.file_kind = load_zip_listing(path, self.file_size);
            return;
        }

        // 6. Audio & Video
        match ext.as_str() {
            "mp3" | "wav" | "ogg" | "flac" | "aac" | "m4a" | "opus" => {
                self.file_kind = FileKind::Media { kind: "Audio" };
                return;
            }
            "mp4" | "mkv" | "mov" | "webm" | "avi" | "wmv" | "flv" => {
                self.file_kind = FileKind::Media { kind: "Video" };
                return;
            }
            _ => {}
        }

        // 7. PDF
        if ext == "pdf" {
            let pages = estimate_pdf_pages(path);
            self.file_kind = FileKind::Pdf {
                page_estimate: pages,
            };
            return;
        }

        // 5. Code & Text (Streaming read up to 512 KB to avoid UI freezing)
        const MAX_TEXT_BYTES: u64 = 512 * 1024;
        let read_limit = self.file_size.min(MAX_TEXT_BYTES) as usize;
        let mut buffer = vec![0u8; read_limit];
        let bytes_read = match File::open(path).and_then(|mut f| f.read(&mut buffer)) {
            Ok(n) => n,
            Err(e) => {
                self.file_kind = FileKind::Error(format!("Read error: {e}"));
                return;
            }
        };
        buffer.truncate(bytes_read);

        // Check if content is valid UTF-8
        if let Ok(text) = std::str::from_utf8(&buffer) {
            let lines = text.lines().count();
            if ext == "md" || ext == "markdown" {
                self.file_kind = FileKind::Markdown {
                    content: text.to_string(),
                    lines,
                };
                return;
            }

            let lang = detect_language(&ext, text);
            let truncated = self.file_size > MAX_TEXT_BYTES;
            self.file_kind = FileKind::Code {
                content: text.to_string(),
                language: lang,
                lines,
                truncated,
            };
            return;
        }

        // 6. Binary / Hex Fallback (Reads first 16KB for hex dump)
        const MAX_HEX_BYTES: usize = 16 * 1024;
        let hex_slice = if buffer.len() > MAX_HEX_BYTES {
            buffer[..MAX_HEX_BYTES].to_vec()
        } else {
            buffer
        };

        self.file_kind = FileKind::Hex {
            data: hex_slice,
            total_size: self.file_size,
        };
    }
}

impl eframe::App for QuickLookApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Keyboard Shortcuts (macOS Quick Look Behavior)
        ctx.input(|i| {
            if i.key_pressed(egui::Key::Escape)
                || i.key_pressed(egui::Key::Space)
                || i.key_pressed(egui::Key::Q)
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            if i.key_pressed(egui::Key::ArrowLeft)
                || i.key_pressed(egui::Key::ArrowUp)
                || i.key_pressed(egui::Key::K)
                || i.key_pressed(egui::Key::OpenBracket)
            {
                self.navigate(-1);
            }
            if i.key_pressed(egui::Key::ArrowRight)
                || i.key_pressed(egui::Key::ArrowDown)
                || i.key_pressed(egui::Key::J)
                || i.key_pressed(egui::Key::CloseBracket)
            {
                self.navigate(1);
            }
            if i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals) {
                self.zoom_scale = (self.zoom_scale * 1.2).min(10.0);
            }
            if i.key_pressed(egui::Key::Minus) {
                self.zoom_scale = (self.zoom_scale / 1.2).max(0.1);
            }
            if i.key_pressed(egui::Key::Num0) {
                self.zoom_scale = 1.0;
                self.pan_offset = egui::Vec2::ZERO;
            }
            if i.key_pressed(egui::Key::PageDown) {
                if let FileKind::Presentation { current_slide, slides } = &mut self.file_kind {
                    if *current_slide + 1 < slides.len() {
                        *current_slide += 1;
                    }
                }
            }
            if i.key_pressed(egui::Key::PageUp) {
                if let FileKind::Presentation { current_slide, .. } = &mut self.file_kind {
                    if *current_slide > 0 {
                        *current_slide -= 1;
                    }
                }
            }
            if i.key_pressed(egui::Key::O) || i.key_pressed(egui::Key::Enter) {
                let _ = open::that(&self.current_path);
            }
        });

        // 1. macOS Top Titlebar / Control Bar
        egui::TopBottomPanel::top("top_bar")
            .exact_height(48.0)
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;

                    // Navigation buttons
                    if ui.button("◀").on_hover_text("Previous (Left Arrow / [)").clicked() {
                        self.navigate(-1);
                    }
                    if ui.button("▶").on_hover_text("Next (Right Arrow / ])").clicked() {
                        self.navigate(1);
                    }

                    if !self.siblings.is_empty() {
                        ui.label(
                            egui::RichText::new(format!(
                                "{}/{}",
                                self.sibling_idx + 1,
                                self.siblings.len()
                            ))
                            .color(egui::Color32::from_rgb(142, 142, 147))
                            .size(12.0),
                        );
                    }

                    ui.separator();

                    // File Name & Details
                    let file_name = self
                        .current_path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("Unknown");

                    ui.label(
                        egui::RichText::new(file_name)
                            .strong()
                            .size(15.0)
                            .color(egui::Color32::WHITE),
                    );

                    // Badge: File Size
                    let size_str = format_bytes(self.file_size);
                    ui.label(
                        egui::RichText::new(format!("·  {size_str}"))
                            .color(egui::Color32::from_rgb(160, 160, 165))
                            .size(12.0),
                    );

                    // Badge: Type Details
                    let detail_str = match &self.file_kind {
                        FileKind::Image { width, height, .. } => {
                            if *width > 0 && *height > 0 {
                                format!("·  {width} × {height} px")
                            } else {
                                "·  Image".into()
                            }
                        }
                        FileKind::Code { language, lines, truncated, .. } => {
                            let trunc = if *truncated { " (first 512KB)" } else { "" };
                            format!("·  {} · {} lines{}", language.to_uppercase(), lines, trunc)
                        }
                        FileKind::Markdown { lines, .. } => {
                            format!("·  MARKDOWN · {lines} lines")
                        }
                        FileKind::Spreadsheet { total_rows, total_cols, .. } => {
                            format!("·  Spreadsheet · {total_rows} rows × {total_cols} cols")
                        }
                        FileKind::Presentation { slides, current_slide } => {
                            format!("·  Presentation · Slide {}/{}", current_slide + 1, slides.len())
                        }
                        FileKind::Document { word_count, paragraphs } => {
                            format!("·  Document · ~{word_count} words · {} paragraphs", paragraphs.len())
                        }
                        FileKind::Directory { total_items, .. } => {
                            format!("·  {total_items} items")
                        }
                        FileKind::Archive { total_entries, .. } => {
                            format!("·  ZIP · {total_entries} files")
                        }
                        FileKind::Pdf { page_estimate } => {
                            if let Some(p) = page_estimate {
                                format!("·  PDF · ~{p} pages")
                            } else {
                                "·  PDF Document".into()
                            }
                        }
                        FileKind::Media { kind } => format!("·  {kind} Media"),
                        FileKind::Hex { total_size, .. } => {
                            format!("·  Binary · {}", format_bytes(*total_size))
                        }
                        FileKind::Error(_) => "·  Error".into(),
                    };

                    ui.label(
                        egui::RichText::new(detail_str)
                            .color(egui::Color32::from_rgb(142, 142, 147))
                            .size(12.0),
                    );

                    // Right Aligned Controls
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Open with default app button
                        if ui
                            .button(
                                egui::RichText::new("↗ Open with App")
                                    .color(egui::Color32::from_rgb(220, 220, 255)),
                            )
                            .on_hover_text("Open in default Linux system application (Enter / o)")
                            .clicked()
                        {
                            let _ = open::that(&self.current_path);
                        }

                        // Image zoom controls
                        if matches!(self.file_kind, FileKind::Image { .. }) {
                            if ui.button("100%").clicked() {
                                self.zoom_scale = 1.0;
                                self.pan_offset = egui::Vec2::ZERO;
                            }
                            if ui.button("+").clicked() {
                                self.zoom_scale = (self.zoom_scale * 1.2).min(10.0);
                            }
                            if ui.button("-").clicked() {
                                self.zoom_scale = (self.zoom_scale / 1.2).max(0.1);
                            }
                        }
                    });
                });
            });

        // 2. Main Content Canvas
        egui::CentralPanel::default().show(ctx, |ui| match &mut self.file_kind {
            FileKind::Image { uri, .. } => {
                render_image_viewer(ui, uri, &mut self.zoom_scale, &mut self.pan_offset);
            }
            FileKind::Code { content, language, .. } => {
                render_code_viewer(ui, content, language);
            }
            FileKind::Markdown { content, .. } => {
                render_markdown_viewer(ui, content);
            }
            FileKind::Spreadsheet { rows, total_rows, total_cols } => {
                render_spreadsheet_viewer(ui, rows, *total_rows, *total_cols, &mut self.search_filter);
            }
            FileKind::Presentation { slides, current_slide } => {
                render_presentation_viewer(ui, slides, current_slide);
            }
            FileKind::Document { paragraphs, word_count } => {
                render_document_viewer(ui, paragraphs, *word_count);
            }
            FileKind::Directory { entries, .. } => {
                render_directory_viewer(ui, entries, &mut self.search_filter);
            }
            FileKind::Archive { entries, .. } => {
                render_archive_viewer(ui, entries, &mut self.search_filter);
            }
            FileKind::Pdf { page_estimate } => {
                render_pdf_viewer(ui, &self.current_path, *page_estimate);
            }
            FileKind::Media { kind } => {
                render_media_viewer(ui, &self.current_path, kind, self.file_size);
            }
            FileKind::Hex { data, total_size } => {
                render_hex_viewer(ui, data, *total_size);
            }
            FileKind::Error(msg) => {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        egui::RichText::new(format!("⚠️ {msg}"))
                            .color(egui::Color32::from_rgb(255, 99, 71))
                            .size(16.0),
                    );
                });
            }
        });
    }
}

// ---------------------------------------------------------------------------
// VIEWERS FOR EACH FORMAT
// ---------------------------------------------------------------------------

fn render_image_viewer(
    ui: &mut egui::Ui,
    uri: &str,
    zoom_scale: &mut f32,
    pan_offset: &mut egui::Vec2,
) {
    let available_size = ui.available_size();
    let (response, painter) = ui.allocate_painter(available_size, egui::Sense::drag());

    // Mouse drag for pan
    if response.dragged() {
        *pan_offset += response.drag_delta();
    }

    // Scroll wheel for zoom
    let scroll_delta = ui.input(|i| i.raw_scroll_delta.y);
    if scroll_delta != 0.0 {
        let factor = if scroll_delta > 0.0 { 1.1 } else { 0.9 };
        *zoom_scale = (*zoom_scale * factor).clamp(0.05, 15.0);
    }

    let center = response.rect.center() + *pan_offset;
    let target_size = available_size * (*zoom_scale);

    let image = egui::Image::new(uri)
        .fit_to_exact_size(target_size)
        .shrink_to_fit();

    let image_rect = egui::Rect::from_center_size(center, target_size);
    image.paint_at(ui, image_rect);

    // Subtle instructions at the bottom
    painter.text(
        response.rect.right_bottom() - egui::vec2(16.0, 16.0),
        egui::Align2::RIGHT_BOTTOM,
        format!("{:.0}%  (Drag to pan · Scroll to zoom)", *zoom_scale * 100.0),
        egui::FontId::proportional(11.0),
        egui::Color32::from_rgb(100, 100, 105),
    );
}

fn render_code_viewer(ui: &mut egui::Ui, content: &str, language: &str) {
    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let theme = egui_extras::syntax_highlighting::CodeTheme::dark(13.0);

            let mut layouter = |ui: &egui::Ui, string: &str, _wrap_width: f32| {
                let layout_job = egui_extras::syntax_highlighting::highlight(
                    ui.ctx(),
                    ui.style(),
                    &theme,
                    string,
                    language,
                );
                ui.fonts(|f| f.layout_job(layout_job))
            };

            ui.add(
                egui::TextEdit::multiline(&mut content.to_string())
                    .font(egui::TextStyle::Monospace)
                    .code_editor()
                    .lock_focus(true)
                    .layouter(&mut layouter),
            );
        });
}

fn render_markdown_viewer(ui: &mut egui::Ui, content: &str) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;

            for line in content.lines() {
                if let Some(h1) = line.strip_prefix("# ") {
                    ui.heading(
                        egui::RichText::new(h1)
                            .strong()
                            .size(24.0)
                            .color(egui::Color32::WHITE),
                    );
                    ui.separator();
                } else if let Some(h2) = line.strip_prefix("## ") {
                    ui.heading(
                        egui::RichText::new(h2)
                            .strong()
                            .size(19.0)
                            .color(egui::Color32::from_rgb(220, 220, 225)),
                    );
                } else if let Some(h3) = line.strip_prefix("### ") {
                    ui.label(
                        egui::RichText::new(h3)
                            .strong()
                            .size(16.0)
                            .color(egui::Color32::from_rgb(200, 200, 210)),
                    );
                } else if let Some(bullet) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(" •").color(egui::Color32::from_rgb(0, 122, 255)));
                        ui.label(bullet);
                    });
                } else if line.starts_with("```") {
                    ui.separator();
                } else if line.starts_with("> ") {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("┃").color(egui::Color32::from_rgb(100, 100, 120)));
                        ui.label(
                            egui::RichText::new(&line[2..])
                                .italics()
                                .color(egui::Color32::from_rgb(180, 180, 185)),
                        );
                    });
                } else if !line.trim().is_empty() {
                    ui.label(line);
                }
            }
        });
}

fn render_spreadsheet_viewer(
    ui: &mut egui::Ui,
    rows: &[Vec<String>],
    total_rows: usize,
    total_cols: usize,
    search_filter: &mut String,
) {
    ui.horizontal(|ui| {
        ui.label("🔍 Filter:");
        ui.text_edit_singleline(search_filter);
        ui.label(
            egui::RichText::new(format!("{total_rows} rows × {total_cols} columns"))
                .color(egui::Color32::from_rgb(142, 142, 147))
                .size(12.0),
        );
    });
    ui.separator();

    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("spreadsheet_grid")
                .striped(true)
                .min_col_width(70.0)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    // Column Headers (A, B, C...)
                    ui.label(egui::RichText::new("#").color(egui::Color32::from_rgb(100, 100, 105)));
                    for c in 0..total_cols {
                        let col_letter = if c < 26 {
                            ((b'A' + c as u8) as char).to_string()
                        } else {
                            format!("C{}", c + 1)
                        };
                        ui.label(
                            egui::RichText::new(col_letter)
                                .strong()
                                .color(egui::Color32::from_rgb(150, 150, 210)),
                        );
                    }
                    ui.end_row();

                    for (row_idx, row) in rows.iter().enumerate() {
                        if !search_filter.is_empty() {
                            let match_found = row.iter().any(|cell| {
                                cell.to_lowercase().contains(&search_filter.to_lowercase())
                            });
                            if !match_found {
                                continue;
                            }
                        }

                        // Row index
                        ui.label(
                            egui::RichText::new(format!("{}", row_idx + 1))
                                .monospace()
                                .color(egui::Color32::from_rgb(100, 100, 105)),
                        );

                        for cell in row {
                            let text = if cell.is_empty() { " " } else { cell.as_str() };
                            ui.label(text);
                        }
                        for _ in row.len()..total_cols {
                            ui.label("");
                        }
                        ui.end_row();
                    }
                });
        });
}

fn render_presentation_viewer(
    ui: &mut egui::Ui,
    slides: &[SlideInfo],
    current_slide: &mut usize,
) {
    if slides.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label("Empty presentation");
        });
        return;
    }

    let slide_count = slides.len();
    if *current_slide >= slide_count {
        *current_slide = 0;
    }

    ui.horizontal(|ui| {
        if ui.button("◀ Prev Slide").on_hover_text("PageUp").clicked() && *current_slide > 0 {
            *current_slide -= 1;
        }
        ui.label(
            egui::RichText::new(format!("Slide {} of {}", *current_slide + 1, slide_count))
                .strong()
                .color(egui::Color32::WHITE),
        );
        if ui.button("Next Slide ▶").on_hover_text("PageDown").clicked() && *current_slide + 1 < slide_count {
            *current_slide += 1;
        }
    });

    ui.separator();

    let slide = &slides[*current_slide];

    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(16.0);
            egui::Frame::canvas(ui.style())
                .fill(egui::Color32::from_rgb(32, 34, 40))
                .stroke(egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(60, 62, 70)))
                .rounding(egui::Rounding::same(8.0))
                .inner_margin(egui::Margin::same(32.0))
                .show(ui, |ui| {
                    ui.set_min_width(600.0);
                    ui.set_min_height(340.0);

                    ui.heading(
                        egui::RichText::new(&slide.title)
                            .size(24.0)
                            .strong()
                            .color(egui::Color32::WHITE),
                    );
                    ui.separator();
                    ui.add_space(12.0);

                    for bullet in &slide.bullets {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("▸").color(egui::Color32::from_rgb(0, 122, 255)).size(15.0));
                            ui.label(
                                egui::RichText::new(bullet)
                                    .size(15.0)
                                    .color(egui::Color32::from_rgb(220, 220, 225)),
                            );
                        });
                        ui.add_space(4.0);
                    }
                });
        });
    });
}

fn render_document_viewer(ui: &mut egui::Ui, paragraphs: &[DocParagraph], word_count: usize) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("Word Document · ~{word_count} words · {} paragraphs", paragraphs.len()))
                .color(egui::Color32::from_rgb(142, 142, 147))
                .size(12.0),
        );
    });
    ui.separator();

    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.vertical_centered(|ui| {
            egui::Frame::canvas(ui.style())
                .fill(egui::Color32::from_rgb(30, 30, 32))
                .stroke(egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(50, 50, 56)))
                .rounding(egui::Rounding::same(6.0))
                .inner_margin(egui::Margin::symmetric(40.0, 30.0))
                .show(ui, |ui| {
                    ui.set_max_width(700.0);

                    for para in paragraphs {
                        if para.is_heading {
                            ui.add_space(10.0);
                            ui.heading(
                                egui::RichText::new(&para.text)
                                    .size(19.0)
                                    .strong()
                                    .color(egui::Color32::WHITE),
                            );
                            ui.add_space(4.0);
                        } else {
                            ui.label(
                                egui::RichText::new(&para.text)
                                    .size(14.0)
                                    .color(egui::Color32::from_rgb(220, 220, 225)),
                            );
                            ui.add_space(6.0);
                        }
                    }
                });
        });
    });
}

fn render_directory_viewer(
    ui: &mut egui::Ui,
    entries: &[DirItem],
    search_filter: &mut String,
) {
    ui.horizontal(|ui| {
        ui.label("🔍 Filter:");
        ui.text_edit_singleline(search_filter);
    });

    ui.separator();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("dir_grid")
                .striped(true)
                .min_col_width(120.0)
                .spacing([24.0, 8.0])
                .show(ui, |ui| {
                    ui.label(egui::RichText::new("Name").strong());
                    ui.label(egui::RichText::new("Size").strong());
                    ui.label(egui::RichText::new("Modified").strong());
                    ui.end_row();

                    for item in entries {
                        if !search_filter.is_empty()
                            && !item.name.to_lowercase().contains(&search_filter.to_lowercase())
                        {
                            continue;
                        }

                        let icon = if item.is_dir { "📁" } else { "📄" };
                        let color = if item.is_dir {
                            egui::Color32::from_rgb(100, 180, 255)
                        } else {
                            egui::Color32::from_rgb(220, 220, 225)
                        };

                        ui.horizontal(|ui| {
                            ui.label(icon);
                            ui.label(egui::RichText::new(&item.name).color(color));
                        });
                        ui.label(&item.size_str);
                        ui.label(&item.modified_str);
                        ui.end_row();
                    }
                });
        });
}

fn render_archive_viewer(
    ui: &mut egui::Ui,
    entries: &[String],
    search_filter: &mut String,
) {
    ui.horizontal(|ui| {
        ui.label("🔍 Search Archive:");
        ui.text_edit_singleline(search_filter);
    });
    ui.separator();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for entry in entries {
                if !search_filter.is_empty()
                    && !entry.to_lowercase().contains(&search_filter.to_lowercase())
                {
                    continue;
                }
                ui.horizontal(|ui| {
                    ui.label("📦");
                    ui.label(egui::RichText::new(entry).monospace());
                });
            }
        });
}

fn render_pdf_viewer(ui: &mut egui::Ui, path: &Path, pages: Option<usize>) {
    ui.centered_and_justified(|ui| {
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new("📄").size(72.0));
            ui.add_space(16.0);
            ui.heading(
                egui::RichText::new(
                    path.file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("document.pdf"),
                )
                .size(22.0)
                .color(egui::Color32::WHITE),
            );

            if let Some(p) = pages {
                ui.label(
                    egui::RichText::new(format!("PDF Document · ~{p} pages"))
                        .color(egui::Color32::from_rgb(180, 180, 185)),
                );
            } else {
                ui.label("Portable Document Format");
            }

            ui.add_space(24.0);
            if ui
                .button(
                    egui::RichText::new("Open in Default PDF Viewer")
                        .size(15.0)
                        .color(egui::Color32::WHITE),
                )
                .clicked()
            {
                let _ = open::that(path);
            }
        });
    });
}

fn render_media_viewer(ui: &mut egui::Ui, path: &Path, kind: &str, size: u64) {
    ui.centered_and_justified(|ui| {
        ui.vertical_centered(|ui| {
            let icon = if kind == "Audio" { "🎵" } else { "🎬" };
            ui.label(egui::RichText::new(icon).size(72.0));
            ui.add_space(16.0);
            ui.heading(
                egui::RichText::new(
                    path.file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("media"),
                )
                .size(22.0)
                .color(egui::Color32::WHITE),
            );

            ui.label(
                egui::RichText::new(format!("{kind} Media File · {}", format_bytes(size)))
                    .color(egui::Color32::from_rgb(180, 180, 185)),
            );

            ui.add_space(24.0);
            if ui
                .button(
                    egui::RichText::new("Play in Default Player")
                        .size(15.0)
                        .color(egui::Color32::WHITE),
                )
                .clicked()
            {
                let _ = open::that(path);
            }
        });
    });
}

fn render_hex_viewer(ui: &mut egui::Ui, data: &[u8], total_size: u64) {
    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "Binary Content (showing first {} of {})",
                        format_bytes(data.len() as u64),
                        format_bytes(total_size)
                    ))
                    .color(egui::Color32::from_rgb(142, 142, 147))
                    .size(12.0),
                );
            });
            ui.separator();

            let mut hex_lines = String::with_capacity(data.len() * 4);
            for (chunk_idx, chunk) in data.chunks(16).enumerate() {
                let offset = chunk_idx * 16;
                let mut hex_part = String::new();
                let mut ascii_part = String::new();

                for (i, &byte) in chunk.iter().enumerate() {
                    if i == 8 {
                        hex_part.push(' ');
                    }
                    hex_part.push_str(&format!("{:02x} ", byte));
                    if byte.is_ascii_graphic() || byte == b' ' {
                        ascii_part.push(byte as char);
                    } else {
                        ascii_part.push('·');
                    }
                }

                // Pad hex if less than 16 bytes
                while hex_part.len() < 49 {
                    hex_part.push(' ');
                }

                hex_lines.push_str(&format!(
                    "{:08x}:  {} |{}|\n",
                    offset, hex_part, ascii_part
                ));
            }

            ui.add(
                egui::TextEdit::multiline(&mut hex_lines.as_str())
                    .font(egui::TextStyle::Monospace)
                    .code_editor()
                    .lock_focus(true),
            );
        });
}

// ---------------------------------------------------------------------------
// HELPERS
// ---------------------------------------------------------------------------

fn get_siblings(path: &Path) -> Vec<PathBuf> {
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };

    let mut list = Vec::new();
    if let Ok(read_dir) = fs::read_dir(parent) {
        for entry in read_dir.flatten() {
            let p = entry.path();
            // Ignore hidden files unless the initial path itself was hidden
            let is_hidden = p
                .file_name()
                .and_then(|s| s.to_str())
                .map(|s| s.starts_with('.'))
                .unwrap_or(false);

            let initial_hidden = path
                .file_name()
                .and_then(|s| s.to_str())
                .map(|s| s.starts_with('.'))
                .unwrap_or(false);

            if !is_hidden || initial_hidden {
                list.push(p);
            }
        }
    }

    list.sort_by(|a, b| {
        a.file_name()
            .cmp(&b.file_name())
    });

    list
}

fn load_directory(path: &Path) -> FileKind {
    let mut items = Vec::new();
    if let Ok(read_dir) = fs::read_dir(path) {
        for entry in read_dir.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let metadata = entry.metadata().ok();
            let is_dir = metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false);
            let size_str = metadata
                .as_ref()
                .map(|m| if is_dir { "--".into() } else { format_bytes(m.len()) })
                .unwrap_or_else(|| "--".into());

            let modified_str = metadata
                .as_ref()
                .and_then(|m| m.modified().ok())
                .map(|time| {
                    let dur = std::time::SystemTime::now()
                        .duration_since(time)
                        .unwrap_or_default();
                    if dur.as_secs() < 3600 {
                        format!("{}m ago", dur.as_secs() / 60)
                    } else if dur.as_secs() < 86400 {
                        format!("{}h ago", dur.as_secs() / 3600)
                    } else {
                        format!("{}d ago", dur.as_secs() / 86400)
                    }
                })
                .unwrap_or_else(|| "--".into());

            items.push(DirItem {
                name,
                is_dir,
                size_str,
                modified_str,
            });
        }
    }

    items.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));

    let total_items = items.len();
    FileKind::Directory {
        entries: items,
        total_items,
    }
}

/// Lightweight zero-dependency ZIP central directory parser
fn load_zip_listing(path: &Path, file_size: u64) -> FileKind {
    let mut entries = Vec::new();
    if let Ok(mut file) = File::open(path) {
        let mut buffer = [0u8; 4];
        let mut offset = 0u64;

        // Scan local file headers: signature 0x04034b50 (PK\x03\x04)
        while offset + 30 <= file_size && entries.len() < 2000 {
            if file.seek(SeekFrom::Start(offset)).is_err() {
                break;
            }
            if file.read_exact(&mut buffer).is_err() {
                break;
            }
            if buffer != [0x50, 0x4b, 0x03, 0x04] {
                break;
            }

            let mut header = [0u8; 26];
            if file.read_exact(&mut header).is_err() {
                break;
            }

            let comp_size = u32::from_le_bytes(header[14..18].try_into().unwrap()) as u64;
            let name_len = u16::from_le_bytes(header[22..24].try_into().unwrap()) as usize;
            let extra_len = u16::from_le_bytes(header[24..26].try_into().unwrap()) as usize;

            let mut name_bytes = vec![0u8; name_len];
            if file.read_exact(&mut name_bytes).is_err() {
                break;
            }

            let name = String::from_utf8_lossy(&name_bytes).to_string();
            entries.push(name);

            offset += 30 + (name_len as u64) + (extra_len as u64) + comp_size;
        }
    }

    let total_entries = entries.len();
    FileKind::Archive {
        entries,
        total_entries,
    }
}

fn estimate_pdf_pages(path: &Path) -> Option<usize> {
    let mut file = File::open(path).ok()?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).ok()?;

    let mut count = 0;
    let target = b"/Type /Page";
    let target_alt = b"/Type/Page";

    for window in buffer.windows(target.len()) {
        if window == target {
            count += 1;
        }
    }

    if count == 0 {
        for window in buffer.windows(target_alt.len()) {
            if window == target_alt {
                count += 1;
            }
        }
    }

    if count > 0 {
        Some(count)
    } else {
        None
    }
}

fn detect_language(ext: &str, sample_text: &str) -> String {
    match ext {
        "rs" => "rs".into(),
        "py" | "pyw" => "py".into(),
        "js" | "mjs" | "cjs" => "js".into(),
        "ts" | "mts" | "cts" => "ts".into(),
        "jsx" | "tsx" => "jsx".into(),
        "c" | "h" => "c".into(),
        "cpp" | "cc" | "cxx" | "hpp" => "cpp".into(),
        "go" => "go".into(),
        "java" => "java".into(),
        "json" => "json".into(),
        "toml" => "toml".into(),
        "yaml" | "yml" => "yaml".into(),
        "html" | "htm" => "html".into(),
        "css" | "scss" | "sass" => "css".into(),
        "sh" | "bash" | "zsh" => "sh".into(),
        "sql" => "sql".into(),
        "xml" | "svg" => "xml".into(),
        _ => {
            if sample_text.starts_with("#!/") {
                "sh".into()
            } else if sample_text.trim_start().starts_with('{') {
                "json".into()
            } else {
                "txt".into()
            }
        }
    }
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

fn read_zip_entry(path: &Path, filter: impl Fn(&str) -> bool) -> Vec<(String, Vec<u8>)> {
    let mut results = Vec::new();
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return results,
    };
    let file_size = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mut buffer = [0u8; 4];
    let mut offset = 0u64;

    while offset + 30 <= file_size && results.len() < 100 {
        if file.seek(SeekFrom::Start(offset)).is_err() {
            break;
        }
        if file.read_exact(&mut buffer).is_err() {
            break;
        }
        if buffer != [0x50, 0x4b, 0x03, 0x04] {
            break;
        }

        let mut header = [0u8; 26];
        if file.read_exact(&mut header).is_err() {
            break;
        }

        let method = u16::from_le_bytes(header[4..6].try_into().unwrap());
        let comp_size = u32::from_le_bytes(header[14..18].try_into().unwrap()) as usize;
        let name_len = u16::from_le_bytes(header[22..24].try_into().unwrap()) as usize;
        let extra_len = u16::from_le_bytes(header[24..26].try_into().unwrap()) as usize;

        let mut name_bytes = vec![0u8; name_len];
        if file.read_exact(&mut name_bytes).is_err() {
            break;
        }

        let name = String::from_utf8_lossy(&name_bytes).to_string();
        let data_offset = offset + 30 + (name_len as u64) + (extra_len as u64);

        if filter(&name) {
            if file.seek(SeekFrom::Start(data_offset)).is_ok() {
                let mut comp_data = vec![0u8; comp_size];
                if file.read_exact(&mut comp_data).is_ok() {
                    let decompressed = if method == 0 {
                        Some(comp_data)
                    } else if method == 8 {
                        miniz_oxide::inflate::decompress_to_vec(&comp_data).ok()
                    } else {
                        None
                    };
                    if let Some(data) = decompressed {
                        results.push((name, data));
                    }
                }
            }
        }

        offset = data_offset + (comp_size as u64);
    }

    results
}

fn load_pptx(path: &Path) -> FileKind {
    let entries = read_zip_entry(path, |name| {
        (name.starts_with("ppt/slides/slide") && name.ends_with(".xml"))
            || (name.starts_with("content.xml"))
    });

    let mut slides_data = Vec::new();
    for (name, data) in entries {
        if name.starts_with("ppt/slides/slide") || name == "content.xml" {
            slides_data.push((name, data));
        }
    }

    slides_data.sort_by_key(|(name, _)| {
        name.trim_start_matches("ppt/slides/slide")
            .trim_end_matches(".xml")
            .parse::<usize>()
            .unwrap_or(0)
    });

    let mut slides = Vec::new();
    for (_, data) in slides_data {
        if let Ok(xml) = std::str::from_utf8(&data) {
            let texts = extract_xml_tags(xml, "a:t");
            if !texts.is_empty() {
                let title = texts[0].clone();
                let bullets = texts[1..].to_vec();
                slides.push(SlideInfo { title, bullets });
            } else {
                slides.push(SlideInfo {
                    title: format!("Slide {}", slides.len() + 1),
                    bullets: vec![],
                });
            }
        }
    }

    if slides.is_empty() {
        slides.push(SlideInfo {
            title: "Presentation".into(),
            bullets: vec!["No text extracted or slide in vector graphics format".into()],
        });
    }

    FileKind::Presentation {
        slides,
        current_slide: 0,
    }
}

fn load_xlsx(path: &Path) -> FileKind {
    let entries = read_zip_entry(path, |name| {
        name == "xl/sharedStrings.xml"
            || name == "xl/worksheets/sheet1.xml"
            || name == "content.xml"
    });

    let mut shared_strings = Vec::new();
    let mut sheet_xml = None;

    for (name, data) in entries {
        if name == "xl/sharedStrings.xml" {
            if let Ok(xml) = std::str::from_utf8(&data) {
                shared_strings = extract_xml_tags(xml, "t");
            }
        } else if name == "xl/worksheets/sheet1.xml" || name == "content.xml" {
            if let Ok(xml) = std::str::from_utf8(&data) {
                sheet_xml = Some(xml.to_string());
            }
        }
    }

    if let Some(xml) = sheet_xml {
        let rows = parse_xlsx_sheet(&xml, &shared_strings);
        let total_rows = rows.len();
        let total_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        return FileKind::Spreadsheet {
            rows,
            total_rows,
            total_cols,
        };
    }

    FileKind::Error("Could not extract spreadsheet sheet".into())
}

fn parse_xlsx_sheet(xml: &str, shared_strings: &[String]) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row_cursor = 0;

    while let Some(r_start) = xml[row_cursor..].find("<row") {
        let abs_r_start = row_cursor + r_start;
        if let Some(r_end) = xml[abs_r_start..].find("</row>") {
            let row_xml = &xml[abs_r_start..abs_r_start + r_end];
            let mut cells = Vec::new();
            let mut c_cursor = 0;

            while let Some(c_start) = row_xml[c_cursor..].find("<c") {
                let abs_c_start = c_cursor + c_start;
                if let Some(c_tag_close) = row_xml[abs_c_start..].find('>') {
                    let c_header = &row_xml[abs_c_start..abs_c_start + c_tag_close];
                    let is_shared = c_header.contains("t=\"s\"");

                    let cell_content_end = row_xml[abs_c_start..].find("</c>").unwrap_or(c_tag_close);
                    let cell_body = &row_xml[abs_c_start + c_tag_close..abs_c_start + cell_content_end];

                    let mut val = String::new();
                    if let Some(v_start) = cell_body.find("<v>") {
                        if let Some(v_end) = cell_body[v_start + 3..].find("</v>") {
                            let raw_v = &cell_body[v_start + 3..v_start + 3 + v_end];
                            if is_shared {
                                if let Ok(idx) = raw_v.trim().parse::<usize>() {
                                    if idx < shared_strings.len() {
                                        val = shared_strings[idx].clone();
                                    }
                                }
                            } else {
                                val = raw_v.trim().to_string();
                            }
                        }
                    }
                    cells.push(val);
                    c_cursor = abs_c_start + cell_content_end;
                } else {
                    break;
                }
            }

            if !cells.is_empty() {
                rows.push(cells);
            }
            if rows.len() >= 500 {
                break;
            }
            row_cursor = abs_r_start + r_end + 6;
        } else {
            break;
        }
    }
    rows
}

fn load_docx(path: &Path) -> FileKind {
    let entries = read_zip_entry(path, |name| name == "word/document.xml" || name == "content.xml");
    if let Some((_, data)) = entries.into_iter().next() {
        if let Ok(xml) = std::str::from_utf8(&data) {
            let paragraphs = extract_docx_paragraphs(xml);
            let word_count: usize = paragraphs
                .iter()
                .map(|p| p.text.split_whitespace().count())
                .sum();
            return FileKind::Document {
                paragraphs,
                word_count,
            };
        }
    }
    FileKind::Error("Could not read Word document contents".into())
}

fn extract_docx_paragraphs(xml: &str) -> Vec<DocParagraph> {
    let mut paragraphs = Vec::new();
    let mut cursor = 0;

    while let Some(p_start) = xml[cursor..].find("<w:p") {
        let abs_p_start = cursor + p_start;
        if let Some(p_end) = xml[abs_p_start..].find("</w:p>") {
            let p_body = &xml[abs_p_start..abs_p_start + p_end];
            let is_heading = p_body.contains("Heading") || p_body.contains("heading");
            let text_parts = extract_xml_tags(p_body, "w:t");
            let text = text_parts.join("");
            if !text.trim().is_empty() {
                paragraphs.push(DocParagraph {
                    text: text.trim().to_string(),
                    is_heading,
                });
            }
            cursor = abs_p_start + p_end + 6;
        } else {
            break;
        }
    }
    paragraphs
}

fn load_csv_or_tsv(content: &str, delimiter: char) -> FileKind {
    let mut rows = Vec::new();
    for line in content.lines().take(500) {
        let cols: Vec<String> = line
            .split(delimiter)
            .map(|s| s.trim_matches('"').trim().to_string())
            .collect();
        rows.push(cols);
    }
    let total_rows = rows.len();
    let total_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    FileKind::Spreadsheet {
        rows,
        total_rows,
        total_cols,
    }
}

fn load_legacy_office(path: &Path, kind_name: &'static str) -> FileKind {
    let mut text_runs = Vec::new();
    if let Ok(mut f) = File::open(path) {
        let mut buf = Vec::new();
        if f.read_to_end(&mut buf).is_ok() {
            let mut current = String::new();
            for &byte in &buf {
                if byte.is_ascii_graphic() || byte == b' ' {
                    current.push(byte as char);
                } else {
                    if current.trim().len() >= 4 {
                        text_runs.push(current.trim().to_string());
                    }
                    current.clear();
                }
                if text_runs.len() >= 60 {
                    break;
                }
            }
        }
    }

    let mut paragraphs = vec![DocParagraph {
        is_heading: true,
        text: format!("{kind_name} (Legacy Format)"),
    }];

    for text in text_runs {
        paragraphs.push(DocParagraph {
            is_heading: false,
            text,
        });
    }

    FileKind::Document {
        paragraphs,
        word_count: 0,
    }
}

fn extract_xml_tags(xml: &str, tag: &str) -> Vec<String> {
    let mut results = Vec::new();
    let start_pattern = format!("<{tag}");
    let end_pattern = format!("</{tag}>");
    let mut cursor = 0;

    while let Some(start_idx) = xml[cursor..].find(&start_pattern) {
        let abs_start = cursor + start_idx;
        let after_tag = abs_start + start_pattern.len();
        if after_tag < xml.len() {
            let next_ch = xml.as_bytes()[after_tag];
            if next_ch != b'>' && next_ch != b' ' && next_ch != b'/' {
                cursor = after_tag;
                continue;
            }
        }
        if let Some(close_bracket) = xml[abs_start..].find('>') {
            let val_start = abs_start + close_bracket + 1;
            if let Some(end_idx) = xml[val_start..].find(&end_pattern) {
                let text = &xml[val_start..val_start + end_idx];
                if !text.is_empty() {
                    results.push(decode_xml_entities(text));
                }
                cursor = val_start + end_idx + end_pattern.len();
                continue;
            }
        }
        cursor = abs_start + start_pattern.len();
    }
    results
}

fn decode_xml_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1048576), "1.0 MB");
        assert_eq!(format_bytes(1073741824), "1.00 GB");
    }

    #[test]
    fn test_detect_language() {
        assert_eq!(detect_language("rs", ""), "rs");
        assert_eq!(detect_language("py", ""), "py");
        assert_eq!(detect_language("js", ""), "js");
        assert_eq!(detect_language("json", ""), "json");
        assert_eq!(detect_language("unknown", "#!/bin/bash\necho hi"), "sh");
        assert_eq!(detect_language("unknown", "{\n  \"key\": 1\n}"), "json");
        assert_eq!(detect_language("unknown", "random plain text"), "txt");
    }

    #[test]
    fn test_siblings_discovery() {
        let siblings = get_siblings(Path::new("Cargo.toml"));
        assert!(!siblings.is_empty());
        assert!(siblings.iter().any(|p| p.file_name().unwrap() == "Cargo.toml"));
    }

    #[test]
    fn test_extract_xml_tags() {
        let xml = "<p><a:t>Slide Title</a:t><other><a:t>Bullet 1</a:t></other></p>";
        let tags = extract_xml_tags(xml, "a:t");
        assert_eq!(tags, vec!["Slide Title", "Bullet 1"]);
    }

    #[test]
    fn test_csv_parser() {
        let csv = "Name,Age,City\nAlice,30,Berlin\nBob,25,Paris";
        if let FileKind::Spreadsheet { rows, total_rows, total_cols } = load_csv_or_tsv(csv, ',') {
            assert_eq!(total_rows, 3);
            assert_eq!(total_cols, 3);
            assert_eq!(rows[0], vec!["Name", "Age", "City"]);
            assert_eq!(rows[1], vec!["Alice", "30", "Berlin"]);
        } else {
            panic!("Expected spreadsheet");
        }
    }
}
