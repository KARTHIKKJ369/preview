use eframe::egui;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Instant;

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
        Some(parse_input_path(&args[1]))
    } else {
        detect_active_selection()
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
    Welcome,
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
        current_page: usize,
        total_pages: usize,
        page_image: Option<egui::ColorImage>,
    },
    Media {
        kind: &'static str,
        poster_image: Option<egui::ColorImage>,
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
    current_path: Option<PathBuf>,
    siblings: Vec<PathBuf>,
    sibling_idx: usize,
    file_size: u64,
    file_kind: FileKind,
    active_texture: Option<egui::TextureHandle>,
    zoom_scale: f32,
    pan_offset: egui::Vec2,
    search_filter: String,
    media_player: Option<MediaPlayer>,
}

impl QuickLookApp {
    pub fn new(target: Option<PathBuf>) -> Self {
        let mut app = Self {
            current_path: None,
            siblings: Vec::new(),
            sibling_idx: 0,
            file_size: 0,
            file_kind: FileKind::Welcome,
            active_texture: None,
            zoom_scale: 1.0,
            pan_offset: egui::Vec2::ZERO,
            search_filter: String::new(),
            media_player: None,
        };

        if let Some(t) = target {
            app.set_target(t);
        }
        app
    }

    fn set_target(&mut self, target: PathBuf) {
        if let Some(mut mp) = self.media_player.take() {
            mp.stop();
        }
        let abs_path = fs::canonicalize(&target).unwrap_or(target);
        self.siblings = get_siblings(&abs_path);
        self.sibling_idx = self.siblings.iter().position(|p| p == &abs_path).unwrap_or(0);
        self.current_path = Some(abs_path);
        self.active_texture = None;
        self.zoom_scale = 1.0;
        self.pan_offset = egui::Vec2::ZERO;
        self.search_filter.clear();
        self.load_current_file();
    }

    fn navigate(&mut self, delta: isize) {
        if self.siblings.is_empty() {
            return;
        }
        if let Some(mut mp) = self.media_player.take() {
            mp.stop();
        }
        let len = self.siblings.len() as isize;
        let new_idx = ((self.sibling_idx as isize + delta).rem_euclid(len)) as usize;
        self.sibling_idx = new_idx;
        self.current_path = Some(self.siblings[new_idx].clone());
        self.active_texture = None;
        self.zoom_scale = 1.0;
        self.pan_offset = egui::Vec2::ZERO;
        self.search_filter.clear();
        self.load_current_file();
    }

