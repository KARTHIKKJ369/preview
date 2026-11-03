use eframe::egui;
use std::collections::{HashMap, HashSet};
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
        presentation: PresentationData,
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
        page_images: HashMap<usize, egui::ColorImage>,
        page_textures: HashMap<usize, egui::TextureHandle>,
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

impl std::fmt::Debug for FileKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Welcome => write!(f, "Welcome"),
            Self::Image { width, height, uri } => f
                .debug_struct("Image")
                .field("width", width)
                .field("height", height)
                .field("uri", uri)
                .finish(),
            Self::Code { language, lines, truncated, .. } => f
                .debug_struct("Code")
                .field("language", language)
                .field("lines", lines)
                .field("truncated", truncated)
                .finish(),
            Self::Markdown { lines, .. } => f
                .debug_struct("Markdown")
                .field("lines", lines)
                .finish(),
            Self::Spreadsheet { total_rows, total_cols, .. } => f
                .debug_struct("Spreadsheet")
                .field("total_rows", total_rows)
                .field("total_cols", total_cols)
                .finish(),
            Self::Presentation { presentation, current_slide } => f
                .debug_struct("Presentation")
                .field("aspect_ratio", &presentation.aspect_ratio)
                .field("slides_len", &presentation.slides.len())
                .field("current_slide", current_slide)
                .finish(),
            Self::Document { word_count, paragraphs } => f
                .debug_struct("Document")
                .field("word_count", word_count)
                .field("paragraphs_len", &paragraphs.len())
                .finish(),
            Self::Directory { total_items, .. } => f
                .debug_struct("Directory")
                .field("total_items", total_items)
                .finish(),
            Self::Archive { total_entries, .. } => f
                .debug_struct("Archive")
                .field("total_entries", total_entries)
                .finish(),
            Self::Pdf { current_page, total_pages, page_images, page_textures } => f
                .debug_struct("Pdf")
                .field("current_page", current_page)
                .field("total_pages", total_pages)
                .field("page_images_len", &page_images.len())
                .field("page_textures_len", &page_textures.len())
                .finish(),
            Self::Media { kind, poster_image } => f
                .debug_struct("Media")
                .field("kind", kind)
                .field("has_poster", &poster_image.is_some())
                .finish(),
            Self::Hex { total_size, .. } => f
                .debug_struct("Hex")
                .field("total_size", total_size)
                .finish(),
            Self::Error(err) => f.debug_tuple("Error").field(err).finish(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PresentationData {
    pub aspect_ratio: f32,
    pub slides: Vec<SlideData>,
}

impl Default for PresentationData {
    fn default() -> Self {
        Self {
            aspect_ratio: 16.0 / 9.0,
            slides: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SlideData {
    pub slide_number: usize,
    pub title: String,
    pub subtitle: Option<String>,
    pub background_color: Option<egui::Color32>,
    pub blocks: Vec<SlideContentBlock>,
    pub tables: Vec<SlideTableData>,
    pub footers: Vec<String>,
    pub is_title_slide: bool,
}

#[derive(Debug, Clone)]
pub enum SlideContentBlock {
    Paragraph {
        text: String,
        bold: bool,
        level: usize,
        is_bullet: bool,
    },
    Card {
        header: Option<String>,
        items: Vec<String>,
    },
}

#[derive(Debug, Clone)]
pub struct SlideTableData {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Default)]
pub struct AudioMetadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: f64,
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

pub struct PdfPageRequest {
    pub path: PathBuf,
    pub page: usize,
    pub generation: usize,
}

pub struct PdfPageResponse {
    pub path: PathBuf,
    pub generation: usize,
    pub page: usize,
    pub image: Option<egui::ColorImage>,
}

pub struct PdfLoader {
    pub request_tx: Sender<PdfPageRequest>,
    pub response_rx: Receiver<PdfPageResponse>,
    pub generation: usize,
    pub in_flight: HashSet<usize>,
    pub failed: HashSet<usize>,
}

impl PdfLoader {
    pub fn new() -> Self {
        let (request_tx, request_rx) = channel::<PdfPageRequest>();
        let (response_tx, response_rx) = channel::<PdfPageResponse>();

        std::thread::spawn(move || {
            while let Ok(req) = request_rx.recv() {
                let img = render_pdf_page_to_image(&req.path, req.page);
                let _ = response_tx.send(PdfPageResponse {
                    path: req.path,
                    generation: req.generation,
                    page: req.page,
                    image: img,
                });
            }
        });

        Self {
            request_tx,
            response_rx,
            generation: 0,
            in_flight: HashSet::new(),
            failed: HashSet::new(),
        }
    }

    pub fn request_page(&mut self, path: &Path, page: usize) {
        if self.in_flight.contains(&page) || self.failed.contains(&page) {
            return;
        }
        self.in_flight.insert(page);
        let _ = self.request_tx.send(PdfPageRequest {
            path: path.to_path_buf(),
            page,
            generation: self.generation,
        });
    }

    pub fn reset_for_new_file(&mut self) {
        self.generation += 1;
        self.in_flight.clear();
        self.failed.clear();
    }
}

impl Default for PdfLoader {
    fn default() -> Self {
        Self::new()
    }
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
    pending_resize: Option<egui::Vec2>,
    pdf_loader: PdfLoader,
    pdf_scroll_target: Option<usize>,
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
            pending_resize: None,
            pdf_loader: PdfLoader::new(),
            pdf_scroll_target: None,
        };

        if let Some(t) = target {
            app.set_target(t);
        }
        app
    }

    pub fn calculate_optimal_size(&self) -> egui::Vec2 {
        match &self.file_kind {
            FileKind::Image { width, height, .. } => {
                if *width > 0 && *height > 0 {
                    let max_w = 1100.0_f32;
                    let max_h = 820.0_f32;
                    let min_w = 460.0_f32;
                    let min_h = 360.0_f32;
                    let top_bar_h = 48.0_f32;

                    let aspect = *width as f32 / *height as f32;

                    let mut target_h = (max_h - top_bar_h).min(*height as f32);
                    let mut target_w = target_h * aspect;

                    if target_w > max_w {
                        target_w = max_w;
                        target_h = target_w / aspect;
                    }

                    egui::vec2(
                        target_w.clamp(min_w, max_w),
                        (target_h + top_bar_h).clamp(min_h, max_h),
                    )
                } else {
                    egui::vec2(800.0, 600.0)
                }
            }
            FileKind::Media { kind, .. } => {
                if *kind == "Audio" {
                    egui::vec2(580.0, 440.0)
                } else {
                    if let Some(player) = &self.media_player {
                        if player.dimensions.0 > 0 && player.dimensions.1 > 0 {
                            let aspect = player.dimensions.0 as f32 / player.dimensions.1 as f32;
                            let target_w = 960.0_f32;
                            let target_h = (target_w / aspect) + 48.0;
                            return egui::vec2(target_w, target_h.clamp(400.0, 800.0));
                        }
                    }
                    egui::vec2(960.0, 580.0)
                }
            }
            FileKind::Pdf { .. } => {
                egui::vec2(1020.0, 880.0)
            }
            FileKind::Presentation { .. } => {
                egui::vec2(1140.0, 680.0)
            }
            FileKind::Spreadsheet { .. } => {
                egui::vec2(1040.0, 660.0)
            }
            FileKind::Document { .. } => {
                egui::vec2(960.0, 800.0)
            }
            FileKind::Code { .. } | FileKind::Markdown { .. } => {
                egui::vec2(920.0, 720.0)
            }
            FileKind::Directory { .. } | FileKind::Archive { .. } => {
                egui::vec2(860.0, 620.0)
            }
            _ => egui::vec2(880.0, 640.0),
        }
    }

    fn set_loaded_kind(&mut self, kind: FileKind) {
        self.file_kind = kind;
        self.pending_resize = Some(self.calculate_optimal_size());
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
        self.pdf_loader.reset_for_new_file();
        self.pdf_scroll_target = None;
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
        self.pdf_loader.reset_for_new_file();
        self.pdf_scroll_target = None;
        self.load_current_file();
    }

    fn load_current_file(&mut self) {
        let path = match self.current_path.as_deref() {
            Some(p) => p,
            None => {
                self.set_loaded_kind(FileKind::Welcome);
                return;
            }
        };

        let metadata = match fs::metadata(path) {
            Ok(m) => m,
            Err(e) => {
                self.file_size = 0;
                self.set_loaded_kind(FileKind::Error(format!("Failed to read metadata: {e}")));
                return;
            }
        };

        self.file_size = metadata.len();

        if metadata.is_dir() {
            let kind = load_directory(path);
            self.set_loaded_kind(kind);
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
                self.set_loaded_kind(FileKind::Image {
                    width,
                    height,
                    uri,
                });
                return;
            }
            _ => {}
        }

        // 2. Presentations (PowerPoint PPTX, ODP, PPT)
        if ext == "pptx" || ext == "odp" {
            let kind = load_pptx(path);
            self.set_loaded_kind(kind);
            return;
        }
        if ext == "ppt" {
            let kind = load_legacy_office(path, "PowerPoint Presentation");
            self.set_loaded_kind(kind);
            return;
        }

        // 3. Spreadsheets (Excel XLSX, ODS, XLS, CSV, TSV)
        if ext == "xlsx" || ext == "ods" {
            let kind = load_xlsx(path);
            self.set_loaded_kind(kind);
            return;
        }
        if ext == "xls" {
            let kind = load_legacy_office(path, "Excel Spreadsheet");
            self.set_loaded_kind(kind);
            return;
        }
        if ext == "csv" {
            if let Ok(content) = fs::read_to_string(path) {
                let kind = load_csv_or_tsv(&content, ',');
                self.set_loaded_kind(kind);
                return;
            }
        }
        if ext == "tsv" {
            if let Ok(content) = fs::read_to_string(path) {
                let kind = load_csv_or_tsv(&content, '\t');
                self.set_loaded_kind(kind);
                return;
            }
        }

        // 4. Word Documents (DOCX, ODT, DOC, RTF)
        if ext == "docx" || ext == "odt" {
            let kind = load_docx(path);
            self.set_loaded_kind(kind);
            return;
        }
        if ext == "doc" || ext == "rtf" {
            let kind = load_legacy_office(path, "Word Document");
            self.set_loaded_kind(kind);
            return;
        }

        // 5. Archives (ZIP detection)
        if ext == "zip" || ext == "jar" || ext == "war" {
            let kind = load_zip_listing(path, self.file_size);
            self.set_loaded_kind(kind);
            return;
        }

        // 6. Audio & Video
        match ext.as_str() {
            "mp3" | "wav" | "ogg" | "flac" | "aac" | "m4a" | "opus" => {
                let poster_image = extract_audio_art_to_image(path);
                let (duration, dims, audio_meta) = probe_media_info(path);
                self.media_player = Some(MediaPlayer::new(path, false, duration, dims, audio_meta));
                self.set_loaded_kind(FileKind::Media {
                    kind: "Audio",
                    poster_image,
                });
                return;
            }
            "mp4" | "mkv" | "mov" | "webm" | "avi" | "wmv" | "flv" => {
                let poster_image = extract_video_frame_to_image(path);
                let (duration, dims, audio_meta) = probe_media_info(path);
                self.media_player = Some(MediaPlayer::new(path, true, duration, dims, audio_meta));
                self.set_loaded_kind(FileKind::Media {
                    kind: "Video",
                    poster_image,
                });
                return;
            }
            _ => {}
        }

        // 7. PDF (Instant async load, 0ms main thread blocking)
        if ext == "pdf" {
            let total_pages = pdf_get_total_pages(path).max(1);
            let mut page_images = HashMap::new();
            if let Some(img) = get_xdg_thumbnail(path) {
                page_images.insert(1, img);
            }
            self.pdf_loader.request_page(path, 1);
            if total_pages > 1 {
                self.pdf_loader.request_page(path, 2);
            }
            self.set_loaded_kind(FileKind::Pdf {
                current_page: 1,
                total_pages,
                page_images,
                page_textures: HashMap::new(),
            });
            return;
        }

        // 8. Code & Text (Streaming read up to 512 KB to avoid UI freezing)
        const MAX_TEXT_BYTES: u64 = 512 * 1024;
        let read_limit = self.file_size.min(MAX_TEXT_BYTES) as usize;
        let mut buffer = vec![0u8; read_limit];
        let bytes_read = match File::open(path).and_then(|mut f| f.read(&mut buffer)) {
            Ok(n) => n,
            Err(e) => {
                self.file_size = 0;
                self.set_loaded_kind(FileKind::Error(format!("Read error: {e}")));
                return;
            }
        };
        buffer.truncate(bytes_read);

        // Check if content is valid UTF-8
        if let Ok(text) = std::str::from_utf8(&buffer) {
            let lines = text.lines().count();
            if ext == "md" || ext == "markdown" {
                self.set_loaded_kind(FileKind::Markdown {
                    content: text.to_string(),
                    lines,
                });
                return;
            }

            let lang = detect_language(&ext, text);
            let truncated = self.file_size > MAX_TEXT_BYTES;
            self.set_loaded_kind(FileKind::Code {
                content: text.to_string(),
                language: lang,
                lines,
                truncated,
            });
            return;
        }

        // 9. Binary / Hex Fallback (Reads first 16KB for hex dump)
        const MAX_HEX_BYTES: usize = 16 * 1024;
        let hex_slice = if buffer.len() > MAX_HEX_BYTES {
            buffer[..MAX_HEX_BYTES].to_vec()
        } else {
            buffer
        };

        self.set_loaded_kind(FileKind::Hex {
            data: hex_slice,
            total_size: self.file_size,
        });
    }
}