    fn load_current_file(&mut self) {
        let path = match self.current_path.as_deref() {
            Some(p) => p,
            None => {
                self.file_kind = FileKind::Welcome;
                return;
            }
        };

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
                let poster_image = extract_audio_art_to_image(path);
                let (duration, dims) = probe_media_info(path);
                self.media_player = Some(MediaPlayer::new(path, false, duration, dims));
                self.file_kind = FileKind::Media {
                    kind: "Audio",
                    poster_image,
                };
                return;
            }
            "mp4" | "mkv" | "mov" | "webm" | "avi" | "wmv" | "flv" => {
                let poster_image = extract_video_frame_to_image(path);
                let (duration, dims) = probe_media_info(path);
                self.media_player = Some(MediaPlayer::new(path, true, duration, dims));
                self.file_kind = FileKind::Media {
                    kind: "Video",
                    poster_image,
                };
                return;
            }
            _ => {}
        }

        // 7. PDF
        if ext == "pdf" {
            let total_pages = pdf_get_total_pages(path);
            let page_image = render_pdf_page_to_image(path, 1);
            self.file_kind = FileKind::Pdf {
                current_page: 1,
                total_pages,
                page_image,
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
        // Drain incoming video frames and update media playback timer
        if let Some(player) = &mut self.media_player {
            if let Some(rx) = &player.frame_receiver {
                let mut latest = None;
                while let Ok(img) = rx.try_recv() {
                    latest = Some(img);
                }
                if let Some(img) = latest {
                    if let Some(tex) = &mut self.active_texture {
                        tex.set(img, egui::TextureOptions::LINEAR);
                    } else {
                        self.active_texture = Some(ctx.load_texture("video_frame", img, egui::TextureOptions::LINEAR));
                    }
                }
            }

            if player.is_playing {
                let now = Instant::now();
                let dt = now.duration_since(player.last_tick).as_secs_f64();
                player.last_tick = now;
                player.current_time += dt;
                if player.duration > 0.0 && player.current_time >= player.duration {
                    player.pause_playback();
                    player.current_time = player.duration;
                }
                ctx.request_repaint();
            }
        }

        // Keyboard Shortcuts (macOS Quick Look Behavior)
        ctx.input(|i| {
            if i.key_pressed(egui::Key::Escape)
                || i.key_pressed(egui::Key::Space)
                || i.key_pressed(egui::Key::Q)
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            if i.key_pressed(egui::Key::P) {
                if let Some(player) = &mut self.media_player {
                    player.toggle_playback();
                }
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
                if let FileKind::Pdf { current_page, total_pages, page_image } = &mut self.file_kind {
                    if *current_page < *total_pages {
                        *current_page += 1;
                        if let Some(ref path) = self.current_path {
                            *page_image = render_pdf_page_to_image(path, *current_page);
                            self.active_texture = None;
                        }
                    }
                }
            }
            if i.key_pressed(egui::Key::PageUp) {
                if let FileKind::Presentation { current_slide, .. } = &mut self.file_kind {
                    if *current_slide > 0 {
                        *current_slide -= 1;
                    }
                }
                if let FileKind::Pdf { current_page, page_image, .. } = &mut self.file_kind {
                    if *current_page > 1 {
                        *current_page -= 1;
                        if let Some(ref path) = self.current_path {
                            *page_image = render_pdf_page_to_image(path, *current_page);
                            self.active_texture = None;
                        }
                    }
                }
            }
            if i.key_pressed(egui::Key::O) || i.key_pressed(egui::Key::Enter) {
                if let Some(ref path) = self.current_path {
                    let _ = open::that(path);
                }
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
                    if let Some(ref current_path) = self.current_path {
                        let file_name = current_path
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
                    } else {
                        ui.label(
                            egui::RichText::new("Quick Look")
                                .strong()
                                .size(15.0)
                                .color(egui::Color32::WHITE),
                        );
                    }

                    // Badge: Type Details
                    let detail_str = match &self.file_kind {
                        FileKind::Welcome => "".into(),
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
                        FileKind::Pdf { current_page, total_pages, .. } => {
                            format!("·  PDF · Page {current_page}/{total_pages}")
                        }
                        FileKind::Media { kind, .. } => format!("·  {kind} Media"),
                        FileKind::Hex { total_size, .. } => {
                            format!("·  Binary · {}", format_bytes(*total_size))
                        }
                        FileKind::Error(_) => "·  Error".into(),
                    };

                    if !detail_str.is_empty() {
                        ui.label(
                            egui::RichText::new(detail_str)
                                .color(egui::Color32::from_rgb(142, 142, 147))
                                .size(12.0),
                        );
                    }

                    // Right Aligned Controls
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Open with default app button
                        if let Some(ref path) = self.current_path {
                            if ui
                                .button(
                                    egui::RichText::new("↗ Open with App")
                                        .color(egui::Color32::from_rgb(220, 220, 255)),
                                )
                                .on_hover_text("Open in default Linux system application (Enter / o)")
                                .clicked()
                            {
                                let _ = open::that(path);
                            }
                        } else if ui.button("📂 Choose File...").clicked() {
                            if let Some(p) = prompt_file_dialog() {
                                self.set_target(p);
                            }
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
        let mut target_to_set = None;
        egui::CentralPanel::default().show(ctx, |ui| match &mut self.file_kind {
            FileKind::Welcome => {
                if let Some(p) = render_welcome_viewer(ui) {
                    target_to_set = Some(p);
                }
            }
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
            FileKind::Pdf { current_page, total_pages, page_image } => {
                if self.active_texture.is_none() {
                    if let Some(img) = page_image.take() {
                        self.active_texture = Some(ctx.load_texture("pdf_page", img, egui::TextureOptions::LINEAR));
                    }
                }
                let p = self.current_path.as_deref().unwrap_or(Path::new("document.pdf"));
                if let Some(new_page) = render_pdf_viewer(
                    ui,
                    p,
                    current_page,
                    *total_pages,
                    self.active_texture.as_ref(),
                    &mut self.zoom_scale,
                    &mut self.pan_offset,
                ) {
                    *page_image = render_pdf_page_to_image(p, new_page);
                    self.active_texture = None;
                }
            }
            FileKind::Media { kind, poster_image } => {
                if let Some(player) = &mut self.media_player {
                    render_media_player(
                        ui,
                        player,
                        self.active_texture.as_ref(),
                        self.file_size,
                        &mut self.zoom_scale,
                        &mut self.pan_offset,
                    );
                } else {
                    if self.active_texture.is_none() {
                        if let Some(img) = poster_image.take() {
                            self.active_texture = Some(ctx.load_texture("media_poster", img, egui::TextureOptions::LINEAR));
                        }
                    }
                    let p = self.current_path.as_deref().unwrap_or(Path::new("media"));
                    render_media_viewer(
                        ui,
                        p,
                        kind,
                        self.file_size,
                        self.active_texture.as_ref(),
                        &mut self.zoom_scale,
                        &mut self.pan_offset,
                    );
                }
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

        if let Some(target) = target_to_set {
            self.set_target(target);
        }
    }
}

impl Drop for QuickLookApp {
    fn drop(&mut self) {
        if let Some(mut mp) = self.media_player.take() {
            mp.stop();
        }
    }
}

// ---------------------------------------------------------------------------
// VIEWERS FOR EACH FORMAT
// ---------------------------------------------------------------------------

fn render_welcome_viewer(ui: &mut egui::Ui) -> Option<PathBuf> {
    let mut selected = None;
    ui.centered_and_justified(|ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(24.0);
            ui.label(egui::RichText::new("🔍").size(64.0));
            ui.add_space(12.0);
            ui.heading(
                egui::RichText::new("Quick Look")
                    .size(24.0)
                    .strong()
                    .color(egui::Color32::WHITE),
            );
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("Select any file in Dolphin and press Space to preview instantly.")
                    .size(15.0)
                    .color(egui::Color32::from_rgb(170, 170, 180)),
            );
            ui.add_space(18.0);

            if ui
                .button(
                    egui::RichText::new("📂  Choose File to Preview...")
                        .size(14.0)
                        .color(egui::Color32::WHITE),
                )
                .clicked()
            {
                if let Some(path) = prompt_file_dialog() {
                    selected = Some(path);
                }
            }

            ui.add_space(28.0);
            egui::Frame::canvas(ui.style())
                .fill(egui::Color32::from_rgb(32, 34, 40))
                .stroke(egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(60, 62, 70)))
                .rounding(egui::Rounding::same(8.0))
                .inner_margin(egui::Margin::symmetric(24.0, 16.0))
                .show(ui, |ui| {
                    ui.set_max_width(520.0);
                    ui.label(
                        egui::RichText::new("💡 Dolphin / KDE Spacebar Setup:")
                            .strong()
                            .color(egui::Color32::from_rgb(0, 122, 255)),
                    );
                    ui.add_space(4.0);
                    ui.label("1. Open Dolphin → Settings → Configure Shortcuts");
                    ui.label("2. Search for 'Quick Look'");
                    ui.label("3. Assign shortcut to Space key");
                });
        });
    });
    selected
}

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

fn render_pdf_viewer(
    ui: &mut egui::Ui,
    path: &Path,
    current_page: &mut usize,
    total_pages: usize,
    texture: Option<&egui::TextureHandle>,
    zoom_scale: &mut f32,
    pan_offset: &mut egui::Vec2,
) -> Option<usize> {
    let mut page_change = None;

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;

        if ui.button("◀ Prev Page").on_hover_text("PageUp / [").clicked() && *current_page > 1 {
            *current_page -= 1;
            page_change = Some(*current_page);
        }

        ui.label(
            egui::RichText::new(format!("Page {} of {}", *current_page, total_pages))
                .strong()
                .color(egui::Color32::WHITE),
        );

        if ui.button("Next Page ▶").on_hover_text("PageDown / ]").clicked() && *current_page < total_pages {
            *current_page += 1;
            page_change = Some(*current_page);
        }

        ui.separator();

        if ui.button("100%").clicked() {
            *zoom_scale = 1.0;
            *pan_offset = egui::Vec2::ZERO;
        }
        if ui.button("+").clicked() {
            *zoom_scale = (*zoom_scale * 1.2).min(10.0);
        }
        if ui.button("-").clicked() {
            *zoom_scale = (*zoom_scale / 1.2).max(0.1);
        }
    });
    ui.separator();

    if let Some(tex) = texture {
        render_texture_interactive(ui, tex, zoom_scale, pan_offset);
    } else {
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

                ui.label(
                    egui::RichText::new(format!("PDF Document · {total_pages} pages"))
                        .color(egui::Color32::from_rgb(180, 180, 185)),
                );

                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("💡 To enable in-window page rendering, install poppler-utils:")
                        .color(egui::Color32::from_rgb(140, 140, 150))
                        .size(13.0),
                );
                ui.label(
                    egui::RichText::new("sudo apt install poppler-utils  (or pacman -S poppler)")
                        .monospace()
                        .color(egui::Color32::from_rgb(180, 180, 220)),
                );

                ui.add_space(20.0);
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

    page_change
}

pub struct MediaPlayer {
    pub is_video: bool,
    pub path: PathBuf,
    pub is_playing: bool,
    pub current_time: f64,
    pub duration: f64,
    pub volume: f32,
    pub dimensions: (u32, u32),
    pub frame_receiver: Option<Receiver<egui::ColorImage>>,
    pub stop_sender: Option<Sender<()>>,
    pub audio_child: Option<Child>,
    pub video_child: Option<Child>,
    pub last_tick: Instant,
}

impl MediaPlayer {
    pub fn new(
        path: &Path,
        is_video: bool,
        duration: f64,
        dimensions: (u32, u32),
    ) -> Self {
        let mut player = Self {
            is_video,
            path: path.to_path_buf(),
            is_playing: false,
            current_time: 0.0,
            duration: if duration > 0.0 { duration } else { 1.0 },
            volume: 1.0,
            dimensions,
            frame_receiver: None,
            stop_sender: None,
            audio_child: None,
            video_child: None,
            last_tick: Instant::now(),
        };

        player.start_playback();
        player
    }

    pub fn start_playback(&mut self) {
        self.stop_processes();
        self.is_playing = true;
        self.last_tick = Instant::now();

        self.audio_child = spawn_audio_process(&self.path, self.current_time, self.volume);

        if self.is_video {
            let (tx, rx) = channel();
            let (stop_tx, stop_rx) = channel();
            let target_dims = calculate_target_dims(self.dimensions.0, self.dimensions.1, 854, 480);
            self.video_child = spawn_video_stream(&self.path, self.current_time, target_dims, tx, stop_rx);
            self.frame_receiver = Some(rx);
            self.stop_sender = Some(stop_tx);
        }
    }