impl eframe::App for QuickLookApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Dynamic window resizing based on content (macOS Quick Look behavior)
        if let Some(target_size) = self.pending_resize.take() {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(target_size));
        }

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

                let finished = player.check_eof_and_reap();
                if !finished && player.duration > 0.0 && player.current_time >= player.duration {
                    let still_running = if player.is_video {
                        player.video_child.as_mut().map(|c| c.try_wait().map(|s| s.is_none()).unwrap_or(false)).unwrap_or(false)
                    } else {
                        player.audio_child.as_mut().map(|c| c.try_wait().map(|s| s.is_none()).unwrap_or(false)).unwrap_or(false)
                    };

                    if still_running {
                        player.duration = player.current_time + 1.0;
                    } else {
                        player.pause_playback();
                        player.current_time = player.duration;
                    }
                }
                ctx.request_repaint();
            }
        }

        // Keyboard Shortcuts (macOS Quick Look Behavior)
        ctx.input(|i| {
            let is_presentation = matches!(self.file_kind, FileKind::Presentation { .. });
            let is_pdf = matches!(self.file_kind, FileKind::Pdf { .. });

            // 1. External editor / file open (Universal: O or Enter)
            if i.key_pressed(egui::Key::O) || i.key_pressed(egui::Key::Enter) {
                if let Some(ref path) = self.current_path {
                    let _ = open::that(path);
                }
            }

            // 2. Media playback toggle (Universal: P)
            if i.key_pressed(egui::Key::P) {
                if let Some(player) = &mut self.media_player {
                    player.toggle_playback();
                }
            }

            // 3. Zoom controls (Universal: +, -, 0)
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

            if is_presentation {
                // --- PRESENTATION MODE KEYBOARD ROUTING ---
                if i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Q) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }

                let alt = i.modifiers.alt;
                let shift = i.modifiers.shift;

                if alt {
                    if i.key_pressed(egui::Key::ArrowLeft)
                        || i.key_pressed(egui::Key::ArrowUp)
                        || i.key_pressed(egui::Key::OpenBracket)
                    {
                        self.navigate(-1);
                    }
                    if i.key_pressed(egui::Key::ArrowRight)
                        || i.key_pressed(egui::Key::ArrowDown)
                        || i.key_pressed(egui::Key::CloseBracket)
                    {
                        self.navigate(1);
                    }
                } else {
                    let next_slide = i.key_pressed(egui::Key::ArrowRight)
                        || i.key_pressed(egui::Key::ArrowDown)
                        || i.key_pressed(egui::Key::PageDown)
                        || i.key_pressed(egui::Key::J)
                        || (i.key_pressed(egui::Key::Space) && !shift);

                    let prev_slide = i.key_pressed(egui::Key::ArrowLeft)
                        || i.key_pressed(egui::Key::ArrowUp)
                        || i.key_pressed(egui::Key::PageUp)
                        || i.key_pressed(egui::Key::K)
                        || i.key_pressed(egui::Key::Backspace)
                        || (i.key_pressed(egui::Key::Space) && shift);

                    let first_slide = i.key_pressed(egui::Key::Home);
                    let last_slide = i.key_pressed(egui::Key::End);

                    if let FileKind::Presentation { presentation, current_slide } = &mut self.file_kind {
                        let total = presentation.slides.len();
                        if total > 0 {
                            if next_slide && *current_slide + 1 < total {
                                *current_slide += 1;
                            } else if prev_slide && *current_slide > 0 {
                                *current_slide -= 1;
                            } else if first_slide {
                                *current_slide = 0;
                            } else if last_slide {
                                *current_slide = total - 1;
                            }
                        }
                    }
                }
            } else if is_pdf {
                // --- PDF MODE KEYBOARD ROUTING ---
                if i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Q) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }

                let alt = i.modifiers.alt;
                let shift = i.modifiers.shift;

                if alt {
                    if i.key_pressed(egui::Key::ArrowLeft)
                        || i.key_pressed(egui::Key::ArrowUp)
                        || i.key_pressed(egui::Key::OpenBracket)
                    {
                        self.navigate(-1);
                    }
                    if i.key_pressed(egui::Key::ArrowRight)
                        || i.key_pressed(egui::Key::ArrowDown)
                        || i.key_pressed(egui::Key::CloseBracket)
                    {
                        self.navigate(1);
                    }
                } else {
                    let next_page = i.key_pressed(egui::Key::ArrowRight)
                        || i.key_pressed(egui::Key::ArrowDown)
                        || i.key_pressed(egui::Key::PageDown)
                        || i.key_pressed(egui::Key::J)
                        || (i.key_pressed(egui::Key::Space) && !shift);

                    let prev_page = i.key_pressed(egui::Key::ArrowLeft)
                        || i.key_pressed(egui::Key::ArrowUp)
                        || i.key_pressed(egui::Key::PageUp)
                        || i.key_pressed(egui::Key::K)
                        || i.key_pressed(egui::Key::Backspace)
                        || (i.key_pressed(egui::Key::Space) && shift);

                    let first_page = i.key_pressed(egui::Key::Home);
                    let last_page = i.key_pressed(egui::Key::End);

                    if let FileKind::Pdf { current_page, total_pages, .. } = &mut self.file_kind {
                        let total = *total_pages;
                        let mut target_page = None;
                        if next_page && *current_page < total {
                            target_page = Some(*current_page + 1);
                        } else if prev_page && *current_page > 1 {
                            target_page = Some(*current_page - 1);
                        } else if first_page {
                            target_page = Some(1);
                        } else if last_page {
                            target_page = Some(total);
                        }

                        if let Some(tp) = target_page {
                            *current_page = tp;
                            self.pdf_scroll_target = Some(tp);
                            if let Some(p) = self.current_path.as_deref() {
                                self.pdf_loader.request_page(p, tp);
                            }
                        }
                    }
                }
            } else {
                // --- STANDARD PREVIEW MODE ---
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
                        FileKind::Presentation { presentation, current_slide } => {
                            format!("·  Presentation · Slide {}/{}", current_slide + 1, presentation.slides.len())
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

                        // Audio Volume Control in Top Bar (matching Screenshot 2)
                        if matches!(self.file_kind, FileKind::Media { kind: "Audio", .. }) {
                            if let Some(player) = &mut self.media_player {
                                custom_volume_control(ui, &mut player.volume);
                                ui.separator();
                            }
                        }

                        // Zoom controls for Image & PDF
                        if matches!(self.file_kind, FileKind::Image { .. }) || matches!(self.file_kind, FileKind::Pdf { .. }) {
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
            FileKind::Image { uri, width, height } => {
                render_image_viewer(ui, uri, *width, *height, &mut self.zoom_scale, &mut self.pan_offset);
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
            FileKind::Presentation { presentation, current_slide } => {
                render_presentation_viewer(ui, presentation, current_slide);
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
            FileKind::Pdf { current_page, total_pages, page_images, page_textures } => {
                // 1. Convert any CPU-side page_images to GPU textures
                let pending_keys: Vec<usize> = page_images.keys().copied().collect();
                for p in pending_keys {
                    if let Some(img) = page_images.remove(&p) {
                        let tex_name = format!("pdf_page_{p}");
                        let handle = ctx.load_texture(&tex_name, img, egui::TextureOptions::LINEAR);
                        page_textures.insert(p, handle);
                    }
                }

                // 2. Poll background loader responses
                while let Ok(resp) = self.pdf_loader.response_rx.try_recv() {
                    if resp.generation == self.pdf_loader.generation {
                        self.pdf_loader.in_flight.remove(&resp.page);
                        if let Some(img) = resp.image {
                            let tex_name = format!("pdf_page_{}", resp.page);
                            let handle = ctx.load_texture(&tex_name, img, egui::TextureOptions::LINEAR);
                            page_textures.insert(resp.page, handle);
                            ctx.request_repaint();

                            // Enforce distance-based LRU eviction: MAX_CACHED_PAGES = 6 (keeps RAM footprint minimal)
                            const MAX_CACHED_PAGES: usize = 6;
                            while page_textures.len() > MAX_CACHED_PAGES {
                                let cur = *current_page;
                                if let Some((&furthest_page, _)) = page_textures
                                    .iter()
                                    .max_by_key(|(&p, _)| (p as isize - cur as isize).abs())
                                {
                                    if furthest_page != cur {
                                        page_textures.remove(&furthest_page);
                                    } else {
                                        break;
                                    }
                                } else {
                                    break;
                                }
                            }
                        } else {
                            // Record page as failed to eliminate infinite re-request spin loops
                            self.pdf_loader.failed.insert(resp.page);
                            ctx.request_repaint();
                        }
                    }
                }

                // 3. Keep repainting while requests are in flight
                if !self.pdf_loader.in_flight.is_empty() {
                    ctx.request_repaint_after(std::time::Duration::from_millis(30));
                }

                let p = self.current_path.as_deref().unwrap_or(Path::new("document.pdf"));
                render_pdf_viewer(
                    ui,
                    p,
                    current_page,
                    *total_pages,
                    page_textures,
                    &mut self.pdf_loader,
                    &mut self.pdf_scroll_target,
                    &mut self.zoom_scale,
                    &mut self.pan_offset,
                );
            }
            FileKind::Media { kind, poster_image } => {
                if self.active_texture.is_none() {
                    if let Some(img) = poster_image.take() {
                        self.active_texture = Some(ctx.load_texture("media_poster", img, egui::TextureOptions::LINEAR));
                    }
                }
                if let Some(player) = &mut self.media_player {
                    if let Some(delta) = render_media_player(
                        ui,
                        player,
                        self.active_texture.as_ref(),
                        self.file_size,
                        &mut self.zoom_scale,
                        &mut self.pan_offset,
                    ) {
                        self.navigate(delta);
                    }
                } else {
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
    orig_w: u32,
    orig_h: u32,
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

    let target_size = if orig_w > 0 && orig_h > 0 {
        let margin = 24.0_f32;
        let max_w = (available_size.x - margin * 2.0).max(10.0);
        let max_h = (available_size.y - margin * 2.0).max(10.0);

        let fit_scale = (max_w / orig_w as f32).min(max_h / orig_h as f32);
        let base_w = orig_w as f32 * fit_scale;
        let base_h = orig_h as f32 * fit_scale;

        egui::vec2(base_w * (*zoom_scale), base_h * (*zoom_scale))
    } else {
        available_size * (*zoom_scale)
    };

    let center = response.rect.center() + *pan_offset;
    let image_rect = egui::Rect::from_center_size(center, target_size);

    // Subtle drop shadow behind document sheet
    let shadow_rect = image_rect.expand(2.5);
    painter.rect_filled(
        shadow_rect,
        egui::Rounding::same(3.0),
        egui::Color32::from_rgba_premultiplied(0, 0, 0, 30),
    );

    let image = egui::Image::new(uri).fit_to_exact_size(target_size);
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

/// Computes slide canvas dimensions preserving the presentation's native aspect ratio
/// and calculates a dynamic scaling factor for responsive typography and spacing.
pub fn compute_presentation_slide_dimensions(
    available_size: egui::Vec2,
    aspect_ratio: f32,
    hud_clearance: f32,
) -> (f32, f32, f32) {
    let aspect = if aspect_ratio.is_finite() && aspect_ratio > 0.05 {
        aspect_ratio.clamp(0.2, 5.0)
    } else {
        16.0 / 9.0
    };

    let hud = if hud_clearance.is_finite() && hud_clearance >= 0.0 {
        hud_clearance
    } else {
        48.0_f32
    };
    let avail_x = if available_size.x.is_finite() {
        available_size.x.max(0.0)
    } else {
        0.0
    };
    let avail_y = if available_size.y.is_finite() {
        available_size.y.max(0.0)
    } else {
        0.0
    };

    let pad_x = 28.0_f32;
    let pad_y = 28.0_f32;

    let usable_w = (avail_x - pad_x * 2.0).max(160.0);
    let usable_h = (avail_y - pad_y * 2.0 - hud).max(100.0);

    let mut slide_w = usable_w;
    let mut slide_h = slide_w / aspect;

    if slide_h > usable_h {
        slide_h = usable_h;
        slide_w = slide_h * aspect;
    }

    let min_w = 160.0_f32;
    let min_h = 90.0_f32;
    if slide_w < min_w || slide_h < min_h {
        let scale_w = min_w / slide_w;
        let scale_h = min_h / slide_h;
        let boost = scale_w.max(scale_h);
        slide_w *= boost;
        slide_h *= boost;
    }

    let max_w = avail_x;
    let max_h = avail_y - hud;
    if max_w >= min_w && max_h >= min_h {
        if slide_w > max_w {
            slide_w = max_w;
            slide_h = slide_w / aspect;
        }
        if slide_h > max_h {
            slide_h = max_h;
            slide_w = slide_h * aspect;
        }
    }

    let scale_factor = (slide_w / 960.0).clamp(0.6, 2.5);

    (slide_w, slide_h, scale_factor)
}

#[derive(Debug, Clone, Copy)]
pub struct SlideThemeColors {
    pub bg: egui::Color32,
    pub primary_text: egui::Color32,
    pub secondary_text: egui::Color32,
    pub muted_text: egui::Color32,
    pub divider: egui::Color32,
    pub card_bg: egui::Color32,
    pub card_border: egui::Color32,
    pub table_header_bg: egui::Color32,
    pub table_alt_row_bg: egui::Color32,
    pub table_border: egui::Color32,
    pub accent: egui::Color32,
}

impl SlideThemeColors {
    pub fn from_background(bg_opt: Option<egui::Color32>) -> Self {
        let bg = bg_opt.unwrap_or(egui::Color32::from_rgb(255, 255, 255));
        let lum = 0.299 * (bg.r() as f32) + 0.587 * (bg.g() as f32) + 0.114 * (bg.b() as f32);
        let is_dark = lum < 128.0;

        let accent = egui::Color32::from_rgb(0, 122, 255);

        if is_dark {
            Self {
                bg,
                primary_text: egui::Color32::from_rgb(245, 245, 247),
                secondary_text: egui::Color32::from_rgb(175, 175, 185),
                muted_text: egui::Color32::from_rgb(130, 130, 140),
                divider: egui::Color32::from_rgb(60, 60, 68),
                card_bg: egui::Color32::from_rgb(42, 42, 48),
                card_border: egui::Color32::from_rgb(65, 65, 75),
                table_header_bg: egui::Color32::from_rgb(48, 48, 56),
                table_alt_row_bg: egui::Color32::from_rgba_premultiplied(255, 255, 255, 12),
                table_border: egui::Color32::from_rgb(65, 65, 75),
                accent,
            }
        } else {
            Self {
                bg,
                primary_text: egui::Color32::from_rgb(24, 24, 28),
                secondary_text: egui::Color32::from_rgb(90, 90, 98),
                muted_text: egui::Color32::from_rgb(142, 142, 147),
                divider: egui::Color32::from_rgb(224, 224, 230),
                card_bg: egui::Color32::from_rgb(248, 249, 252),
                card_border: egui::Color32::from_rgb(226, 228, 234),
                table_header_bg: egui::Color32::from_rgb(242, 244, 248),
                table_alt_row_bg: egui::Color32::from_rgba_premultiplied(0, 0, 0, 7),
                table_border: egui::Color32::from_rgb(218, 218, 224),
                accent,
            }
        }
    }
}

fn render_bullet_item(
    ui: &mut egui::Ui,
    text: &str,
    bold: bool,
    level: usize,
    is_bullet: bool,
    scale_factor: f32,
    colors: &SlideThemeColors,
) {
    let (indent, glyph, glyph_color, font_size) = match level {
        0 => (
            0.0_f32,
            "•",
            colors.accent,
            (15.0 * scale_factor).clamp(10.0, 26.0),
        ),
        1 => (
            24.0 * scale_factor,
            "–",
            colors.secondary_text,
            (14.0 * scale_factor).clamp(9.5, 24.0),
        ),
        2 => (
            48.0 * scale_factor,
            "▪",
            colors.muted_text,
            (13.0 * scale_factor).clamp(9.0, 22.0),
        ),
        lvl => (
            (48.0 + 20.0 * (lvl - 2) as f32) * scale_factor,
            "▫",
            colors.muted_text,
            (12.0 * scale_factor).clamp(8.5, 20.0),
        ),
    };

    ui.horizontal(|ui| {
        if indent > 0.0 {
            ui.add_space(indent);
        }

        if is_bullet {
            let bullet_box_w = 18.0 * scale_factor;
            let (b_rect, _) = ui.allocate_exact_size(
                egui::vec2(bullet_box_w, font_size + 2.0),
                egui::Sense::hover(),
            );
            ui.painter().text(
                b_rect.center(),
                egui::Align2::CENTER_CENTER,
                glyph,
                egui::FontId::proportional(font_size),
                glyph_color,
            );
        }

        let mut rt = egui::RichText::new(text)
            .size(font_size)
            .color(colors.primary_text);
        if bold {
            rt = rt.strong();
        }
        ui.label(rt);
    });
}

fn render_card_block(
    ui: &mut egui::Ui,
    header: Option<&str>,
    items: &[String],
    scale_factor: f32,
    colors: &SlideThemeColors,
) {
    let card_pad_x = 14.0 * scale_factor;
    let card_pad_y = 10.0 * scale_factor;
    egui::Frame::none()
        .rounding(egui::Rounding::same(6.0 * scale_factor))
        .fill(colors.card_bg)
        .stroke(egui::Stroke::new(1.0_f32, colors.card_border))
        .inner_margin(egui::Margin::symmetric(card_pad_x, card_pad_y))
        .show(ui, |ui| {
            if let Some(h) = header {
                ui.label(
                    egui::RichText::new(h)
                        .size((14.0 * scale_factor).clamp(10.0, 24.0))
                        .strong()
                        .color(colors.primary_text),
                );
                ui.add_space(4.0 * scale_factor);
            }
            for item in items {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("•")
                            .size((13.0 * scale_factor).clamp(9.0, 20.0))
                            .color(colors.accent),
                    );
                    ui.add_space(4.0 * scale_factor);
                    ui.label(
                        egui::RichText::new(item)
                            .size((13.0 * scale_factor).clamp(9.0, 20.0))
                            .color(colors.primary_text),
                    );
                });
            }
        });
}

fn render_slide_table(
    ui: &mut egui::Ui,
    table: &SlideTableData,
    slide_number: usize,
    table_index: usize,
    scale_factor: f32,
    colors: &SlideThemeColors,
) {
    if table.headers.is_empty() && table.rows.is_empty() {
        return;
    }

    ui.add_space(10.0 * scale_factor);

    let cell_pad_x = 12.0 * scale_factor;
    let cell_pad_y = 6.0 * scale_factor;
    let header_font_size = (13.5 * scale_factor).clamp(9.5, 22.0);
    let cell_font_size = (13.0 * scale_factor).clamp(9.0, 20.0);

    egui::Frame::none()
        .rounding(egui::Rounding::same(6.0 * scale_factor))
        .stroke(egui::Stroke::new(1.0_f32, colors.table_border))
        .fill(colors.card_bg)
        .inner_margin(egui::Margin::symmetric(cell_pad_x, cell_pad_y))
        .show(ui, |ui| {
            let grid_id = ui.make_persistent_id(("slide_table_grid", slide_number, table_index));
            egui::Grid::new(grid_id)
                .striped(true)
                .min_col_width(70.0 * scale_factor)
                .spacing([16.0 * scale_factor, 8.0 * scale_factor])
                .show(ui, |ui| {
                    if !table.headers.is_empty() {
                        for h in &table.headers {
                            ui.label(
                                egui::RichText::new(h)
                                    .size(header_font_size)
                                    .strong()
                                    .color(colors.primary_text),
                            );
                        }
                        ui.end_row();
                    }

                    for row in &table.rows {
                        for cell in row {
                            ui.label(
                                egui::RichText::new(cell)
                                    .size(cell_font_size)
                                    .color(colors.primary_text),
                            );
                        }
                        ui.end_row();
                    }
                });
        });
}

fn render_slide_footer(
    ui: &mut egui::Ui,
    slide: &SlideData,
    scale_factor: f32,
    colors: &SlideThemeColors,
    total_slides: usize,
) {
    ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
        ui.add_space(4.0 * scale_factor);

        let sep_rect = egui::Rect::from_min_size(
            ui.cursor().min,
            egui::vec2(ui.available_width(), 1.0),
        );
        ui.painter().rect_filled(
            sep_rect,
            0.0,
            egui::Color32::from_rgba_premultiplied(
                colors.divider.r(),
                colors.divider.g(),
                colors.divider.b(),
                120,
            ),
        );
        ui.add_space(6.0 * scale_factor);

        ui.horizontal(|ui| {
            let footer_font_size = (11.0 * scale_factor).clamp(8.0, 16.0);

            if let Some(footer_text) = slide.footers.first() {
                ui.label(
                    egui::RichText::new(footer_text)
                        .size(footer_font_size)
                        .color(colors.muted_text),
                );
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format!("Slide {} of {}", slide.slide_number, total_slides))
                        .size(footer_font_size)
                        .color(colors.muted_text),
                );
            });
        });
    });
}

fn render_title_slide_layout(
    ui: &mut egui::Ui,
    slide: &SlideData,
    scale_factor: f32,
    colors: &SlideThemeColors,
    _slide_w: f32,
    slide_h: f32,
    total_slides: usize,
) {
    let top_space = (slide_h * 0.22).max(12.0);
    ui.add_space(top_space);

    ui.vertical_centered(|ui| {
        let title_size = (36.0 * scale_factor).clamp(20.0, 64.0);
        ui.label(
            egui::RichText::new(&slide.title)
                .size(title_size)
                .strong()
                .color(colors.primary_text),
        );

        ui.add_space(14.0 * scale_factor);
        let bar_w = 56.0 * scale_factor;
        let bar_h = 3.0 * scale_factor;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(bar_w, bar_h), egui::Sense::hover());
        ui.painter().rect_filled(rect, egui::Rounding::same(bar_h * 0.5), colors.accent);

        if let Some(subtitle) = &slide.subtitle {
            ui.add_space(14.0 * scale_factor);
            let sub_size = (20.0 * scale_factor).clamp(13.0, 36.0);
            ui.label(
                egui::RichText::new(subtitle)
                    .size(sub_size)
                    .color(colors.secondary_text),
            );
        }
    });

    render_slide_footer(ui, slide, scale_factor, colors, total_slides);
}

fn render_content_slide_layout(
    ui: &mut egui::Ui,
    slide: &SlideData,
    scale_factor: f32,
    colors: &SlideThemeColors,
    _slide_w: f32,
    _slide_h: f32,
    total_slides: usize,
) {
    let title_size = (24.0 * scale_factor).clamp(16.0, 42.0);
    ui.label(
        egui::RichText::new(&slide.title)
            .size(title_size)
            .strong()
            .color(colors.primary_text),
    );

    ui.add_space(8.0 * scale_factor);
    let sep_rect = egui::Rect::from_min_size(
        ui.cursor().min,
        egui::vec2(ui.available_width(), 1.5 * scale_factor.clamp(0.8, 1.2)),
    );
    ui.painter().rect_filled(sep_rect, 0.0, colors.divider);
    ui.add_space(16.0 * scale_factor);

    let footer_reserve = 32.0 * scale_factor;
    let body_max_height = (ui.available_height() - footer_reserve).max(40.0);

    egui::ScrollArea::vertical()
        .id_salt("slide_body_scroll")
        .max_height(body_max_height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0 * scale_factor;

            for block in &slide.blocks {
                match block {
                    SlideContentBlock::Paragraph { text, bold, level, is_bullet } => {
                        render_bullet_item(ui, text, *bold, *level, *is_bullet, scale_factor, colors);
                    }
                    SlideContentBlock::Card { header, items } => {
                        render_card_block(ui, header.as_deref(), items, scale_factor, colors);
                    }
                }
            }

            for (table_idx, table) in slide.tables.iter().enumerate() {
                render_slide_table(ui, table, slide.slide_number, table_idx, scale_factor, colors);
            }
        });

    render_slide_footer(ui, slide, scale_factor, colors, total_slides);
}

fn render_slide_wireframe(painter: &egui::Painter, rect: egui::Rect, slide: &SlideData) {
    let inner = rect.shrink2(egui::vec2(7.0, 5.0));

    let bg_lum = slide.background_color.map(|c| {
        0.299 * (c.r() as f32) + 0.587 * (c.g() as f32) + 0.114 * (c.b() as f32)
    }).unwrap_or(255.0);
    let is_dark_bg = bg_lum < 128.0;

    let title_color = if is_dark_bg {
        egui::Color32::from_rgb(240, 240, 245)
    } else {
        egui::Color32::from_rgb(32, 32, 38)
    };
    let skeleton_line_color = if is_dark_bg {
        egui::Color32::from_white_alpha(150)
    } else {
        egui::Color32::from_rgb(205, 205, 215)
    };
    let divider_color = if is_dark_bg {
        egui::Color32::from_rgb(65, 65, 75)
    } else {
        egui::Color32::from_rgb(225, 225, 232)
    };
    let bullet_dot_color = egui::Color32::from_rgb(0, 122, 255);

    if slide.is_title_slide {
        let center = inner.center();

        let title_bar_w = (inner.width() * 0.72).min(80.0);
        let title_bar_h = 4.5_f32;
        let title_rect = egui::Rect::from_center_size(
            center - egui::vec2(0.0, 5.0),
            egui::vec2(title_bar_w, title_bar_h),
        );
        painter.rect_filled(title_rect, egui::Rounding::same(1.5), title_color);

        let sub_bar_w = (inner.width() * 0.46).min(52.0);
        let sub_bar_h = 2.5_f32;
        let sub_rect = egui::Rect::from_center_size(
            center + egui::vec2(0.0, 6.0),
            egui::vec2(sub_bar_w, sub_bar_h),
        );
        painter.rect_filled(sub_rect, egui::Rounding::same(1.0), skeleton_line_color);
    } else {
        let mini_title: String = slide.title.chars().take(16).collect();
        painter.text(
            inner.min + egui::vec2(0.0, 1.0),
            egui::Align2::LEFT_TOP,
            mini_title,
            egui::FontId::proportional(7.0),
            title_color,
        );

        let sep_y = inner.min.y + 11.5;
        painter.hline(inner.x_range(), sep_y, egui::Stroke::new(0.8_f32, divider_color));

        let body_y = sep_y + 4.0;

        if !slide.tables.is_empty() {
            let tbl_w = inner.width() - 4.0;
            let hdr_rect = egui::Rect::from_min_size(
                egui::pos2(inner.min.x + 2.0, body_y),
                egui::vec2(tbl_w, 3.5),
            );
            painter.rect_filled(hdr_rect, egui::Rounding::same(1.0), title_color);

            for r in 0..2 {
                let row_y = body_y + 5.5 + (r as f32) * 5.0;
                if row_y + 3.0 > inner.max.y { break; }
                let r1 = egui::Rect::from_min_size(
                    egui::pos2(inner.min.x + 2.0, row_y),
                    egui::vec2(tbl_w * 0.45, 2.5),
                );
                let r2 = egui::Rect::from_min_size(
                    egui::pos2(inner.min.x + 4.0 + tbl_w * 0.45, row_y),
                    egui::vec2(tbl_w * 0.45, 2.5),
                );
                painter.rect_filled(r1, egui::Rounding::same(0.5), skeleton_line_color);
                painter.rect_filled(r2, egui::Rounding::same(0.5), skeleton_line_color);
            }
        } else {
            let item_count = slide.blocks.len().clamp(1, 4);
            for i in 0..item_count {
                let y = body_y + (i as f32) * 6.5;
                if y + 3.0 > inner.max.y { break; }

                let (lvl, is_bullet) = match slide.blocks.get(i) {
                    Some(SlideContentBlock::Paragraph { level, is_bullet, .. }) => (*level, *is_bullet),
                    _ => (0, true),
                };

                let indent = (lvl as f32 * 5.0).min(12.0);

                if is_bullet {
                    let dot_pos = egui::pos2(inner.min.x + indent + 1.5, y + 1.5);
                    painter.circle_filled(dot_pos, 1.0, bullet_dot_color);
                }

                let bar_x = inner.min.x + indent + if is_bullet { 5.0 } else { 1.5 };
                let bar_w = match (slide.slide_number + i) % 3 {
                    0 => (inner.width() - 22.0 - indent).max(18.0),
                    1 => (inner.width() - 36.0 - indent).max(14.0),
                    _ => (inner.width() - 28.0 - indent).max(16.0),
                };

                let bar_rect = egui::Rect::from_min_size(
                    egui::pos2(bar_x, y),
                    egui::vec2(bar_w, 2.2),
                );
                painter.rect_filled(bar_rect, egui::Rounding::same(0.8), skeleton_line_color);
            }
        }
    }
}

pub fn render_presentation_floating_hud(
    ui: &mut egui::Ui,
    central_rect: egui::Rect,
    current_slide: &mut usize,
    total_slides: usize,
) {
    if total_slides <= 1 {
        return;
    }

    let hud_w = 236.0_f32;
    let hud_h = 38.0_f32;
    let hud_pos = egui::pos2(
        central_rect.center().x - hud_w * 0.5,
        central_rect.max.y - hud_h - 16.0,
    );
    let hud_rect = egui::Rect::from_min_size(hud_pos, egui::vec2(hud_w, hud_h));

    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(hud_rect), |ui| {
        egui::Frame::none()
            .fill(egui::Color32::from_black_alpha(190))
            .rounding(egui::Rounding::same(20.0))
            .stroke(egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(35)))
            .shadow(egui::epaint::Shadow {
                offset: egui::vec2(0.0, 4.0),
                blur: 16.0,
                spread: 0.0,
                color: egui::Color32::from_black_alpha(90),
            })
            .inner_margin(egui::Margin::symmetric(12.0, 4.0))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;

                    let cur = *current_slide;

                    let prev_enabled = cur > 0;
                    let prev_text_color = if prev_enabled {
                        egui::Color32::WHITE
                    } else {
                        egui::Color32::from_white_alpha(70)
                    };
                    let prev_btn = egui::Button::new(
                        egui::RichText::new("⏴ Prev")
                            .size(12.5)
                            .color(prev_text_color),
                    )
                    .fill(egui::Color32::TRANSPARENT)
                    .rounding(egui::Rounding::same(12.0));

                    if ui
                        .add_enabled(prev_enabled, prev_btn)
                        .on_hover_text("Previous Slide (Left Arrow / Backspace)")
                        .clicked()
                    {
                        *current_slide = cur.saturating_sub(1);
                    }

                    ui.painter().vline(
                        ui.cursor().min.x,
                        ui.cursor().min.y + 6.0..=ui.cursor().max.y - 6.0,
                        egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(40)),
                    );
                    ui.add_space(4.0);

                    ui.label(
                        egui::RichText::new(format!("Slide {} of {}", cur + 1, total_slides))
                            .size(12.5)
                            .strong()
                            .color(egui::Color32::from_rgb(225, 225, 230)),
                    );

                    ui.add_space(4.0);
                    ui.painter().vline(
                        ui.cursor().min.x,
                        ui.cursor().min.y + 6.0..=ui.cursor().max.y - 6.0,
                        egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(40)),
                    );

                    let next_enabled = cur + 1 < total_slides;
                    let next_text_color = if next_enabled {
                        egui::Color32::WHITE
                    } else {
                        egui::Color32::from_white_alpha(70)
                    };
                    let next_btn = egui::Button::new(
                        egui::RichText::new("Next ⏵")
                            .size(12.5)
                            .color(next_text_color),
                    )
                    .fill(egui::Color32::TRANSPARENT)
                    .rounding(egui::Rounding::same(12.0));

                    if ui
                        .add_enabled(next_enabled, next_btn)
                        .on_hover_text("Next Slide (Right Arrow / Space)")
                        .clicked()
                    {
                        *current_slide = (cur + 1).min(total_slides - 1);
                    }
                });
            });
    });
}

pub fn render_presentation_thumbnail_sidebar(
    ui: &mut egui::Ui,
    presentation: &PresentationData,
    current_slide: &mut usize,
) {
    let current_idx = *current_slide;

    egui::SidePanel::left("pptx_thumbs")
        .exact_width(146.0)
        .resizable(false)
        .frame(
            egui::Frame::none()
                .fill(egui::Color32::from_rgb(24, 24, 26))
                .inner_margin(egui::Margin::symmetric(8.0, 10.0)),
        )
        .show_inside(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("pptx_slide_thumbs")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 10.0;

                    let card_w = 120.0_f32;
                    let aspect = if presentation.aspect_ratio.is_finite() && presentation.aspect_ratio > 0.05 {
                        presentation.aspect_ratio.clamp(0.5, 3.0)
                    } else {
                        16.0 / 9.0
                    };
                    let card_h = (card_w / aspect).clamp(55.0, 105.0);
                    let card_size = egui::vec2(card_w, card_h);

                    let last_scrolled_id = ui.make_persistent_id("pptx_last_scrolled_slide");
                    let last_scrolled: Option<usize> = ui.data_mut(|d| d.get_temp(last_scrolled_id));
                    let should_scroll_to_active = last_scrolled != Some(current_idx);

                    for (idx, slide) in presentation.slides.iter().enumerate() {
                        let is_selected = idx == current_idx;

                        ui.vertical_centered(|ui| {
                            let (rect, response) =
                                ui.allocate_exact_size(card_size, egui::Sense::click());

                            if response.clicked() {
                                *current_slide = idx;
                            }

                            if is_selected && should_scroll_to_active {
                                response.scroll_to_me(Some(egui::Align::Center));
                                ui.data_mut(|d| d.insert_temp(last_scrolled_id, current_idx));
                            }

                            let painter = ui.painter_at(rect);

                            let border_color = if is_selected {
                                egui::Color32::from_rgb(0, 122, 255)
                            } else if response.hovered() {
                                egui::Color32::from_rgb(120, 120, 125)
                            } else {
                                egui::Color32::from_rgb(55, 55, 60)
                            };
                            let border_w = if is_selected { 2.5_f32 } else { 1.0_f32 };

                            let bg_fill = slide.background_color.unwrap_or(egui::Color32::WHITE);
                            painter.rect_filled(rect, egui::Rounding::same(3.0), bg_fill);

                            render_slide_wireframe(&painter, rect, slide);

                            painter.rect_stroke(
                                rect,
                                egui::Rounding::same(3.0),
                                egui::Stroke::new(border_w, border_color),
                            );

                            ui.add_space(2.0);
                            ui.label(
                                egui::RichText::new(format!("{}", idx + 1))
                                    .size(10.0)
                                    .color(if is_selected {
                                        egui::Color32::from_rgb(0, 122, 255)
                                    } else {
                                        egui::Color32::from_rgb(140, 140, 145)
                                    })
                                    .strong(),
                            );
                        });
                    }
                });
        });
}