    pub fn pause_playback(&mut self) {
        self.is_playing = false;
        self.stop_processes();
    }

    pub fn toggle_playback(&mut self) {
        if self.is_playing {
            self.pause_playback();
        } else {
            if self.current_time >= self.duration {
                self.current_time = 0.0;
            }
            self.start_playback();
        }
    }

    pub fn seek(&mut self, new_time: f64) {
        self.current_time = new_time.clamp(0.0, self.duration);
        if self.is_playing {
            self.start_playback();
        }
    }

    pub fn stop_processes(&mut self) {
        if let Some(stop_tx) = self.stop_sender.take() {
            let _ = stop_tx.send(());
        }
        if let Some(mut child) = self.video_child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(mut child) = self.audio_child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.frame_receiver = None;
    }

    pub fn stop(&mut self) {
        self.is_playing = false;
        self.stop_processes();
    }
}

impl Drop for MediaPlayer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn render_media_player(
    ui: &mut egui::Ui,
    player: &mut MediaPlayer,
    texture: Option<&egui::TextureHandle>,
    _size: u64,
    zoom_scale: &mut f32,
    pan_offset: &mut egui::Vec2,
) {
    let available_rect = ui.available_rect_before_wrap();
    let mut seek_to = None;
    let mut toggle_play = false;

    if player.is_video {
        let video_area_height = (available_rect.height() - 70.0).max(100.0);
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(available_rect.width(), video_area_height),
            egui::Sense::click_and_drag(),
        );

        if response.clicked() {
            toggle_play = true;
        }
        if response.dragged() {
            *pan_offset += response.drag_delta();
        }

        let scroll_delta = ui.input(|i| i.raw_scroll_delta.y);
        if scroll_delta != 0.0 {
            let factor = if scroll_delta > 0.0 { 1.1 } else { 0.9 };
            *zoom_scale = (*zoom_scale * factor).clamp(0.1, 10.0);
        }

        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(12, 12, 14));

        if let Some(tex) = texture {
            let aspect = if player.dimensions.1 > 0 {
                (player.dimensions.0 as f32 / player.dimensions.1 as f32).max(0.1)
            } else {
                16.0 / 9.0
            };

            let mut fit_w = rect.width();
            let mut fit_h = fit_w / aspect;
            if fit_h > rect.height() {
                fit_h = rect.height();
                fit_w = fit_h * aspect;
            }

            fit_w *= *zoom_scale;
            fit_h *= *zoom_scale;

            let center = rect.center() + *pan_offset;
            let img_rect = egui::Rect::from_center_size(center, egui::vec2(fit_w, fit_h));
            let image = egui::Image::new(tex).fit_to_exact_size(egui::vec2(fit_w, fit_h));
            image.paint_at(ui, img_rect);

            // Subtle pause indicator overlay when paused
            if !player.is_playing {
                let badge_center = rect.center();
                painter.circle_filled(badge_center, 36.0, egui::Color32::from_black_alpha(160));
                painter.text(
                    badge_center,
                    egui::Align2::CENTER_CENTER,
                    "▶",
                    egui::FontId::proportional(28.0),
                    egui::Color32::WHITE,
                );
            }
        } else {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "🎬 Starting video playback...",
                egui::FontId::proportional(16.0),
                egui::Color32::from_rgb(160, 160, 170),
            );
        }
    } else {
        // Audio Viewport Area
        let audio_area_height = (available_rect.height() - 70.0).max(100.0);
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(available_rect.width(), audio_area_height),
            egui::Sense::click(),
        );

        if response.clicked() {
            toggle_play = true;
        }

        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(20, 20, 24));

        let center_y = rect.center().y - 20.0;
        painter.text(
            egui::pos2(rect.center().x, center_y - 40.0),
            egui::Align2::CENTER_CENTER,
            "🎵",
            egui::FontId::proportional(48.0),
            egui::Color32::WHITE,
        );

        let name = player.path.file_name().and_then(|s| s.to_str()).unwrap_or("Audio Track");
        painter.text(
            egui::pos2(rect.center().x, center_y + 10.0),
            egui::Align2::CENTER_CENTER,
            name,
            egui::FontId::proportional(18.0),
            egui::Color32::WHITE,
        );

        // Animated Equalizer Waveform Bars (macOS style)
        let bar_count = 32;
        let bar_w = 4.0;
        let spacing = 4.0;
        let total_w = bar_count as f32 * (bar_w + spacing);
        let start_x = rect.center().x - (total_w / 2.0);
        let base_y = center_y + 60.0;
        let t = player.current_time as f32;

        for i in 0..bar_count {
            let x = start_x + (i as f32 * (bar_w + spacing));
            let height = if player.is_playing {
                let wave = (t * 6.0 + i as f32 * 0.4).sin().abs() * 0.6
                    + (t * 11.0 + i as f32 * 0.9).sin().abs() * 0.4;
                6.0 + (wave * 30.0)
            } else {
                6.0
            };
            let bar_rect = egui::Rect::from_min_size(
                egui::pos2(x, base_y - (height / 2.0)),
                egui::vec2(bar_w, height),
            );
            let color = if player.is_playing {
                egui::Color32::from_rgb(0, 122, 255) // macOS Accent Blue
            } else {
                egui::Color32::from_rgb(80, 80, 90)
            };
            painter.rect_filled(bar_rect, 2.0, color);
        }
    }

    // Floating macOS Glass HUD at Bottom
    egui::Frame::canvas(ui.style())
        .fill(egui::Color32::from_black_alpha(220))
        .stroke(egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(50, 50, 60)))
        .rounding(egui::Rounding::same(8.0))
        .inner_margin(egui::Margin::symmetric(14.0, 8.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                // Play / Pause Button
                let play_icon = if player.is_playing { "⏸" } else { "▶" };
                if ui
                    .button(egui::RichText::new(play_icon).size(16.0).strong())
                    .on_hover_text("Play / Pause (Click or press P)")
                    .clicked()
                {
                    toggle_play = true;
                }

                // Time Elapsed
                ui.label(
                    egui::RichText::new(format_time(player.current_time))
                        .monospace()
                        .size(12.0)
                        .color(egui::Color32::from_rgb(200, 200, 210)),
                );

                // Scrubber Slider
                let mut slider_val = player.current_time;
                let slider_resp = ui.add_sized(
                    [ui.available_width() - 210.0, 18.0],
                    egui::Slider::new(&mut slider_val, 0.0..=player.duration.max(1.0))
                        .show_value(false),
                );

                if slider_resp.changed() {
                    seek_to = Some(slider_val);
                }

                // Time Total
                ui.label(
                    egui::RichText::new(format_time(player.duration))
                        .monospace()
                        .size(12.0)
                        .color(egui::Color32::from_rgb(140, 140, 150)),
                );

                ui.separator();

                // Volume Icon + Slider
                let vol_icon = if player.volume <= 0.01 { "🔇" } else { "🔊" };
                ui.label(vol_icon);
                ui.add_sized(
                    [55.0, 18.0],
                    egui::Slider::new(&mut player.volume, 0.0..=1.0).show_value(false),
                );

                // Open with App
                if ui
                    .button(
                        egui::RichText::new("↗")
                            .color(egui::Color32::from_rgb(200, 200, 255)),
                    )
                    .on_hover_text("Open in default system player")
                    .clicked()
                {
                    let _ = open::that(&player.path);
                }
            });
        });

    if toggle_play {
        player.toggle_playback();
    }
    if let Some(pos) = seek_to {
        player.seek(pos);
    }
}