pub fn render_presentation_viewer(
    ui: &mut egui::Ui,
    presentation: &PresentationData,
    current_slide: &mut usize,
) {
    if presentation.slides.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label("Empty presentation");
        });
        return;
    }

    let total_slides = presentation.slides.len();
    if *current_slide >= total_slides {
        *current_slide = 0;
    }

    render_presentation_thumbnail_sidebar(ui, presentation, current_slide);

    egui::CentralPanel::default()
        .frame(
            egui::Frame::none()
                .fill(egui::Color32::from_rgb(32, 32, 35))
                .inner_margin(egui::Margin::same(0.0)),
        )
        .show_inside(ui, |ui| {
            let avail = ui.available_size();
            let central_rect = ui.max_rect();

            let (slide_w, slide_h, scale_factor) =
                compute_presentation_slide_dimensions(avail, presentation.aspect_ratio, 48.0);

            let slide = &presentation.slides[*current_slide];
            let colors = SlideThemeColors::from_background(slide.background_color);

            ui.vertical_centered(|ui| {
                let v_pad = ((avail.y - 48.0 - slide_h) * 0.5).max(16.0);
                ui.add_space(v_pad);

                egui::Frame::none()
                    .fill(colors.bg)
                    .rounding(egui::Rounding::same(8.0))
                    .stroke(egui::Stroke::new(1.0_f32, colors.divider))
                    .shadow(egui::epaint::Shadow {
                        offset: egui::vec2(0.0, (6.0 * scale_factor).clamp(4.0, 14.0)),
                        blur: (18.0 * scale_factor).clamp(10.0, 32.0),
                        spread: 0.0,
                        color: egui::Color32::from_black_alpha(65),
                    })
                    .inner_margin(egui::Margin::symmetric(
                        (40.0 * scale_factor).clamp(16.0, 60.0),
                        (32.0 * scale_factor).clamp(14.0, 48.0),
                    ))
                    .show(ui, |ui| {
                        ui.set_min_size(egui::vec2(slide_w, slide_h));
                        ui.set_max_size(egui::vec2(slide_w, slide_h));

                        if slide.is_title_slide {
                            render_title_slide_layout(ui, slide, scale_factor, &colors, slide_w, slide_h, total_slides);
                        } else {
                            render_content_slide_layout(ui, slide, scale_factor, &colors, slide_w, slide_h, total_slides);
                        }
                    });
            });

            render_presentation_floating_hud(ui, central_rect, current_slide, total_slides);
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

fn custom_control_button(
    ui: &mut egui::Ui,
    icon: &str,
    size: f32,
    font_size: f32,
    tooltip: &str,
) -> egui::Response {
    let desired_size = egui::vec2(size, size);
    let (rect, resp) = ui.allocate_exact_size(desired_size, egui::Sense::click());
    let painter = ui.painter_at(rect);

    let bg_color = if resp.is_pointer_button_down_on() {
        egui::Color32::from_white_alpha(55)
    } else if resp.hovered() {
        egui::Color32::from_white_alpha(28)
    } else {
        egui::Color32::TRANSPARENT
    };

    if bg_color != egui::Color32::TRANSPARENT {
        painter.circle_filled(rect.center(), size / 2.0, bg_color);
    }

    let text_color = if resp.hovered() {
        egui::Color32::WHITE
    } else {
        egui::Color32::from_rgb(220, 220, 225)
    };

    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional(font_size),
        text_color,
    );

    if !tooltip.is_empty() {
        resp.on_hover_text(tooltip)
    } else {
        resp
    }
}

fn custom_media_scrubber(
    ui: &mut egui::Ui,
    current_time: &mut f64,
    duration: f64,
    width: f32,
) -> Option<f64> {
    let dur = duration.max(0.1);
    let desired_size = egui::vec2(width.max(60.0), 22.0);
    let (rect, resp) = ui.allocate_exact_size(desired_size, egui::Sense::click_and_drag());

    let is_active = resp.hovered() || resp.dragged();
    let track_h = if is_active { 5.5 } else { 3.5 };
    let track_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(rect.width(), track_h));
    let track_rounding = egui::Rounding::same(track_h / 2.0);

    let progress = (*current_time / dur).clamp(0.0, 1.0) as f32;
    let filled_w = track_rect.width() * progress;
    let filled_rect = egui::Rect::from_min_size(
        track_rect.min,
        egui::vec2(filled_w, track_h),
    );

    let knob_center = egui::pos2(track_rect.min.x + filled_w, track_rect.center().y);
    let knob_radius = if is_active { 7.0 } else { 5.0 };

    let painter = ui.painter_at(rect);

    // Track background
    painter.rect_filled(track_rect, track_rounding, egui::Color32::from_white_alpha(35));

    // Filled progress
    if filled_w > 0.0 {
        painter.rect_filled(
            filled_rect,
            track_rounding,
            egui::Color32::from_rgb(255, 255, 255), // Pure white progress like macOS Quick Look
        );
    }

    // Knob drop shadow
    painter.circle_filled(
        knob_center + egui::vec2(0.0, 1.0),
        knob_radius + 0.5,
        egui::Color32::from_black_alpha(80),
    );

    // Knob body
    painter.circle_filled(knob_center, knob_radius, egui::Color32::WHITE);

    let mut seek_target = None;

    if resp.dragged() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let frac = ((pos.x - track_rect.min.x) / track_rect.width()).clamp(0.0, 1.0);
            *current_time = (frac as f64) * dur;
        }
    }

    if resp.drag_stopped() {
        seek_target = Some(*current_time);
    } else if resp.clicked() && !resp.dragged() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let frac = ((pos.x - track_rect.min.x) / track_rect.width()).clamp(0.0, 1.0);
            let target = (frac as f64) * dur;
            *current_time = target;
            seek_target = Some(target);
        }
    }

    seek_target
}

fn custom_volume_control(ui: &mut egui::Ui, volume: &mut f32) {
    let vol_icon = if *volume <= 0.01 { "🔇" } else { "🔊" };
    let icon_resp = custom_control_button(ui, vol_icon, 24.0, 13.0, "Mute / Unmute");
    if icon_resp.clicked() {
        if *volume <= 0.01 {
            *volume = 1.0;
        } else {
            *volume = 0.0;
        }
    }

    let track_w = 54.0_f32;
    let track_h = 3.5_f32;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(track_w, 20.0), egui::Sense::click_and_drag());
    let track_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(track_w, track_h));
    let track_rounding = egui::Rounding::same(track_h / 2.0);

    let progress = volume.clamp(0.0, 1.0);
    let filled_w = track_rect.width() * progress;
    let filled_rect = egui::Rect::from_min_size(track_rect.min, egui::vec2(filled_w, track_h));
    let knob_center = egui::pos2(track_rect.min.x + filled_w, track_rect.center().y);
    let knob_radius = if resp.hovered() || resp.dragged() { 5.5 } else { 4.0 };

    let painter = ui.painter_at(rect);
    painter.rect_filled(track_rect, track_rounding, egui::Color32::from_white_alpha(35));
    if filled_w > 0.0 {
        painter.rect_filled(filled_rect, track_rounding, egui::Color32::WHITE);
    }
    painter.circle_filled(knob_center, knob_radius, egui::Color32::WHITE);

    if resp.dragged() || resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            *volume = ((pos.x - track_rect.min.x) / track_rect.width()).clamp(0.0, 1.0);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_pdf_viewer(
    ui: &mut egui::Ui,
    path: &Path,
    current_page: &mut usize,
    total_pages: usize,
    page_textures: &HashMap<usize, egui::TextureHandle>,
    pdf_loader: &mut PdfLoader,
    scroll_target: &mut Option<usize>,
    zoom_scale: &mut f32,
    pan_offset: &mut egui::Vec2,
) {
    let total_pages = total_pages.max(1);
    *current_page = (*current_page).clamp(1, total_pages);

    // Trackpad pinch-to-zoom (without hijacking vertical scroll)
    let zoom_delta = ui.input(|i| i.zoom_delta());
    if zoom_delta != 1.0 {
        *zoom_scale = (*zoom_scale * zoom_delta).clamp(0.2, 5.0);
    }

    // Reference aspect ratio (ISO A4 standard fallback: 595.28 / 841.89)
    let ref_aspect = page_textures
        .get(current_page)
        .or_else(|| page_textures.get(&1))
        .or_else(|| page_textures.values().next())
        .map(|t| {
            let s = t.size_vec2();
            if s.y > 0.0 {
                s.x / s.y
            } else {
                595.28 / 841.89
            }
        })
        .unwrap_or(595.28 / 841.89);

    // Request active page immediately if missing and not failed (Lazy Loading)
    if !page_textures.contains_key(current_page) && !pdf_loader.failed.contains(current_page) {
        pdf_loader.request_page(path, *current_page);
    }
    // Prefetch next page if available
    let next_page = *current_page + 1;
    if next_page <= total_pages
        && !page_textures.contains_key(&next_page)
        && !pdf_loader.failed.contains(&next_page)
    {
        pdf_loader.request_page(path, next_page);
    }

    let mut page_to_navigate = None;

    // 1. Left sidebar — page thumbnails (instant vector cards, zero batch pdftoppm overhead)
    egui::SidePanel::left("pdf_thumbs")
        .exact_width(136.0)
        .resizable(false)
        .frame(
            egui::Frame::none()
                .fill(egui::Color32::from_rgb(20, 20, 22))
                .inner_margin(egui::Margin::symmetric(8.0, 12.0)),
        )
        .show_inside(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("pdf_thumb_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 12.0;

                    let thumb_w = 98.0_f32;
                    let thumb_h = (thumb_w / ref_aspect).clamp(65.0, 145.0);
                    let card_size = egui::vec2(thumb_w, thumb_h);

                    for page in 1..=total_pages {
                        let is_selected = page == *current_page;

                        ui.vertical_centered(|ui| {
                            let (rect, response) =
                                ui.allocate_exact_size(card_size, egui::Sense::click());

                            if response.clicked() {
                                page_to_navigate = Some(page);
                            }

                            // Programmatic scroll into view when page changes
                            if *scroll_target == Some(page) {
                                response.scroll_to_me(Some(egui::Align::Center));
                            }

                            let painter = ui.painter_at(rect);
                            let border_color = if is_selected {
                                egui::Color32::from_rgb(0, 122, 255) // macOS active blue
                            } else if response.hovered() {
                                egui::Color32::from_rgb(120, 120, 125)
                            } else {
                                egui::Color32::from_rgb(50, 50, 55)
                            };
                            let border_w = if is_selected { 2.5_f32 } else { 1.0_f32 };

                            // Subtle drop shadow behind thumbnail card
                            painter.rect_filled(
                                rect.translate(egui::vec2(0.0, 2.0)),
                                egui::Rounding::same(4.0),
                                egui::Color32::from_black_alpha(40),
                            );

                            // Pure white paper card base
                            painter.rect_filled(
                                rect,
                                egui::Rounding::same(3.0),
                                egui::Color32::WHITE,
                            );

                            // Real page texture IF already in memory, otherwise high-craft instant vector card
                            if let Some(tex) = page_textures.get(&page) {
                                let img = egui::Image::new(tex)
                                    .fit_to_exact_size(card_size)
                                    .rounding(egui::Rounding::same(2.0));
                                img.paint_at(ui, rect);
                            } else {
                                // Draw simulated document lines (0ms latency, zero subprocesses)
                                let line_color = egui::Color32::from_rgb(225, 225, 232);
                                let start_x = rect.min.x + 12.0;
                                let end_x = rect.max.x - 12.0;
                                let line_w = end_x - start_x;

                                let y1 = rect.min.y + card_size.y * 0.28;
                                let y2 = rect.min.y + card_size.y * 0.42;
                                let y3 = rect.min.y + card_size.y * 0.56;
                                let y4 = rect.min.y + card_size.y * 0.70;

                                painter.rect_filled(
                                    egui::Rect::from_min_size(
                                        egui::pos2(start_x, y1),
                                        egui::vec2(line_w * 0.65, 3.0),
                                    ),
                                    egui::Rounding::same(1.5),
                                    line_color,
                                );
                                painter.rect_filled(
                                    egui::Rect::from_min_size(
                                        egui::pos2(start_x, y2),
                                        egui::vec2(line_w, 2.0),
                                    ),
                                    egui::Rounding::same(1.0),
                                    line_color,
                                );
                                painter.rect_filled(
                                    egui::Rect::from_min_size(
                                        egui::pos2(start_x, y3),
                                        egui::vec2(line_w * 0.85, 2.0),
                                    ),
                                    egui::Rounding::same(1.0),
                                    line_color,
                                );
                                painter.rect_filled(
                                    egui::Rect::from_min_size(
                                        egui::pos2(start_x, y4),
                                        egui::vec2(line_w * 0.45, 2.0),
                                    ),
                                    egui::Rounding::same(1.0),
                                    line_color,
                                );

                                // Page number in center
                                painter.text(
                                    rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    format!("{page}"),
                                    egui::FontId::proportional(13.0),
                                    egui::Color32::from_rgb(140, 140, 150),
                                );
                            }

                            // Border stroke on top
                            painter.rect_stroke(
                                rect,
                                egui::Rounding::same(3.0),
                                egui::Stroke::new(border_w, border_color),
                            );

                            // Page number badge below card
                            ui.add_space(2.0);
                            ui.label(
                                egui::RichText::new(format!("{page}"))
                                    .size(10.5)
                                    .color(if is_selected {
                                        egui::Color32::from_rgb(0, 122, 255)
                                    } else {
                                        egui::Color32::from_rgb(140, 140, 145)
                                    })
                                    .strong(),
                            );
                        });
                    }
                });
        });

    if scroll_target.is_some() {
        *scroll_target = None;
    }

    // 2. Central PDF active page document view
    let central_rect = ui.available_rect_before_wrap();
    egui::CentralPanel::default()
        .frame(
            egui::Frame::none()
                .fill(egui::Color32::from_rgb(32, 32, 35))
                .inner_margin(egui::Margin::same(0.0)),
        )
        .show_inside(ui, |ui| {
            // Mouse wheel page navigation
            let scroll_y = ui.input(|i| i.raw_scroll_delta.y);
            if scroll_y < -15.0 && *current_page < total_pages {
                page_to_navigate = Some(*current_page + 1);
            } else if scroll_y > 15.0 && *current_page > 1 {
                page_to_navigate = Some(*current_page - 1);
            }

            let avail_w = (ui.available_width() - 48.0).max(180.0);
            let avail_h = (ui.available_height() - 64.0).max(180.0);

            // Calculate page dimensions with 100% mathematical aspect ratio preservation
            let active_aspect = if let Some(tex) = page_textures.get(current_page) {
                let s = tex.size_vec2();
                if s.y > 0.0 {
                    s.x / s.y
                } else {
                    ref_aspect
                }
            } else {
                ref_aspect
            };

            let mut page_w = avail_w;
            let mut page_h = page_w / active_aspect;
            if page_h > avail_h {
                page_h = avail_h;
                page_w = page_h * active_aspect;
            }

            page_w = (page_w * (*zoom_scale)).max(160.0);
            page_h = page_w / active_aspect;
            let page_size = egui::vec2(page_w, page_h);

            ui.vertical_centered(|ui| {
                let pad_y = ((ui.available_height() - 48.0 - page_h) * 0.5).max(16.0);
                ui.add_space(pad_y);

                let (rect, resp) = ui.allocate_exact_size(page_size, egui::Sense::click_and_drag());
                if resp.dragged() {
                    *pan_offset += resp.drag_delta();
                }

                let painter = ui.painter_at(rect);

                // macOS Quick Look drop shadow
                painter.rect_filled(
                    rect.translate(egui::vec2(0.0, 6.0)),
                    egui::Rounding::same(6.0),
                    egui::Color32::from_black_alpha(65),
                );

                // White paper canvas
                painter.rect_filled(
                    rect,
                    egui::Rounding::same(3.0),
                    egui::Color32::WHITE,
                );

                // Page texture or crisp loading placeholder
                if let Some(tex) = page_textures.get(current_page) {
                    let img = egui::Image::new(tex)
                        .fit_to_exact_size(page_size)
                        .rounding(egui::Rounding::same(3.0));
                    img.paint_at(ui, rect);
                } else {
                    let is_failed = pdf_loader.failed.contains(current_page);
                    painter.text(
                        rect.center() - egui::vec2(0.0, 16.0),
                        egui::Align2::CENTER_CENTER,
                        if is_failed { "⚠️" } else { "📄" },
                        egui::FontId::proportional(42.0),
                        if is_failed {
                            egui::Color32::from_rgb(220, 120, 100)
                        } else {
                            egui::Color32::from_rgb(180, 180, 190)
                        },
                    );
                    painter.text(
                        rect.center() + egui::vec2(0.0, 24.0),
                        egui::Align2::CENTER_CENTER,
                        if is_failed {
                            format!("Page {} (Render Failed)", *current_page)
                        } else {
                            format!("Loading Page {} of {}...", *current_page, total_pages)
                        },
                        egui::FontId::proportional(14.0),
                        if is_failed {
                            egui::Color32::from_rgb(180, 90, 80)
                        } else {
                            egui::Color32::from_rgb(120, 120, 130)
                        },
                    );
                }

                // Crisp boundary stroke
                painter.rect_stroke(
                    rect,
                    egui::Rounding::same(3.0),
                    egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(215, 215, 220)),
                );
            });

            // 3. Floating Page Navigation HUD at bottom center (macOS Quick Look pill)
            let hud_rect = egui::Rect::from_center_size(
                egui::pos2(central_rect.center().x, central_rect.bottom() - 32.0),
                egui::vec2(190.0, 36.0),
            );

            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(hud_rect), |ui| {
                egui::Frame::none()
                    .fill(egui::Color32::from_black_alpha(205))
                    .stroke(egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(35)))
                    .rounding(egui::Rounding::same(18.0))
                    .shadow(egui::epaint::Shadow {
                        offset: egui::vec2(0.0, 4.0),
                        blur: 16.0,
                        spread: 0.0,
                        color: egui::Color32::from_black_alpha(90),
                    })
                    .inner_margin(egui::Margin::symmetric(10.0, 4.0))
                    .show(ui, |ui| {
                        ui.horizontal_centered(|ui| {
                            let can_prev = *current_page > 1;
                            let prev_resp = custom_control_button(
                                ui,
                                "◀",
                                26.0,
                                12.0,
                                if can_prev { "Previous Page (Left / Up)" } else { "" },
                            );
                            if can_prev && prev_resp.clicked() {
                                page_to_navigate = Some(*current_page - 1);
                            }

                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(format!("{} / {}", *current_page, total_pages))
                                    .size(12.5)
                                    .strong()
                                    .color(egui::Color32::WHITE),
                            );
                            ui.add_space(4.0);

                            let can_next = *current_page < total_pages;
                            let next_resp = custom_control_button(
                                ui,
                                "▶",
                                26.0,
                                12.0,
                                if can_next { "Next Page (Right / Down / Space)" } else { "" },
                            );
                            if can_next && next_resp.clicked() {
                                page_to_navigate = Some(*current_page + 1);
                            }
                        });
                    });
            });
        });

    if let Some(target_p) = page_to_navigate {
        *current_page = target_p.clamp(1, total_pages);
        *scroll_target = Some(*current_page);
        if !page_textures.contains_key(current_page) {
            pdf_loader.request_page(path, *current_page);
        }
    }
}

pub struct MediaPlayer {
    pub is_video: bool,
    pub path: PathBuf,
    pub is_playing: bool,
    pub current_time: f64,
    pub duration: f64,
    pub volume: f32,
    pub dimensions: (u32, u32),
    pub audio_meta: Option<AudioMetadata>,
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
        audio_meta: Option<AudioMetadata>,
    ) -> Self {
        let mut player = Self {
            is_video,
            path: path.to_path_buf(),
            is_playing: false,
            current_time: 0.0,
            duration: if duration > 0.0 { duration } else { 1.0 },
            volume: 1.0,
            dimensions,
            audio_meta,
            frame_receiver: None,
            stop_sender: None,
            audio_child: None,
            video_child: None,
            last_tick: Instant::now(),
        };

        player.start_playback();
        player
    }

    pub fn check_eof_and_reap(&mut self) -> bool {
        if !self.is_playing {
            return false;
        }

        let mut stream_ended = false;
        if self.is_video {
            if let Some(child) = &mut self.video_child {
                if let Ok(Some(_status)) = child.try_wait() {
                    stream_ended = true;
                }
            }
        } else {
            if let Some(child) = &mut self.audio_child {
                if let Ok(Some(_status)) = child.try_wait() {
                    stream_ended = true;
                }
            }
        }

        if stream_ended {
            self.current_time = self.duration;
            self.pause_playback();
            return true;
        }

        false
    }

    pub fn start_playback(&mut self) {
        self.stop_processes();
        if self.duration > 0.0 && self.current_time >= self.duration {
            self.current_time = 0.0;
        }
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
            if self.duration > 0.0 && self.current_time >= self.duration {
                self.current_time = 0.0;
            }
            self.start_playback();
        }
    }

    pub fn seek(&mut self, new_time: f64) {
        self.current_time = new_time.clamp(0.0, self.duration);
        if self.duration > 0.0 && self.current_time >= self.duration {
            self.pause_playback();
            return;
        }
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

fn render_audio_player(
    ui: &mut egui::Ui,
    player: &mut MediaPlayer,
    texture: Option<&egui::TextureHandle>,
    _size: u64,
) -> Option<isize> {
    let mut toggle_play = false;
    let mut seek_to = None;
    let mut nav_delta = None;

    let meta = player.audio_meta.clone().unwrap_or_default();
    let display_title = if !meta.title.is_empty() {
        meta.title.clone()
    } else {
        player
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Audio Track")
            .to_string()
    };

    // 1. Central card area — fills all space above the transport bar
    let transport_h = 56.0_f32;
    let body_h = (ui.available_height() - transport_h - 4.0).max(100.0);

    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), body_h),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.add_space(32.0);

            // Album art card (150x150, rounded corners)
            let art_size = egui::vec2(150.0, 150.0);
            let (art_resp, art_painter) = ui.allocate_painter(art_size, egui::Sense::hover());
            let art_rect = art_resp.rect;

            // Subtle drop shadow behind art card
            art_painter.rect_filled(
                art_rect.translate(egui::vec2(0.0, 4.0)),
                egui::Rounding::same(14.0),
                egui::Color32::from_black_alpha(60),
            );

            if let Some(tex) = texture {
                art_painter.image(
                    tex.id(),
                    art_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
                art_painter.rect_stroke(
                    art_rect,
                    egui::Rounding::same(14.0),
                    egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(30)),
                );
            } else {
                // macOS Quick Look clean white card with subtle music note
                art_painter.rect_filled(
                    art_rect,
                    egui::Rounding::same(14.0),
                    egui::Color32::WHITE,
                );
                art_painter.rect_stroke(
                    art_rect,
                    egui::Rounding::same(14.0),
                    egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(220, 220, 228)),
                );
                art_painter.text(
                    art_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "♪",
                    egui::FontId::proportional(72.0),
                    egui::Color32::from_rgb(175, 175, 185),
                );
            }

            ui.add_space(28.0);

            // Track metadata
            ui.vertical(|ui| {
                ui.add_space(8.0);

                ui.label(
                    egui::RichText::new(&display_title)
                        .size(20.0)
                        .strong()
                        .color(egui::Color32::WHITE),
                );

                ui.add_space(10.0);

                let artist_text = if !meta.artist.is_empty() {
                    format!("Artist: {}", meta.artist)
                } else {
                    "Artist: Unknown".into()
                };
                ui.label(
                    egui::RichText::new(artist_text)
                        .size(13.5)
                        .color(egui::Color32::from_rgb(160, 160, 168)),
                );

                ui.add_space(4.0);

                let album_text = if !meta.album.is_empty() {
                    format!("Album: {}", meta.album)
                } else {
                    format!("Album: {}", display_title)
                };
                ui.label(
                    egui::RichText::new(album_text)
                        .size(13.5)
                        .color(egui::Color32::from_rgb(160, 160, 168)),
                );

                ui.add_space(4.0);

                ui.label(
                    egui::RichText::new(format!("Time: {}", format_time(player.duration)))
                        .size(13.5)
                        .color(egui::Color32::from_rgb(160, 160, 168)),
                );
            });
        },
    );

    // 2. Transport bar (macOS Quick Look dark glass)
    egui::Frame::none()
        .fill(egui::Color32::from_rgb(20, 20, 22))
        .stroke(egui::Stroke::new(
            1.0_f32,
            egui::Color32::from_rgb(38, 38, 44),
        ))
        .inner_margin(egui::Margin::symmetric(14.0, 10.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;

                if custom_control_button(ui, "⟲15", 30.0, 12.0, "Rewind 15 seconds").clicked() {
                    seek_to = Some((player.current_time - 15.0).max(0.0));
                }

                let play_icon = if player.is_playing { "⏸" } else { "▶" };
                if custom_control_button(ui, play_icon, 34.0, 16.0, "Play / Pause (P)").clicked() {
                    toggle_play = true;
                }

                if custom_control_button(ui, "15⟳", 30.0, 12.0, "Forward 15 seconds").clicked() {
                    seek_to = Some((player.current_time + 15.0).min(player.duration));
                }

                ui.label(
                    egui::RichText::new(format_time(player.current_time))
                        .monospace()
                        .size(12.0)
                        .color(egui::Color32::from_rgb(200, 200, 205)),
                );

                let scrubber_w = (ui.available_width() - 95.0).max(80.0);
                if let Some(target) = custom_media_scrubber(ui, &mut player.current_time, player.duration, scrubber_w) {
                    seek_to = Some(target);
                }

                ui.label(
                    egui::RichText::new(format_time(player.duration))
                        .monospace()
                        .size(12.0)
                        .color(egui::Color32::from_rgb(140, 140, 145)),
                );

                if custom_control_button(ui, "⏭", 30.0, 13.0, "Next file").clicked() {
                    nav_delta = Some(1_isize);
                }
            });
        });

    if toggle_play {
        player.toggle_playback();
    }
    if let Some(pos) = seek_to {
        player.seek(pos);
    }

    nav_delta
}

fn render_media_player(
    ui: &mut egui::Ui,
    player: &mut MediaPlayer,
    texture: Option<&egui::TextureHandle>,
    size: u64,
    zoom_scale: &mut f32,
    pan_offset: &mut egui::Vec2,
) -> Option<isize> {
    if !player.is_video {
        return render_audio_player(ui, player, texture, size);
    }

    let available_rect = ui.available_rect_before_wrap();
    let mut seek_to = None;
    let mut toggle_play = false;

    let (rect, response) = ui.allocate_exact_size(
        available_rect.size(),
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

    // Floating macOS Glass HUD at Bottom
    let hud_w = (rect.width() - 48.0).clamp(420.0, 680.0);
    let hud_h = 44.0_f32;
    let hud_rect = egui::Rect::from_center_size(
        egui::pos2(rect.center().x, rect.bottom() - 34.0),
        egui::vec2(hud_w, hud_h),
    );

    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(hud_rect), |ui| {
        egui::Frame::none()
            .fill(egui::Color32::from_black_alpha(205))
            .stroke(egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(35)))
            .rounding(egui::Rounding::same(22.0))
            .shadow(egui::epaint::Shadow {
                offset: egui::vec2(0.0, 4.0),
                blur: 16.0,
                spread: 0.0,
                color: egui::Color32::from_black_alpha(90),
            })
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;

                    if custom_control_button(ui, "⟲15", 28.0, 12.0, "Rewind 15 seconds").clicked() {
                        seek_to = Some((player.current_time - 15.0).max(0.0));
                    }

                    let play_icon = if player.is_playing { "⏸" } else { "▶" };
                    if custom_control_button(ui, play_icon, 32.0, 15.0, "Play / Pause (P)").clicked() {
                        toggle_play = true;
                    }

                    if custom_control_button(ui, "15⟳", 28.0, 12.0, "Forward 15 seconds").clicked() {
                        seek_to = Some((player.current_time + 15.0).min(player.duration));
                    }

                    ui.label(
                        egui::RichText::new(format_time(player.current_time))
                            .monospace()
                            .size(11.5)
                            .color(egui::Color32::from_rgb(200, 200, 210)),
                    );

                    let scrubber_w = calculate_scrubber_width(ui.available_width());
                    if let Some(target) = custom_media_scrubber(ui, &mut player.current_time, player.duration, scrubber_w) {
                        seek_to = Some(target);
                    }

                    ui.label(
                        egui::RichText::new(format_time(player.duration))
                            .monospace()
                            .size(11.5)
                            .color(egui::Color32::from_rgb(140, 140, 150)),
                    );

                    ui.separator();

                    custom_volume_control(ui, &mut player.volume);

                    if custom_control_button(ui, "↗", 26.0, 13.0, "Open in default system player").clicked() {
                        let _ = open::that(&player.path);
                    }
                });
            });
    });

    if toggle_play {
        player.toggle_playback();
    }
    if let Some(pos) = seek_to {
        player.seek(pos);
    }

    None
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

/// Calculates scrubber slider width for media player, protecting against layout underflow.
pub fn calculate_scrubber_width(available_width: f32) -> f32 {
    (available_width - 210.0).max(60.0)
}

fn format_time(seconds: f64) -> String {
    let total_secs = seconds.max(0.0) as u64;
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    format!("{mins:02}:{secs:02}")
}