fn spawn_audio_process(path: &Path, start_secs: f64, volume: f32) -> Option<Child> {
    let path_str = path.to_str()?;
    let vol_int = (volume.clamp(0.0, 1.0) * 100.0) as u32;
    let start_str = format!("{:.2}", start_secs.max(0.0));

    if let Ok(child) = Command::new("ffplay")
        .args([
            "-nodisp",
            "-autoexit",
            "-loglevel", "quiet",
            "-ss", &start_str,
            "-volume", &vol_int.to_string(),
            path_str,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        return Some(child);
    }

    if let Ok(child) = Command::new("mpv")
        .args([
            "--no-video",
            &format!("--start={start_str}"),
            &format!("--volume={vol_int}"),
            path_str,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        return Some(child);
    }

    None
}

fn spawn_video_stream(
    path: &Path,
    start_secs: f64,
    target_dims: (u32, u32),
    tx: Sender<egui::ColorImage>,
    stop_rx: Receiver<()>,
) -> Option<Child> {
    let path_str = path.to_str()?;
    let (w, h) = target_dims;
    let start_str = format!("{:.2}", start_secs.max(0.0));
    let scale_filter = format!("scale={w}:{h}");

    let mut cmd = Command::new("ffmpeg");
    cmd.args([
        "-re",
        "-ss", &start_str,
        "-i", path_str,
        "-vf", &scale_filter,
        "-f", "rawvideo",
        "-pix_fmt", "rgba",
        "-",
    ])
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::null());

    let mut child = cmd.spawn().ok()?;
    let mut stdout = child.stdout.take()?;

    std::thread::spawn(move || {
        let frame_size = (w * h * 4) as usize;
        let mut buffer = vec![0u8; frame_size];

        while stop_rx.try_recv().is_err() {
            if stdout.read_exact(&mut buffer).is_err() {
                break;
            }
            let color_image = egui::ColorImage::from_rgba_unmultiplied(
                [w as usize, h as usize],
                &buffer,
            );
            if tx.send(color_image).is_err() {
                break;
            }
        }
    });

    Some(child)
}

fn calculate_target_dims(orig_w: u32, orig_h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if orig_w == 0 || orig_h == 0 {
        return (640, 360);
    }
    let aspect = orig_w as f32 / orig_h as f32;
    let mut w = max_w;
    let mut h = (w as f32 / aspect).round() as u32;

    if h > max_h {
        h = max_h;
        w = (h as f32 * aspect).round() as u32;
    }

    w = (w / 2) * 2;
    h = (h / 2) * 2;
    (w.max(320), h.max(180))
}

fn format_time(seconds: f64) -> String {
    let total_secs = seconds.max(0.0) as u64;
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    format!("{mins:02}:{secs:02}")
}

fn probe_media_info(path: &Path) -> (f64, (u32, u32)) {
    let mut duration = 0.0;
    let mut dims = (854, 480);
    let path_str = match path.to_str() {
        Some(s) => s,
        None => return (duration, dims),
    };

    if let Ok(out) = Command::new("ffprobe")
        .args([
            "-v", "error",
            "-show_entries", "format=duration",
            "-of", "default=noprint_wrappers=1:nokey=1",
            path_str,
        ])
        .output()
    {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            if let Ok(d) = s.trim().parse::<f64>() {
                duration = d;
            }
        }
    }

    if let Ok(out) = Command::new("ffprobe")
        .args([
            "-v", "error",
            "-select_streams", "v:0",
            "-show_entries", "stream=width,height",
            "-of", "csv=s=x:p=0",
            path_str,
        ])
        .output()
    {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let parts: Vec<&str> = s.split('x').collect();
            if parts.len() == 2 {
                if let (Ok(w), Ok(h)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
                    if w > 0 && h > 0 {
                        dims = (w, h);
                    }
                }
            }
        }
    }

    (duration, dims)
}

fn render_media_viewer(
    ui: &mut egui::Ui,
    path: &Path,
    kind: &str,
    size: u64,
    texture: Option<&egui::TextureHandle>,
    zoom_scale: &mut f32,
    pan_offset: &mut egui::Vec2,
) {
    if let Some(tex) = texture {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("{kind} Preview · {}", format_bytes(size)))
                        .color(egui::Color32::from_rgb(142, 142, 147))
                        .size(12.0),
                );
                ui.separator();
                if ui
                    .button(
                        egui::RichText::new("▶ Play in Player")
                            .color(egui::Color32::from_rgb(120, 220, 120)),
                    )
                    .on_hover_text("Open in system player (Enter / o)")
                    .clicked()
                {
                    let _ = open::that(path);
                }
            });
            ui.separator();
            render_texture_interactive(ui, tex, zoom_scale, pan_offset);
        });
    } else {
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

                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("💡 To enable in-window video frame preview, install ffmpeg:")
                        .color(egui::Color32::from_rgb(140, 140, 150))
                        .size(13.0),
                );
                ui.label(
                    egui::RichText::new("sudo apt install ffmpeg  (or pacman -S ffmpeg)")
                        .monospace()
                        .color(egui::Color32::from_rgb(180, 180, 220)),
                );

                ui.add_space(20.0);
                if ui
                    .button(
                        egui::RichText::new("▶ Play in Default Player")
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
}

fn render_texture_interactive(
    ui: &mut egui::Ui,
    texture: &egui::TextureHandle,
    zoom_scale: &mut f32,
    pan_offset: &mut egui::Vec2,
) {
    let available_size = ui.available_size();
    let (response, painter) = ui.allocate_painter(available_size, egui::Sense::drag());

    if response.dragged() {
        *pan_offset += response.drag_delta();
    }

    let scroll_delta = ui.input(|i| i.raw_scroll_delta.y);
    if scroll_delta != 0.0 {
        let factor = if scroll_delta > 0.0 { 1.1 } else { 0.9 };
        *zoom_scale = (*zoom_scale * factor).clamp(0.05, 15.0);
    }

    let center = response.rect.center() + *pan_offset;
    let target_size = available_size * (*zoom_scale);

    let image = egui::Image::new(texture)
        .fit_to_exact_size(target_size)
        .shrink_to_fit();

    let image_rect = egui::Rect::from_center_size(center, target_size);
    image.paint_at(ui, image_rect);

    painter.text(
        response.rect.right_bottom() - egui::vec2(16.0, 16.0),
        egui::Align2::RIGHT_BOTTOM,
        format!("{:.0}%  (Drag to pan · Scroll to zoom)", *zoom_scale * 100.0),
        egui::FontId::proportional(11.0),
        egui::Color32::from_rgb(100, 100, 105),
    );
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

fn prompt_file_dialog() -> Option<PathBuf> {
    // 1. Try kdialog (KDE Plasma)
    if let Ok(output) = std::process::Command::new("kdialog")
        .arg("--getopenfilename")
        .output()
    {
        if output.status.success() {
            let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !s.is_empty() {
                return Some(parse_input_path(&s));
            }
        }
    }
    // 2. Try zenity (GNOME / GTK)
    if let Ok(output) = std::process::Command::new("zenity")
        .arg("--file-selection")
        .output()
    {
        if output.status.success() {
            let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !s.is_empty() {
                return Some(parse_input_path(&s));
            }
        }
    }
    None
}

fn parse_input_path(raw: &str) -> PathBuf {
    let raw_trimmed = raw.trim();
    let path_str = if let Some(stripped) = raw_trimmed.strip_prefix("file://") {
        decode_url_percent(stripped)
    } else {
        raw_trimmed.to_string()
    };
    PathBuf::from(path_str)
}

fn decode_url_percent(s: &str) -> String {
    let mut result = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                result.push(byte);
                i += 3;
                continue;
            }
        }
        result.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&result).to_string()
}