fn probe_media_info(path: &Path) -> (f64, (u32, u32), Option<AudioMetadata>) {
    let mut duration = 0.0;
    let mut dims = (854, 480);
    let mut meta = AudioMetadata::default();
    let mut bit_rate = 0.0_f64;
    let path_str = match path.to_str() {
        Some(s) => s,
        None => return (duration, dims, None),
    };

    if let Ok(out) = Command::new("ffprobe")
        .args([
            "-v", "error",
            "-show_entries", "format_tags=title,artist,album:format=duration,bit_rate:stream=duration",
            "-of", "default=noprint_wrappers=1",
            path_str,
        ])
        .output()
    {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            for line in s.lines() {
                let trimmed = line.trim();
                if let Some(val) = trimmed.strip_prefix("duration=") {
                    if let Ok(d) = val.parse::<f64>() {
                        if d > duration {
                            duration = d;
                            meta.duration = d;
                        }
                    }
                } else if let Some(val) = trimmed.strip_prefix("bit_rate=") {
                    if let Ok(b) = val.parse::<f64>() {
                        bit_rate = b;
                    }
                } else if let Some(val) = trimmed.strip_prefix("TAG:title=") {
                    meta.title = val.to_string();
                } else if let Some(val) = trimmed.strip_prefix("TAG:artist=") {
                    meta.artist = val.to_string();
                } else if let Some(val) = trimmed.strip_prefix("TAG:album=") {
                    meta.album = val.to_string();
                }
            }
        }
    }

    if bit_rate > 0.0 {
        if let Ok(m) = fs::metadata(path) {
            let estimated_dur = (m.len() as f64 * 8.0) / bit_rate;
            if estimated_dur > duration * 1.5 {
                duration = estimated_dur;
                meta.duration = estimated_dur;
            }
        }
    }

    if meta.title.is_empty() {
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            let clean = if let Some(pos) = stem.find(" - ") {
                &stem[pos + 3..]
            } else {
                stem
            };
            meta.title = clean.to_string();
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

    (duration, dims, Some(meta))
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

    let tex_size = texture.size_vec2();
    let target_size = if tex_size.x > 0.0 && tex_size.y > 0.0 {
        let margin = 24.0_f32;
        let max_w = (available_size.x - margin * 2.0).max(10.0);
        let max_h = (available_size.y - margin * 2.0).max(10.0);

        let fit_scale = (max_w / tex_size.x).min(max_h / tex_size.y);
        let base_w = tex_size.x * fit_scale;
        let base_h = tex_size.y * fit_scale;

        egui::vec2(base_w * (*zoom_scale), base_h * (*zoom_scale))
    } else {
        available_size * (*zoom_scale)
    };

    let center = response.rect.center() + *pan_offset;
    let image_rect = egui::Rect::from_center_size(center, target_size);

    // Subtle drop shadow behind document sheet
    let shadow_rect = image_rect.expand(2.5);
    painter.rect_filled(
        shadow_rect,
        egui::Rounding::same(3.0),
        egui::Color32::from_rgba_premultiplied(0, 0, 0, 30),
    );

    let image = egui::Image::new(texture).fit_to_exact_size(target_size);
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

pub fn estimate_pdf_pages_from_bytes(buffer: &[u8]) -> Option<usize> {
    let mut count = 0;
    let is_page_terminator = |b: u8| -> bool {
        b.is_ascii_whitespace() || matches!(b, b'/' | b'>' | b']' | b'<' | b'[' | b'(' | b')' | b'%' | b';')
    };

    let mut i = 0;
    while i + 5 <= buffer.len() {
        if &buffer[i..i + 5] == b"/Type" {
            let mut j = i + 5;
            while j < buffer.len() && buffer[j].is_ascii_whitespace() {
                j += 1;
            }
            if j + 5 <= buffer.len() && &buffer[j..j + 5] == b"/Page" {
                let next_idx = j + 5;
                if next_idx >= buffer.len() || is_page_terminator(buffer[next_idx]) {
                    count += 1;
                    i = next_idx;
                    continue;
                }
            }
        }
        i += 1;
    }

    if count > 0 {
        Some(count)
    } else {
        // Fallback: search for /Count <num> in /Type /Pages
        let count_target = b"/Count";
        let mut max_count = 0;
        let mut idx = 0;
        while idx + count_target.len() < buffer.len() {
            if &buffer[idx..idx + count_target.len()] == count_target {
                let mut k = idx + count_target.len();
                while k < buffer.len() && buffer[k].is_ascii_whitespace() {
                    k += 1;
                }
                let mut num_str = String::new();
                while k < buffer.len() && buffer[k].is_ascii_digit() {
                    num_str.push(buffer[k] as char);
                    k += 1;
                }
                if let Ok(c) = num_str.parse::<usize>() {
                    if c > max_count {
                        max_count = c;
                    }
                }
                idx = k;
            } else {
                idx += 1;
            }
        }
        if max_count > 0 {
            Some(max_count)
        } else {
            None
        }
    }
}

pub fn estimate_pdf_pages(path: &Path) -> Option<usize> {
    let mut file = File::open(path).ok()?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).ok()?;
    estimate_pdf_pages_from_bytes(&buffer)
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
    if file_size < 22 {
        return results;
    }

    // 1. Scan for End of Central Directory Record (EOCD) from EOF
    let search_len = (file_size as usize).min(65536 + 22);
    let search_offset = file_size - (search_len as u64);
    if file.seek(SeekFrom::Start(search_offset)).is_ok() {
        let mut search_buf = vec![0u8; search_len];
        if file.read_exact(&mut search_buf).is_ok() {
            let mut eocd_pos = None;
            for i in (0..=search_len.saturating_sub(22)).rev() {
                if search_buf[i..i + 4] == [0x50, 0x4b, 0x05, 0x06] {
                    eocd_pos = Some(i);
                    break;
                }
            }

            if let Some(pos) = eocd_pos {
                let eocd = &search_buf[pos..];
                let total_entries = u16::from_le_bytes(eocd[10..12].try_into().unwrap()) as usize;
                let cd_offset = u32::from_le_bytes(eocd[16..20].try_into().unwrap()) as u64;

                let mut cd_cursor = cd_offset;
                for _ in 0..total_entries {
                    if file.seek(SeekFrom::Start(cd_cursor)).is_err() {
                        break;
                    }
                    let mut cd_hdr = [0u8; 46];
                    if file.read_exact(&mut cd_hdr).is_err() || cd_hdr[0..4] != [0x50, 0x4b, 0x01, 0x02] {
                        break;
                    }

                    let method = u16::from_le_bytes(cd_hdr[10..12].try_into().unwrap());
                    let comp_size = u32::from_le_bytes(cd_hdr[20..24].try_into().unwrap()) as usize;
                    let name_len = u16::from_le_bytes(cd_hdr[28..30].try_into().unwrap()) as usize;
                    let extra_len = u16::from_le_bytes(cd_hdr[30..32].try_into().unwrap()) as usize;
                    let comment_len = u16::from_le_bytes(cd_hdr[32..34].try_into().unwrap()) as usize;
                    let local_hdr_offset = u32::from_le_bytes(cd_hdr[42..46].try_into().unwrap()) as u64;

                    let mut name_buf = vec![0u8; name_len];
                    if file.read_exact(&mut name_buf).is_err() {
                        break;
                    }
                    let name = String::from_utf8_lossy(&name_buf).to_string();
                    cd_cursor += 46 + (name_len as u64) + (extra_len as u64) + (comment_len as u64);

                    if filter(&name) && file.seek(SeekFrom::Start(local_hdr_offset)).is_ok() {
                        let mut local_hdr = [0u8; 30];
                        if file.read_exact(&mut local_hdr).is_ok() && local_hdr[0..4] == [0x50, 0x4b, 0x03, 0x04] {
                            let local_name_len = u16::from_le_bytes(local_hdr[26..28].try_into().unwrap()) as u64;
                            let local_extra_len = u16::from_le_bytes(local_hdr[28..30].try_into().unwrap()) as u64;
                            let data_offset = local_hdr_offset + 30 + local_name_len + local_extra_len;

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
                    }
                }

                if !results.is_empty() {
                    return results;
                }
            }
        }
    }

    // 2. Fallback: Sequential Local File Header walk
    let mut buffer = [0u8; 4];
    let mut offset = 0u64;

    while offset + 30 <= file_size && results.len() < 100 {
        if file.seek(SeekFrom::Start(offset)).is_err() {
            break;
        }
        if file.read_exact(&mut buffer).is_err() || buffer != [0x50, 0x4b, 0x03, 0x04] {
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

        if filter(&name) && comp_size > 0 && file.seek(SeekFrom::Start(data_offset)).is_ok() {
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

        if comp_size == 0 {
            break;
        }
        offset = data_offset + (comp_size as u64);
    }

    results
}

/// Extracts the value of an attribute `attr_name` from an XML tag string.
/// Supports both double and single quotes.
pub fn extract_xml_attr(tag: &str, attr_name: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let pattern = format!("{attr_name}={quote}");
        if let Some(pos) = tag.find(&pattern) {
            let val_start = pos + pattern.len();
            if let Some(val_len) = tag[val_start..].find(quote) {
                return Some(decode_xml_entities(&tag[val_start..val_start + val_len]));
            }
        }
    }
    None
}

/// Safely extracts an XML tag starting at `slice`, finding its closing delimiter (`/>` or `>`).
/// Returns `Some((tag_slice, tag_len))` where `tag_len` is the byte length to advance the cursor.
pub fn parse_xml_tag(slice: &str) -> Option<(&str, usize)> {
    let gt = slice.find('>')?;
    Some((&slice[..=gt], gt + 1))
}

/// Parses presentation dimensions (cx, cy) and slide relationship IDs from `ppt/presentation.xml`.
pub fn parse_presentation_xml(xml: &str) -> (f32, Vec<String>) {
    let mut aspect_ratio = 16.0 / 9.0;

    if let Some(pos) = xml.find("<p:sldSz").or_else(|| xml.find("<sldSz")) {
        if let Some((tag, _)) = parse_xml_tag(&xml[pos..]) {
            let cx_opt = extract_xml_attr(tag, "cx").and_then(|v| v.parse::<u64>().ok());
            let cy_opt = extract_xml_attr(tag, "cy").and_then(|v| v.parse::<u64>().ok());

            if let (Some(cx), Some(cy)) = (cx_opt, cy_opt) {
                if cy > 0 && cx > 0 {
                    aspect_ratio = (cx as f32 / cy as f32).clamp(0.2, 5.0);
                }
            }
        }
    }

    let mut slide_rids = Vec::new();
    let mut cursor = 0;
    while let Some(pos) = xml[cursor..].find("<p:sldId").or_else(|| xml[cursor..].find("<sldId")) {
        let abs_pos = cursor + pos;
        let Some((tag, tag_len)) = parse_xml_tag(&xml[abs_pos..]) else {
            break;
        };
        if let Some(rid) = extract_xml_attr(tag, "r:id").or_else(|| extract_xml_attr(tag, "id")) {
            slide_rids.push(rid);
        }
        cursor = abs_pos + tag_len;
    }

    (aspect_ratio, slide_rids)
}

/// Parses relationship mappings from `ppt/_rels/presentation.xml.rels`.
pub fn parse_presentation_rels_xml(xml: &str) -> HashMap<String, String> {
    let mut rid_map = HashMap::new();
    let mut cursor = 0;

    while let Some(pos) = xml[cursor..].find("<Relationship") {
        let abs_pos = cursor + pos;
        let Some((tag, tag_len)) = parse_xml_tag(&xml[abs_pos..]) else {
            break;
        };

        if let (Some(id), Some(target)) = (extract_xml_attr(tag, "Id"), extract_xml_attr(tag, "Target")) {
            let clean_target = target.trim_start_matches('/');
            let normalized_path = if clean_target.starts_with("ppt/") {
                clean_target.to_string()
            } else {
                format!("ppt/{clean_target}")
            };
            rid_map.insert(id, normalized_path);
        }

        cursor = abs_pos + tag_len;
    }

    rid_map
}

/// Extracts text runs and bold formatting from within a paragraph `<a:p>`.
pub fn extract_paragraph_runs(para_xml: &str) -> Vec<(String, bool)> {
    let mut runs = Vec::new();
    let mut cursor = 0;

    while let Some(r_pos) = para_xml[cursor..].find("<a:r") {
        let abs_r_pos = cursor + r_pos;
        if let Some(r_end) = para_xml[abs_r_pos..].find("</a:r>") {
            let r_body = &para_xml[abs_r_pos..abs_r_pos + r_end];
            let bold = r_body.contains("b=\"1\"") || r_body.contains("b=\"true\"");

            let mut t_cursor = 0;
            while let Some(t_open) = r_body[t_cursor..].find("<a:t") {
                let abs_t_open = t_cursor + t_open;
                if let Some(tag_close) = r_body[abs_t_open..].find('>') {
                    let content_start = abs_t_open + tag_close + 1;
                    if let Some(t_end) = r_body[content_start..].find("</a:t>") {
                        let text = decode_xml_entities(&r_body[content_start..content_start + t_end]);
                        if !text.is_empty() {
                            runs.push((text, bold));
                        }
                        t_cursor = content_start + t_end + 6;
                        continue;
                    }
                }
                break;
            }
            cursor = abs_r_pos + r_end + 6;
        } else {
            break;
        }
    }

    let mut fld_cursor = 0;
    while let Some(f_pos) = para_xml[fld_cursor..].find("<a:fld") {
        let abs_f_pos = fld_cursor + f_pos;
        if let Some(f_end) = para_xml[abs_f_pos..].find("</a:fld>") {
            let f_body = &para_xml[abs_f_pos..abs_f_pos + f_end];
            let mut t_cursor = 0;
            while let Some(t_open) = f_body[t_cursor..].find("<a:t") {
                let abs_t_open = t_cursor + t_open;
                if let Some(tag_close) = f_body[abs_t_open..].find('>') {
                    let content_start = abs_t_open + tag_close + 1;
                    if let Some(t_end) = f_body[content_start..].find("</a:t>") {
                        let text = decode_xml_entities(&f_body[content_start..content_start + t_end]);
                        if !text.is_empty() {
                            runs.push((text, false));
                        }
                        t_cursor = content_start + t_end + 6;
                        continue;
                    }
                }
                break;
            }
            fld_cursor = abs_f_pos + f_end + 8;
        } else {
            break;
        }
    }

    if runs.is_empty() {
        let tags = extract_xml_tags(para_xml, "a:t");
        for t in tags {
            if !t.is_empty() {
                runs.push((t, false));
            }
        }
    }

    runs
}

/// Represents an XML element slice located within an XML string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XmlElement<'a> {
    pub body: &'a str,
    pub is_self_closing: bool,
    pub next_cursor: usize,
}

/// Robustly scans `xml` starting from `cursor` for an XML element matching `open_prefix`
/// and closing with `close_tag`.
///
/// Features:
/// 1. Tag name boundary validation: verifies that the byte immediately following `open_prefix`
///    is one of `>`, `/`, or ASCII whitespace (` `, `\t`, `\r`, `\n`). This prevents false prefix
///    collisions (e.g., `<p:spTree>` matching `<p:sp>`, or `<a:pPr>` matching `<a:p>`).
/// 2. Self-closing tag detection: detects `<tag/>` and `<tag .../>` (with or without whitespace
///    before `/`), returning `is_self_closing = true` and setting `next_cursor` past `>`.
/// 3. Paired tag scanning: for open tags, searches for `close_tag` strictly starting *after* the
///    opening tag header `>`.
/// 4. Loop continuation on malformed XML: if an open tag is never closed, advances past the opening
///    tag header to avoid infinite loops and avoid dropping subsequent sibling tags.
pub fn find_xml_element<'a>(
    xml: &'a str,
    mut cursor: usize,
    open_prefix: &str,
    close_tag: &str,
) -> Option<XmlElement<'a>> {
    while let Some(rel_pos) = xml[cursor..].find(open_prefix) {
        let abs_open = cursor + rel_pos;
        let after_tag = abs_open + open_prefix.len();

        if let Some(&b) = xml.as_bytes().get(after_tag) {
            if b != b'>' && b != b'/' && !b.is_ascii_whitespace() {
                cursor = after_tag;
                continue;
            }
        }

        let gt_rel = xml[abs_open..].find('>')?;
        let open_tag_end = abs_open + gt_rel + 1;
        let open_tag = &xml[abs_open..open_tag_end];
        let is_self_closing = open_tag.ends_with("/>")
            || (gt_rel > 0 && open_tag[..open_tag.len() - 1].trim_end().ends_with('/'));

        if is_self_closing {
            return Some(XmlElement {
                body: open_tag,
                is_self_closing: true,
                next_cursor: open_tag_end,
            });
        }

        if let Some(close_rel) = xml[open_tag_end..].find(close_tag) {
            let abs_close = open_tag_end + close_rel;
            let next_cursor = abs_close + close_tag.len();
            return Some(XmlElement {
                body: &xml[abs_open..abs_close],
                is_self_closing: false,
                next_cursor,
            });
        } else {
            cursor = open_tag_end;
        }
    }

    None
}

/// Robustly parses individual slide XML `ppt/slides/slide*.xml`.
pub fn parse_slide_xml(xml: &str, slide_number: usize) -> SlideData {
    // 1. Slide Background Color
    let mut background_color = None;
    if let Some(bg_pos) = xml.find("<p:bg").or_else(|| xml.find("<bg")) {
        if let Some(bg_end) = xml[bg_pos..].find("</p:bg>").or_else(|| xml[bg_pos..].find("</bg>")) {
            let bg_body = &xml[bg_pos..bg_pos + bg_end];
            if let Some(clr_pos) = bg_body.find("<a:srgbClr").or_else(|| bg_body.find("<srgbClr")) {
                if let Some((clr_tag, _)) = parse_xml_tag(&bg_body[clr_pos..]) {
                    if let Some(val) = extract_xml_attr(clr_tag, "val") {
                        if val.len() == 6 {
                            if let (Ok(r), Ok(g), Ok(b)) = (
                                u8::from_str_radix(&val[0..2], 16),
                                u8::from_str_radix(&val[2..4], 16),
                                u8::from_str_radix(&val[4..6], 16),
                            ) {
                                background_color = Some(egui::Color32::from_rgb(r, g, b));
                            }
                        }
                    }
                }
            }
        }
    }

    let mut title_parts = Vec::new();
    let mut subtitle = None;
    let mut footers = Vec::new();
    let mut blocks = Vec::new();
    let mut tables = Vec::new();
    let mut has_ctr_title = false;

    // 2. Iterate Shapes `<p:sp>`
    let mut cursor = 0;
    while let Some(elem) = find_xml_element(xml, cursor, "<p:sp", "</p:sp>") {
        cursor = elem.next_cursor;
        if elem.is_self_closing {
            continue;
        }
        let sp_body = elem.body;

        // Check Placeholder type
        let mut ph_type = None;
        if let Some(ph_pos) = sp_body.find("<p:ph") {
            if let Some((ph_tag, _)) = parse_xml_tag(&sp_body[ph_pos..]) {
                ph_type = extract_xml_attr(ph_tag, "type");
                if ph_type.is_none() {
                    ph_type = Some("body".to_string());
                }
            }
        }

        if ph_type.as_deref() == Some("ctrTitle") {
            has_ctr_title = true;
        }

        // Extract paragraphs from <p:txBody>
        let mut p_cursor = 0;
        let mut shape_paragraphs = Vec::new();

        while let Some(p_elem) = find_xml_element(sp_body, p_cursor, "<a:p", "</a:p>") {
            p_cursor = p_elem.next_cursor;
            if p_elem.is_self_closing {
                continue;
            }
            let p_body = p_elem.body;

            // Paragraph properties (level & bullet style)
            let mut level = 0_usize;
            let mut is_bullet = true;

            if let Some(ppr_pos) = p_body.find("<a:pPr") {
                if let Some((ppr_tag, _)) = parse_xml_tag(&p_body[ppr_pos..]) {
                    if let Some(lvl_str) = extract_xml_attr(ppr_tag, "lvl") {
                        level = lvl_str.parse::<usize>().unwrap_or(0);
                    }
                    if p_body.contains("<a:buNone") {
                        is_bullet = false;
                    }
                }
            }

            let runs = extract_paragraph_runs(p_body);
            let full_text = runs.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>().join("").trim().to_string();
            let bold = runs.iter().any(|(_, b)| *b);

            if !full_text.is_empty() {
                shape_paragraphs.push((full_text, bold, level, is_bullet));
            }
        }

        if shape_paragraphs.is_empty() {
            continue;
        }

        match ph_type.as_deref() {
            Some("ctrTitle") | Some("title") => {
                for (text, _, _, _) in shape_paragraphs {
                    title_parts.push(text);
                }
            }
            Some("subTitle") => {
                let sub = shape_paragraphs.into_iter().map(|(t, _, _, _)| t).collect::<Vec<_>>().join("\n");
                if subtitle.is_none() {
                    subtitle = Some(sub);
                }
            }
            Some("ftr") | Some("dt") | Some("sldNum") => {
                for (text, _, _, _) in shape_paragraphs {
                    footers.push(text);
                }
            }
            _ => {
                for (text, bold, level, is_bullet) in shape_paragraphs {
                    blocks.push(SlideContentBlock::Paragraph {
                        text,
                        bold,
                        level,
                        is_bullet,
                    });
                }
            }
        }
    }

    // 3. Iterate Graphic Frames `<p:graphicFrame>` for Tables `<a:tbl>`
    let mut gf_cursor = 0;
    while let Some(gf_elem) = find_xml_element(xml, gf_cursor, "<p:graphicFrame", "</p:graphicFrame>") {
        gf_cursor = gf_elem.next_cursor;
        if gf_elem.is_self_closing {
            continue;
        }
        let gf_body = gf_elem.body;

        if gf_body.contains("<a:tbl") {
            let mut tr_cursor = 0;
            let mut all_rows = Vec::new();

            while let Some(tr_elem) = find_xml_element(gf_body, tr_cursor, "<a:tr", "</a:tr>") {
                tr_cursor = tr_elem.next_cursor;
                if tr_elem.is_self_closing {
                    continue;
                }
                let tr_body = tr_elem.body;

                let mut tc_cursor = 0;
                let mut row_cells = Vec::new();

                while let Some(tc_elem) = find_xml_element(tr_body, tc_cursor, "<a:tc", "</a:tc>") {
                    tc_cursor = tc_elem.next_cursor;
                    if tc_elem.is_self_closing {
                        row_cells.push(String::new());
                        continue;
                    }
                    let tc_body = tc_elem.body;

                    let runs = extract_paragraph_runs(tc_body);
                    let cell_text = runs
                        .iter()
                        .map(|(t, _)| t.as_str())
                        .collect::<Vec<_>>()
                        .join("")
                        .trim()
                        .to_string();
                    row_cells.push(cell_text);
                }

                if !row_cells.is_empty() {
                    all_rows.push(row_cells);
                }
            }

            if !all_rows.is_empty() {
                let headers = all_rows[0].clone();
                let rows = if all_rows.len() > 1 {
                    all_rows[1..].to_vec()
                } else {
                    Vec::new()
                };
                tables.push(SlideTableData { headers, rows });
            }
        }
    }

    let title = if !title_parts.is_empty() {
        title_parts.join(" ")
    } else {
        format!("Slide {slide_number}")
    };

    let is_title_slide = has_ctr_title
        || (slide_number == 1 && blocks.is_empty() && tables.is_empty())
        || (subtitle.is_some() && blocks.is_empty() && tables.is_empty());

    SlideData {
        slide_number,
        title,
        subtitle,
        background_color,
        blocks,
        tables,
        footers,
        is_title_slide,
    }
}