fn detect_active_selection() -> Option<PathBuf> {
    // 1. Query KDE Dolphin over D-Bus (supports qdbus6, qdbus, qdbus-qt5, and busctl)
    let kde_query = r#"
        for cmd in qdbus6 qdbus qdbus-qt5; do
            if command -v "$cmd" >/dev/null 2>&1; then
                svc=$("$cmd" 2>/dev/null | grep -m1 org.kde.dolphin)
                if [ -n "$svc" ]; then
                    url=$("$cmd" "$svc" /dolphin/Dolphin_1 org.kde.dolphin.MainWindow.selectedUrls 2>/dev/null | head -n1)
                    if [ -n "$url" ]; then echo "$url"; exit 0; fi
                fi
            fi
        done
        if command -v busctl >/dev/null 2>&1; then
            svc=$(busctl --user list 2>/dev/null | grep -m1 org.kde.dolphin | awk '{print $1}')
            if [ -n "$svc" ]; then
                url=$(busctl --user call "$svc" /dolphin/Dolphin_1 org.kde.dolphin.MainWindow selectedUrls 2>/dev/null | grep -o 'file://[^"]*' | head -n1)
                if [ -n "$url" ]; then echo "$url"; exit 0; fi
            fi
        fi
    "#;

    if let Ok(output) = std::process::Command::new("sh").arg("-c").arg(kde_query).output() {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !text.is_empty() {
                let p = parse_input_path(&text);
                if p.exists() {
                    return Some(p);
                }
            }
        }
    }

    // 2. Query universal Wayland / X11 primary selection or clipboard
    let clip_query = r#"
        wl-paste --primary 2>/dev/null || \
        wl-paste 2>/dev/null || \
        xclip -o -selection primary 2>/dev/null || \
        xclip -o -selection clipboard 2>/dev/null || \
        xsel -o -p 2>/dev/null || \
        xsel -o -b 2>/dev/null
    "#;

    if let Ok(output) = std::process::Command::new("sh").arg("-c").arg(clip_query).output() {
        if output.status.success() {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    let p = parse_input_path(trimmed);
                    if p.exists() {
                        return Some(p);
                    }
                }
            }
        }
    }

    None
}

fn load_color_image_from_bytes(bytes: &[u8]) -> Option<egui::ColorImage> {
    let dyn_img = image::load_from_memory(bytes).ok()?;
    let size = [dyn_img.width() as _, dyn_img.height() as _];
    let image_buffer = dyn_img.to_rgba8();
    let pixels = image_buffer.into_raw();
    Some(egui::ColorImage::from_rgba_unmultiplied(size, &pixels))
}

fn pdf_get_total_pages(path: &Path) -> usize {
    if let Some(path_str) = path.to_str() {
        if let Ok(output) = std::process::Command::new("pdfinfo")
            .arg(path_str)
            .output()
        {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                for line in text.lines() {
                    if let Some(rest) = line.strip_prefix("Pages:") {
                        if let Ok(count) = rest.trim().parse::<usize>() {
                            return count;
                        }
                    }
                }
            }
        }
    }
    estimate_pdf_pages(path).unwrap_or(1)
}

fn render_pdf_page_to_image(path: &Path, page: usize) -> Option<egui::ColorImage> {
    let path_str = path.to_str()?;

    // 1. pdftoppm (poppler-utils) - fast and high quality
    if let Ok(output) = std::process::Command::new("pdftoppm")
        .args(["-png", "-r", "150", "-f", &page.to_string(), "-l", &page.to_string(), path_str, "-"])
        .output()
    {
        if output.status.success() && !output.stdout.is_empty() {
            if let Some(img) = load_color_image_from_bytes(&output.stdout) {
                return Some(img);
            }
        }
    }

    // 2. Ghostscript (gs) - default on almost all Linux distributions
    if let Ok(output) = std::process::Command::new("gs")
        .args([
            "-sDEVICE=png16m",
            &format!("-dFirstPage={page}"),
            &format!("-dLastPage={page}"),
            "-r150",
            "-sOutputFile=-",
            "-q",
            "-dNOPAUSE",
            "-dBATCH",
            path_str,
        ])
        .output()
    {
        if output.status.success() && !output.stdout.is_empty() {
            if let Some(img) = load_color_image_from_bytes(&output.stdout) {
                return Some(img);
            }
        }
    }

    // 3. mutool (MuPDF)
    if let Ok(output) = std::process::Command::new("mutool")
        .args(["draw", "-o", "-", "-r", "150", path_str, &page.to_string()])
        .output()
    {
        if output.status.success() && !output.stdout.is_empty() {
            if let Some(img) = load_color_image_from_bytes(&output.stdout) {
                return Some(img);
            }
        }
    }

    // 4. Fallback for page 1: FreeDesktop / KDE thumbnail cache
    if page == 1 {
        if let Some(img) = get_xdg_thumbnail(path) {
            return Some(img);
        }
    }

    None
}