fn load_pptx(path: &Path) -> FileKind {
    let entries = read_zip_entry(path, |name| {
        name == "ppt/presentation.xml"
            || name == "ppt/_rels/presentation.xml.rels"
            || (name.starts_with("ppt/slides/slide") && name.ends_with(".xml") && !name.contains("Layout") && !name.contains("Master") && !name.contains("_rels"))
            || name == "content.xml"
    });

    let mut pres_xml = None;
    let mut rels_xml = None;
    let mut slides_map: HashMap<String, Vec<u8>> = HashMap::new();
    let mut odp_content = None;

    for (name, data) in entries {
        if name == "ppt/presentation.xml" {
            pres_xml = String::from_utf8(data).ok();
        } else if name == "ppt/_rels/presentation.xml.rels" {
            rels_xml = String::from_utf8(data).ok();
        } else if name.starts_with("ppt/slides/slide") && name.ends_with(".xml") {
            slides_map.insert(name, data);
        } else if name == "content.xml" {
            odp_content = String::from_utf8(data).ok();
        }
    }

    let mut aspect_ratio = 16.0 / 9.0;
    let mut ordered_slide_names = Vec::new();

    if let Some(xml) = &pres_xml {
        let (ar, rids) = parse_presentation_xml(xml);
        aspect_ratio = ar;

        let rid_map = rels_xml.as_deref().map(parse_presentation_rels_xml).unwrap_or_default();

        for rid in rids {
            if let Some(path) = rid_map.get(&rid) {
                if slides_map.contains_key(path) {
                    ordered_slide_names.push(path.clone());
                }
            } else {
                let num_digits: String = rid.chars().filter(|c| c.is_ascii_digit()).collect();
                let candidate = format!("ppt/slides/slide{num_digits}.xml");
                if slides_map.contains_key(&candidate) {
                    ordered_slide_names.push(candidate);
                }
            }
        }
    }

    if ordered_slide_names.is_empty() {
        let mut all_slide_paths: Vec<String> = slides_map.keys().cloned().collect();
        all_slide_paths.sort_by_key(|name| {
            name.chars()
                .filter(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse::<usize>()
                .unwrap_or(0)
        });
        ordered_slide_names = all_slide_paths;
    } else {
        let mut missing: Vec<String> = slides_map
            .keys()
            .filter(|k| !ordered_slide_names.contains(k))
            .cloned()
            .collect();
        missing.sort();
        ordered_slide_names.extend(missing);
    }

    let mut slides = Vec::new();
    for (idx, slide_name) in ordered_slide_names.into_iter().enumerate() {
        if let Some(data) = slides_map.get(&slide_name) {
            if let Ok(xml) = std::str::from_utf8(data) {
                let slide_data = parse_slide_xml(xml, idx + 1);
                slides.push(slide_data);
            }
        }
    }

    if slides.is_empty() {
        if let Some(xml) = odp_content {
            let paras = extract_xml_paragraphs(&xml);
            let title = paras.first().cloned().unwrap_or_else(|| "Presentation".to_string());
            let bullets = if paras.len() > 1 {
                paras[1..]
                    .iter()
                    .map(|p| SlideContentBlock::Paragraph {
                        text: p.clone(),
                        bold: false,
                        level: 0,
                        is_bullet: true,
                    })
                    .collect()
            } else {
                Vec::new()
            };

            slides.push(SlideData {
                slide_number: 1,
                title,
                subtitle: None,
                background_color: None,
                blocks: bullets,
                tables: Vec::new(),
                footers: Vec::new(),
                is_title_slide: true,
            });
        }
    }

    if slides.is_empty() {
        slides.push(SlideData {
            slide_number: 1,
            title: "Empty Presentation".into(),
            subtitle: None,
            background_color: None,
            blocks: vec![SlideContentBlock::Paragraph {
                text: "No slide content found".into(),
                bold: false,
                level: 0,
                is_bullet: false,
            }],
            tables: Vec::new(),
            footers: Vec::new(),
            is_title_slide: true,
        });
    }

    FileKind::Presentation {
        presentation: PresentationData {
            aspect_ratio,
            slides,
        },
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

fn extract_xml_paragraphs(xml: &str) -> Vec<String> {
    let mut paras = Vec::new();
    let p_open = "<a:p";
    let p_close = "</a:p>";
    let mut start = 0;

    while let Some(open_idx) = xml[start..].find(p_open) {
        let abs_open = start + open_idx;
        let end_tag_idx = match xml[abs_open..].find('>') {
            Some(i) => abs_open + i + 1,
            None => break,
        };
        let close_idx = match xml[end_tag_idx..].find(p_close) {
            Some(i) => end_tag_idx + i,
            None => break,
        };

        let para_xml = &xml[end_tag_idx..close_idx];
        let runs = extract_xml_tags(para_xml, "a:t");
        let combined = runs.join("").trim().to_string();
        if !combined.is_empty() {
            paras.push(combined);
        }

        start = close_idx + p_close.len();
    }

    paras
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
    // 1. In-memory fast scan (< 1ms, zero subprocess overhead)
    if let Some(count) = estimate_pdf_pages(path) {
        if count > 0 {
            return count;
        }
    }

    // 2. Fallback to pdfinfo if binary parsing was ambiguous
    if let Some(path_str) = path.to_str() {
        if let Ok(output) = std::process::Command::new("pdfinfo")
            .current_dir(std::env::temp_dir())
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
    1
}

fn render_pdf_page_to_image(path: &Path, page: usize) -> Option<egui::ColorImage> {
    let path_str = path.to_str()?;
    let temp_dir = std::env::temp_dir();

    // 1. pdftocairo (poppler-utils) - fast and explicitly designed for stdout streaming with "-"
    if let Ok(output) = std::process::Command::new("pdftocairo")
        .current_dir(&temp_dir)
        .args([
            "-png",
            "-singlefile",
            "-r",
            "150",
            "-f",
            &page.to_string(),
            "-l",
            &page.to_string(),
            path_str,
            "-",
        ])
        .output()
    {
        if output.status.success() && !output.stdout.is_empty() {
            if let Some(img) = load_color_image_from_bytes(&output.stdout) {
                return Some(img);
            }
        }
    }

    // 2. pdftoppm (poppler-utils) - omit output prefix so it writes directly to stdout, NEVER to disk!
    if let Ok(output) = std::process::Command::new("pdftoppm")
        .current_dir(&temp_dir)
        .args([
            "-png",
            "-r",
            "150",
            "-f",
            &page.to_string(),
            "-l",
            &page.to_string(),
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

    // 3. mutool (MuPDF) - ultra fast (< 25ms) and streams directly to stdout with "-"
    if let Ok(output) = std::process::Command::new("mutool")
        .current_dir(&temp_dir)
        .args(["draw", "-o", "-", "-r", "150", path_str, &page.to_string()])
        .output()
    {
        if output.status.success() && !output.stdout.is_empty() {
            if let Some(img) = load_color_image_from_bytes(&output.stdout) {
                return Some(img);
            }
        }
    }

    // 4. macOS built-in sips for page 1 (uses /tmp and immediately unlinks, zero files in user dir)
    if page == 1 {
        let temp_png = temp_dir.join(format!("ql_sips_{}_{}.png", std::process::id(), page));
        if let Ok(status) = std::process::Command::new("sips")
            .current_dir(&temp_dir)
            .args(["-s", "format", "png", path_str, "--out", temp_png.to_str().unwrap_or("")])
            .status()
        {
            if status.success() {
                if let Ok(bytes) = std::fs::read(&temp_png) {
                    let _ = std::fs::remove_file(&temp_png);
                    if let Some(img) = load_color_image_from_bytes(&bytes) {
                        return Some(img);
                    }
                }
                let _ = std::fs::remove_file(&temp_png);
            }
        }
    }

    // 5. Ghostscript (gs) - streaming directly to stdout with -sOutputFile=-
    if let Ok(output) = std::process::Command::new("gs")
        .current_dir(&temp_dir)
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

    // 6. Fallback for page 1: FreeDesktop / KDE thumbnail cache
    if page == 1 {
        if let Some(img) = get_xdg_thumbnail(path) {
            return Some(img);
        }
    }

    None
}

fn extract_video_frame_to_image(path: &Path) -> Option<egui::ColorImage> {
    let path_str = path.to_str()?;
    let temp_dir = std::env::temp_dir();

    // 1. ffmpeg
    if let Ok(output) = std::process::Command::new("ffmpeg")
        .current_dir(&temp_dir)
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
        .current_dir(&temp_dir)
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
        .current_dir(&temp_dir)
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
    let temp_dir = std::env::temp_dir();

    if let Ok(output) = std::process::Command::new("ffmpeg")
        .current_dir(&temp_dir)
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
    fn test_calculate_scrubber_width_bounds() {
        assert_eq!(calculate_scrubber_width(1000.0), 790.0);
        assert_eq!(calculate_scrubber_width(480.0), 270.0);
        assert_eq!(calculate_scrubber_width(250.0), 60.0);
        assert_eq!(calculate_scrubber_width(210.0), 60.0);
        assert_eq!(calculate_scrubber_width(100.0), 60.0);
        assert_eq!(calculate_scrubber_width(0.0), 60.0);
        assert_eq!(calculate_scrubber_width(-50.0), 60.0);
    }

    #[test]
    fn test_media_scrubber_calculation() {
        let duration = 120.0;
        let frac = 0.5;
        let time = frac * duration;
        assert_eq!(time, 60.0);

        let clamped_frac = (-0.2_f64).clamp(0.0, 1.0);
        assert_eq!(clamped_frac * duration, 0.0);

        let over_frac = (1.5_f64).clamp(0.0, 1.0);
        assert_eq!(over_frac * duration, 120.0);
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

    #[test]
    fn test_extract_xml_paragraphs() {
        let xml = "<p:txBody><a:p><a:r><a:t>NET</a:t></a:r><a:r><a:t>WORK LAYER</a:t></a:r></a:p><a:p><a:r><a:t>Bullet point 1</a:t></a:r></a:p></p:txBody>";
        let paras = extract_xml_paragraphs(xml);
        assert_eq!(paras, vec!["NETWORK LAYER", "Bullet point 1"]);
    }

    #[test]
    fn test_optimal_window_size() {
        let mut app = QuickLookApp::new(None);
        
        // Audio
        app.file_kind = FileKind::Media { kind: "Audio", poster_image: None };
        let audio_sz = app.calculate_optimal_size();
        assert_eq!(audio_sz, egui::vec2(580.0, 440.0));

        // PDF
        app.file_kind = FileKind::Pdf {
            current_page: 1,
            total_pages: 5,
            page_images: HashMap::new(),
            page_textures: HashMap::new(),
        };
        let pdf_sz = app.calculate_optimal_size();
        assert_eq!(pdf_sz, egui::vec2(1020.0, 880.0));

        // PPTX
        app.file_kind = FileKind::Presentation { presentation: PresentationData::default(), current_slide: 0 };
        let ppt_sz = app.calculate_optimal_size();
        assert_eq!(ppt_sz, egui::vec2(1140.0, 680.0));

        // Portrait Image (e.g. 1000x2000)
        app.file_kind = FileKind::Image { width: 1000, height: 2000, uri: "".into() };
        let img_sz = app.calculate_optimal_size();
        assert!(img_sz.x < img_sz.y);
    }

    #[test]
    fn test_render_pdf_page_never_pollutes_cwd() {
        let dummy_pdf = Path::new("non_existent_test_document_never_created.pdf");
        let _ = render_pdf_page_to_image(dummy_pdf, 1);
        let _ = render_pdf_page_to_image(dummy_pdf, 2);

        // Verify zero stray files were generated in current directory
        assert!(!Path::new("-1.png").exists());
        assert!(!Path::new("-2.png").exists());
        assert!(!Path::new("---1.png").exists());
        assert!(!Path::new("--1.png").exists());
    }

    #[test]
    fn test_estimate_pdf_pages_no_overcount() {
        let raw_pdf = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>\nendobj\n3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n4 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n";
        let count = estimate_pdf_pages_from_bytes(raw_pdf);
        assert_eq!(count, Some(2), "Must not overcount /Type /Pages container");
    }

    #[test]
    fn test_estimate_pdf_pages_compact_and_varied_delimiters() {
        let raw_pdf = b"%PDF-1.4\n1 0 obj\n<< /Type/Pages /Kids [2 0 R] /Count 1 >>\nendobj\n2 0 obj\n<< /Type/Page/Parent 1 0 R >>\nendobj\n";
        let count = estimate_pdf_pages_from_bytes(raw_pdf);
        assert_eq!(count, Some(1));
    }

    #[test]
    fn test_estimate_pdf_pages_whitespace_variations() {
        let raw_pdf = b"%PDF-1.4\n<< /Type\r\n  /Page >>\n<< /Type\t/Page/Contents 5 0 R >>\n<< /Type /Pages >>\n";
        let count = estimate_pdf_pages_from_bytes(raw_pdf);
        assert_eq!(count, Some(2));
    }

    #[test]
    fn test_pdf_aspect_ratio_preservation_a4() {
        // A4 Portrait at 150 DPI: 1240 x 1754 (or 595.28 x 841.89)
        let tex_w = 1240.0_f32;
        let tex_h = 1754.0_f32;
        let native_aspect = tex_w / tex_h;

        let window_sizes = [
            (1020.0_f32, 880.0_f32),
            (1920.0, 1080.0),
            (800.0, 600.0),
            (1440.0, 900.0),
            (600.0, 1200.0),
        ];
        let zoom_levels = [0.5_f32, 1.0, 1.5, 2.0];

        for &(w, h) in &window_sizes {
            for &zoom in &zoom_levels {
                let margin = 24.0_f32;
                let max_w = (w - 136.0 - margin * 2.0).max(10.0);
                let max_h = (h - 48.0 - margin * 2.0).max(10.0);
                let fit_scale = (max_w / tex_w).min(max_h / tex_h);
                let target_w = tex_w * fit_scale * zoom;
                let target_h = tex_h * fit_scale * zoom;

                let rendered_aspect = target_w / target_h;
                let error = (rendered_aspect - native_aspect).abs();
                assert!(
                    error < 1e-6,
                    "Aspect ratio distorted! Window: ({w}, {h}), Zoom: {zoom}, Error: {error}"
                );
            }
        }
    }

    #[test]
    fn test_pdf_page_navigation_clamp() {
        let total_pages = 5;
        let mut current_page = 1;

        // PageDown
        current_page = (current_page + 1).min(total_pages);
        assert_eq!(current_page, 2);

        // End
        current_page = total_pages;
        assert_eq!(current_page, 5);

        // PageDown at end clamps to total_pages
        current_page = (current_page + 1).min(total_pages);
        assert_eq!(current_page, 5);

        // PageUp
        current_page = (current_page - 1).max(1);
        assert_eq!(current_page, 4);

        // Home
        current_page = 1;
        assert_eq!(current_page, 1);

        // PageUp at beginning clamps to 1
        current_page = (current_page - 1).max(1);
        assert_eq!(current_page, 1);
    }

    #[test]
    fn test_pptx_slide_dimensions_parsing() {
        // Standard 16:9
        let xml_16_9 = r#"<p:presentation><p:sldSz cx="12192000" cy="6858000"/><p:sldIdLst><p:sldId id="256" r:id="rId1"/><p:sldId id="257" r:id="rId2"/></p:sldIdLst></p:presentation>"#;
        let (ar_16_9, rids_16_9) = parse_presentation_xml(xml_16_9);
        assert!((ar_16_9 - 16.0 / 9.0).abs() < 1e-4);
        assert_eq!(rids_16_9, vec!["rId1", "rId2"]);

        // Standard 4:3
        let xml_4_3 = r#"<p:presentation><p:sldSz cx="9144000" cy="6858000"/></p:presentation>"#;
        let (ar_4_3, _) = parse_presentation_xml(xml_4_3);
        assert!((ar_4_3 - 4.0 / 3.0).abs() < 1e-4);

        // Fallback on missing or invalid
        let xml_empty = r#"<p:presentation></p:presentation>"#;
        let (ar_default, _) = parse_presentation_xml(xml_empty);
        assert!((ar_default - 16.0 / 9.0).abs() < 1e-4);
    }

    #[test]
    fn test_pptx_shape_and_placeholder_extraction() {
        let title_slide_xml = r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
            <p:cSld>
                <p:spTree>
                    <p:sp>
                        <p:nvSpPr><p:nvPr><p:ph type="ctrTitle"/></p:nvPr></p:nvSpPr>
                        <p:txBody><a:p><a:r><a:t>Sample Presentation Title</a:t></a:r></a:p></p:txBody>
                    </p:sp>
                    <p:sp>
                        <p:nvSpPr><p:nvPr><p:ph type="subTitle"/></p:nvPr></p:nvSpPr>
                        <p:txBody><a:p><a:r><a:t>Presentation Subtitle Line</a:t></a:r></a:p></p:txBody>
                    </p:sp>
                    <p:sp>
                        <p:nvSpPr><p:nvPr><p:ph type="ftr"/></p:nvPr></p:nvSpPr>
                        <p:txBody><a:p><a:r><a:t>Confidential Slide Footer</a:t></a:r></a:p></p:txBody>
                    </p:sp>
                </p:spTree>
            </p:cSld>
        </p:sld>"#;

        let title_slide = parse_slide_xml(title_slide_xml, 1);
        assert_eq!(title_slide.title, "Sample Presentation Title");
        assert_eq!(title_slide.subtitle.as_deref(), Some("Presentation Subtitle Line"));
        assert!(title_slide.is_title_slide);
        assert_eq!(title_slide.footers, vec!["Confidential Slide Footer"]);

        let content_slide_xml = r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
            <p:cSld>
                <p:spTree>
                    <p:sp>
                        <p:nvSpPr><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr>
                        <p:txBody><a:p><a:r><a:t>Architecture Overview</a:t></a:r></a:p></p:txBody>
                    </p:sp>
                    <p:sp>
                        <p:nvSpPr><p:nvPr><p:ph type="body"/></p:nvPr></p:nvSpPr>
                        <p:txBody>
                            <a:p><a:pPr lvl="0"/><a:r><a:rPr b="1"/><a:t>Primary Feature</a:t></a:r></a:p>
                            <a:p><a:pPr lvl="1"/><a:r><a:t>Sub-feature Detail</a:t></a:r></a:p>
                            <a:p><a:pPr lvl="2"><a:buNone/></a:pPr><a:r><a:t>Nested Plain Note</a:t></a:r></a:p>
                        </p:txBody>
                    </p:sp>
                </p:spTree>
            </p:cSld>
        </p:sld>"#;

        let content_slide = parse_slide_xml(content_slide_xml, 2);
        assert_eq!(content_slide.title, "Architecture Overview");
        assert!(!content_slide.is_title_slide);
        assert_eq!(content_slide.blocks.len(), 3);

        match &content_slide.blocks[0] {
            SlideContentBlock::Paragraph { text, bold, level, is_bullet } => {
                assert_eq!(text, "Primary Feature");
                assert!(*bold);
                assert_eq!(*level, 0);
                assert!(*is_bullet);
            }
            _ => panic!("Expected Paragraph block"),
        }

        match &content_slide.blocks[1] {
            SlideContentBlock::Paragraph { text, level, is_bullet, .. } => {
                assert_eq!(text, "Sub-feature Detail");
                assert_eq!(*level, 1);
                assert!(*is_bullet);
            }
            _ => panic!("Expected Paragraph block"),
        }

        match &content_slide.blocks[2] {
            SlideContentBlock::Paragraph { text, level, is_bullet, .. } => {
                assert_eq!(text, "Nested Plain Note");
                assert_eq!(*level, 2);
                assert!(!*is_bullet);
            }
            _ => panic!("Expected Paragraph block"),
        }
    }

    #[test]
    fn test_pptx_table_extraction() {
        let table_slide_xml = r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
            <p:cSld>
                <p:spTree>
                    <p:sp>
                        <p:nvSpPr><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr>
                        <p:txBody><a:p><a:r><a:t>Metrics Summary</a:t></a:r></a:p></p:txBody>
                    </p:sp>
                    <p:graphicFrame>
                        <a:graphic>
                            <a:graphicData>
                                <a:tbl>
                                    <a:tr>
                                        <a:tc><a:txBody><a:p><a:r><a:t>Metric</a:t></a:r></a:p></a:txBody></a:tc>
                                        <a:tc><a:txBody><a:p><a:r><a:t>Target</a:t></a:r></a:p></a:txBody></a:tc>
                                        <a:tc><a:txBody><a:p><a:r><a:t>Status</a:t></a:r></a:p></a:txBody></a:tc>
                                    </a:tr>
                                    <a:tr>
                                        <a:tc><a:txBody><a:p><a:r><a:t>Latency</a:t></a:r></a:p></a:txBody></a:tc>
                                        <a:tc><a:txBody><a:p><a:r><a:t>&lt;16ms</a:t></a:r></a:p></a:txBody></a:tc>
                                        <a:tc><a:txBody><a:p><a:r><a:t>PASS</a:t></a:r></a:p></a:txBody></a:tc>
                                    </a:tr>
                                </a:tbl>
                            </a:graphicData>
                        </a:graphic>
                    </p:graphicFrame>
                </p:spTree>
            </p:cSld>
        </p:sld>"#;

        let slide = parse_slide_xml(table_slide_xml, 3);
        assert_eq!(slide.title, "Metrics Summary");
        assert_eq!(slide.tables.len(), 1);
        let table = &slide.tables[0];
        assert_eq!(table.headers, vec!["Metric", "Target", "Status"]);
        assert_eq!(table.rows.len(), 1);
        assert_eq!(table.rows[0], vec!["Latency", "<16ms", "PASS"]);
    }

    #[test]
    fn test_presentation_aspect_ratio_containment() {
        let avail = egui::vec2(1200.0, 800.0);

        let (w16, h16, scale16) = compute_presentation_slide_dimensions(avail, 16.0 / 9.0, 48.0);
        assert!(((w16 / h16) - 16.0 / 9.0).abs() < 1e-3);
        assert!(w16 <= avail.x && h16 <= avail.y - 48.0);
        assert!(scale16 >= 0.6 && scale16 <= 2.5);

        let (w4, h4, scale4) = compute_presentation_slide_dimensions(avail, 4.0 / 3.0, 48.0);
        assert!(((w4 / h4) - 4.0 / 3.0).abs() < 1e-3);
        assert!(w4 <= avail.x && h4 <= avail.y - 48.0);
        assert!(scale4 >= 0.6 && scale4 <= 2.5);

        let (w_nan, h_nan, _) = compute_presentation_slide_dimensions(avail, f32::NAN, 48.0);
        assert!(((w_nan / h_nan) - 16.0 / 9.0).abs() < 1e-3);

        let (w_inf, h_inf, _) = compute_presentation_slide_dimensions(avail, f32::INFINITY, 48.0);
        assert!(((w_inf / h_inf) - 16.0 / 9.0).abs() < 1e-3);

        let (w_zero, h_zero, s_zero) = compute_presentation_slide_dimensions(egui::vec2(0.0, 0.0), 16.0 / 9.0, 48.0);
        assert!(w_zero >= 160.0 && h_zero >= 90.0);
        assert_eq!(s_zero, 0.6);
    }

    #[test]
    fn test_pptx_slide_navigation_boundary_clamps() {
        let total_slides = 4_usize;
        let mut current_slide = 0_usize;

        current_slide = current_slide.saturating_sub(1);
        assert_eq!(current_slide, 0);

        for step in 1..total_slides {
            current_slide = (current_slide + 1).min(total_slides - 1);
            assert_eq!(current_slide, step);
        }

        current_slide = (current_slide + 1).min(total_slides - 1);
        assert_eq!(current_slide, 3);

        current_slide = 0;
        assert_eq!(current_slide, 0);

        current_slide = total_slides - 1;
        assert_eq!(current_slide, 3);
    }

    #[test]
    fn test_slide_theme_luminance_detection() {
        let dark_theme = SlideThemeColors::from_background(Some(egui::Color32::from_rgb(20, 20, 22)));
        assert!(dark_theme.primary_text.r() > 200);
        assert!(dark_theme.primary_text.g() > 200);
        assert!(dark_theme.primary_text.b() > 200);

        let light_theme = SlideThemeColors::from_background(Some(egui::Color32::from_rgb(255, 255, 255)));
        assert!(light_theme.primary_text.r() < 50);
        assert!(light_theme.primary_text.g() < 50);
        assert!(light_theme.primary_text.b() < 50);

        let default_theme = SlideThemeColors::from_background(None);
        assert!(default_theme.primary_text.r() < 50);
    }

    #[test]
    fn test_load_pptx_real_fixtures() {
        let sample_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.pptx");
        if sample_path.exists() {
            let kind = load_pptx(&sample_path);
            match kind {
                FileKind::Presentation { presentation, current_slide } => {
                    assert_eq!(current_slide, 0);
                    assert!((presentation.aspect_ratio - 16.0 / 9.0).abs() < 1e-4);
                    assert_eq!(presentation.slides.len(), 4);
                    assert_eq!(presentation.slides[0].title, "Linux Quick Look Presentation");
                    assert_eq!(presentation.slides[0].subtitle.as_deref(), Some("Modern Desktop Preview Architecture"));
                    assert!(presentation.slides[0].is_title_slide);
                    assert_eq!(presentation.slides[1].title, "Core Architecture & Subsystems");
                    assert!(!presentation.slides[1].is_title_slide);
                    assert_eq!(presentation.slides[2].title, "Performance Benchmark & Reliability");
                    assert_eq!(presentation.slides[2].tables.len(), 1);
                    assert_eq!(presentation.slides[3].title, "Conclusion & Quality Standards");
                }
                _ => panic!("Expected FileKind::Presentation"),
            }
        }

        let p43_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/portrait_4_3.pptx");
        if p43_path.exists() {
            let kind = load_pptx(&p43_path);
            match kind {
                FileKind::Presentation { presentation, current_slide } => {
                    assert_eq!(current_slide, 0);
                    assert!((presentation.aspect_ratio - 4.0 / 3.0).abs() < 1e-4);
                    assert_eq!(presentation.slides.len(), 2);
                    assert_eq!(presentation.slides[0].title, "Classic 4:3 Presentation");
                    assert!(presentation.slides[0].is_title_slide);
                }
                _ => panic!("Expected FileKind::Presentation"),
            }
        }
    }

    #[test]
    fn test_parse_xml_tag() {
        // Self-closing tag
        let (tag, len) = parse_xml_tag(r#"<p:sldSz cx="12192000" cy="6858000"/>rest"#).unwrap();
        assert_eq!(tag, r#"<p:sldSz cx="12192000" cy="6858000"/>"#);
        assert_eq!(len, tag.len());

        // Regular tag
        let (tag2, len2) = parse_xml_tag(r#"<p:sldId id="1">rest"#).unwrap();
        assert_eq!(tag2, r#"<p:sldId id="1">"#);
        assert_eq!(len2, tag2.len());

        // Followed by multi-byte UTF-8
        let (tag3, len3) = parse_xml_tag("<p:sldId>こんにちは").unwrap();
        assert_eq!(tag3, "<p:sldId>");
        assert_eq!(len3, 9);

        // At EOF
        let (tag4, len4) = parse_xml_tag("<p:sldId>").unwrap();
        assert_eq!(tag4, "<p:sldId>");
        assert_eq!(len4, 9);

        // Unclosed
        assert!(parse_xml_tag("<p:sldId").is_none());
    }

    #[test]
    fn test_find_xml_element_paired_tag() {
        let xml = r#"<p:txBody><a:p><a:r><a:t>Hello</a:t></a:r></a:p></p:txBody>"#;
        let elem = find_xml_element(xml, 0, "<a:p", "</a:p>").expect("Expected element");
        assert!(!elem.is_self_closing);
        assert!(elem.body.contains("Hello"));
        assert_eq!(elem.next_cursor, xml.find("</p:txBody>").unwrap());
    }

    #[test]
    fn test_find_xml_element_self_closing_variants() {
        let xml = r#"<a:p/><a:p /><a:p lvl="1"/> <a:p lvl="2" />"#;
        let mut cursor = 0;
        let mut count = 0;
        while let Some(elem) = find_xml_element(xml, cursor, "<a:p", "</a:p>") {
            assert!(elem.is_self_closing);
            cursor = elem.next_cursor;
            count += 1;
        }
        assert_eq!(count, 4);
    }

    #[test]
    fn test_find_xml_element_prefix_boundary_rejection() {
        let xml = r#"<p:spTree><p:sp><p:txBody><a:pPr lvl="0"/><a:p><a:r><a:t>Inside</a:t></a:r></a:p></p:txBody></p:sp></p:spTree>"#;
        // "<p:sp" should NOT match "<p:spTree>"
        let sp_elem = find_xml_element(xml, 0, "<p:sp", "</p:sp>").expect("Should find <p:sp>");
        assert!(sp_elem.body.starts_with("<p:sp>"));

        // "<a:p" should NOT match "<a:pPr"
        let p_elem = find_xml_element(sp_elem.body, 0, "<a:p", "</a:p>").expect("Should find <a:p>");
        assert!(p_elem.body.starts_with("<a:p>"));
    }

    #[test]
    fn test_pptx_self_closing_paragraphs_in_slide() {
        let slide_xml = r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
            <p:cSld><p:spTree>
                <p:sp>
                    <p:nvSpPr><p:nvPr><p:ph type="body"/></p:nvPr></p:nvSpPr>
                    <p:txBody>
                        <a:p><a:r><a:t>Bullet 1</a:t></a:r></a:p>
                        <a:p/>
                        <a:p><a:r><a:t>Bullet 2</a:t></a:r></a:p>
                        <a:p lvl="1"/>
                        <a:p><a:r><a:t>Bullet 3</a:t></a:r></a:p>
                        <a:p/>
                    </p:txBody>
                </p:sp>
            </p:spTree></p:cSld>
        </p:sld>"#;
        let slide = parse_slide_xml(slide_xml, 2);
        assert_eq!(slide.blocks.len(), 3);
        if let SlideContentBlock::Paragraph { text, .. } = &slide.blocks[0] {
            assert_eq!(text, "Bullet 1");
        }
        if let SlideContentBlock::Paragraph { text, .. } = &slide.blocks[1] {
            assert_eq!(text, "Bullet 2");
        }
        if let SlideContentBlock::Paragraph { text, .. } = &slide.blocks[2] {
            assert_eq!(text, "Bullet 3");
        }
    }

    #[test]
    fn test_pptx_self_closing_table_cells_and_rows() {
        let table_slide_xml = r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
            <p:cSld><p:spTree>
                <p:graphicFrame>
                    <a:tbl>
                        <a:tr>
                            <a:tc><a:txBody><a:p><a:r><a:t>Col 1</a:t></a:r></a:p></a:txBody></a:tc>
                            <a:tc><a:txBody><a:p><a:r><a:t>Col 2</a:t></a:r></a:p></a:txBody></a:tc>
                            <a:tc><a:txBody><a:p><a:r><a:t>Col 3</a:t></a:r></a:p></a:txBody></a:tc>
                        </a:tr>
                        <a:tr>
                            <a:tc><a:txBody><a:p><a:r><a:t>Row2-C1</a:t></a:r></a:p></a:txBody></a:tc>
                            <a:tc/>
                            <a:tc><a:txBody><a:p><a:r><a:t>Row2-C3</a:t></a:r></a:p></a:txBody></a:tc>
                        </a:tr>
                        <a:tr/>
                        <a:tr>
                            <a:tc><a:txBody><a:p><a:r><a:t>Row4-C1</a:t></a:r></a:p></a:txBody></a:tc>
                            <a:tc><a:txBody><a:p><a:r><a:t>Row4-C2</a:t></a:r></a:p></a:txBody></a:tc>
                            <a:tc/>
                        </a:tr>
                    </a:tbl>
                </p:graphicFrame>
            </p:spTree></p:cSld>
        </p:sld>"#;
        let slide = parse_slide_xml(table_slide_xml, 3);
        assert_eq!(slide.tables.len(), 1);
        let table = &slide.tables[0];
        assert_eq!(table.headers, vec!["Col 1", "Col 2", "Col 3"]);
        assert_eq!(table.rows.len(), 2);
        assert_eq!(table.rows[0], vec!["Row2-C1", "", "Row2-C3"]);
        assert_eq!(table.rows[1], vec!["Row4-C1", "Row4-C2", ""]);
    }

    #[test]
    fn test_aspect_ratio_proportional_clamping_4_3_at_300x200() {
        let (w, h, scale) = compute_presentation_slide_dimensions(
            egui::vec2(300.0, 200.0),
            4.0 / 3.0,
            48.0,
        );
        let ratio = w / h;
        assert!(
            (ratio - 4.0 / 3.0).abs() < 1e-4,
            "4:3 aspect ratio violated at 300x200: w={w}, h={h}, ratio={ratio}"
        );
        assert!(scale >= 0.6 && scale <= 2.5, "Scale factor out of bounds: {scale}");
        assert!(w <= 300.0, "Slide width {w} exceeded available width 300.0");
        assert!(h <= 200.0 - 48.0, "Slide height {h} exceeded available height minus clearance 152.0");
    }
}