fn extract_video_frame_to_image(path: &Path) -> Option<egui::ColorImage> {
    let path_str = path.to_str()?;

    // 1. ffmpeg
    if let Ok(output) = std::process::Command::new("ffmpeg")
        .args([
            "-ss", "00:00:01",
            "-i", path_str,
            "-frames:v", "1",
            "-f", "image2pipe",
            "-vcodec", "png",
            "-"
        ])
        .output()
    {
        if output.status.success() && !output.stdout.is_empty() {
            if let Some(img) = load_color_image_from_bytes(&output.stdout) {
                return Some(img);
            }
        }
    }

    // 2. ffmpegthumbnailer
    if let Ok(output) = std::process::Command::new("ffmpegthumbnailer")
        .args(["-i", path_str, "-o", "-", "-s", "1280", "-c", "png"])
        .output()
    {
        if output.status.success() && !output.stdout.is_empty() {
            if let Some(img) = load_color_image_from_bytes(&output.stdout) {
                return Some(img);
            }
        }
    }

    // 3. mpv video frame capture
    if let Ok(output) = std::process::Command::new("mpv")
        .args([
            "--no-audio",
            "--start=1",
            "--frames=1",
            "--vo=image",
            "--vo-image-format=png",
            "--vo-image-outdir=/dev/stdout",
            path_str,
        ])
        .output()
    {
        if output.status.success() && !output.stdout.is_empty() {
            if let Some(img) = load_color_image_from_bytes(&output.stdout) {
                return Some(img);
            }
        }
    }

    // 4. FreeDesktop / KDE Dolphin thumbnail cache
    if let Some(img) = get_xdg_thumbnail(path) {
        return Some(img);
    }

    None
}

fn extract_audio_art_to_image(path: &Path) -> Option<egui::ColorImage> {
    let path_str = path.to_str()?;
    if let Ok(output) = std::process::Command::new("ffmpeg")
        .args(["-i", path_str, "-an", "-vcodec", "png", "-f", "image2pipe", "-"])
        .output()
    {
        if output.status.success() && !output.stdout.is_empty() {
            if let Some(img) = load_color_image_from_bytes(&output.stdout) {
                return Some(img);
            }
        }
    }

    // Fallback: FreeDesktop / KDE Dolphin thumbnail cache
    if let Some(img) = get_xdg_thumbnail(path) {
        return Some(img);
    }

    None
}

fn get_xdg_thumbnail(path: &Path) -> Option<egui::ColorImage> {
    let abs_path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let uri = format!("file://{}", abs_path.to_string_lossy());

    // Compute MD5 hash of file:// URI via md5sum or md5
    let mut cmd = std::process::Command::new("md5sum");
    let hash = if let Ok(mut child) = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
    {
        use std::io::Write;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(uri.as_bytes());
        }
        child.wait_with_output().ok().and_then(|out| {
            String::from_utf8_lossy(&out.stdout)
                .split_whitespace()
                .next()
                .map(|s| s.to_string())
        })
    } else if let Ok(out) = std::process::Command::new("md5").arg("-s").arg(&uri).output() {
        let s = String::from_utf8_lossy(&out.stdout);
        s.split_whitespace().last().map(|s| s.to_string())
    } else {
        None
    }?;

    let home = std::env::var("HOME").ok()?;
    let dirs = ["xx-large", "x-large", "large", "normal"];
    for d in &dirs {
        let candidate = format!("{home}/.cache/thumbnails/{d}/{hash}.png");
        if let Ok(bytes) = fs::read(&candidate) {
            if let Some(img) = load_color_image_from_bytes(&bytes) {
                return Some(img);
            }
        }
    }

    None
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

    #[test]
    fn test_parse_input_path() {
        let path1 = parse_input_path("file:///home/karthik/My%20Documents/test.txt");
        assert_eq!(path1, PathBuf::from("/home/karthik/My Documents/test.txt"));

        let path2 = parse_input_path("/home/karthik/plain_path.png");
        assert_eq!(path2, PathBuf::from("/home/karthik/plain_path.png"));
    }

    #[test]
    fn test_format_time() {
        assert_eq!(format_time(0.0), "00:00");
        assert_eq!(format_time(45.0), "00:45");
        assert_eq!(format_time(75.0), "01:15");
        assert_eq!(format_time(3665.0), "61:05");
    }

    #[test]
    fn test_calculate_target_dims() {
        let (w, h) = calculate_target_dims(1920, 1080, 854, 480);
        assert_eq!(w, 854);
        assert_eq!(h, 480);

        let (w2, h2) = calculate_target_dims(1080, 1920, 854, 480);
        assert_eq!(h2, 480);
        assert_eq!(w2 % 2, 0);
    }
}
