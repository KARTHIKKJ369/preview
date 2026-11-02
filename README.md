# Quick Look for Linux (Rust)

A fast, lightweight, macOS-inspired Quick Look preview application written in Rust.

## Reverse-Engineered macOS Preview / Quick Look Architecture

macOS Quick Look achieves its instant, zero-lag feel through four core design decisions:

1. **Read-Only Streaming Pipeline**: It avoids loading full editable document models. Large files are read in small chunks or memory-mapped so startup latency remains strictly sub-16ms.
2. **GPU Texture Blitting**: Rendering is delegated directly to hardware acceleration (OpenGL / Metal / Vulkan), maintaining 60/120 FPS during panning and zooming.
3. **Sibling Discovery**: Previewing a file automatically enumerates adjacent items in the folder, letting you press `Left` / `Right` or `Up` / `Down` to flick through files without window recreation or flicker.
4. **Universal Fallback**: It never throws an unhandled format error. Unrecognized files gracefully degrade to a formatted 3-column Hex Viewer (Offset, Hex Bytes, ASCII) and metadata inspector.

## Supported Formats

| Category | Formats | Viewer Capabilities |
| :--- | :--- | :--- |
| **Images** | PNG, JPG, JPEG, GIF, WEBP, BMP, SVG, ICO, TIFF | GPU-rendered, auto-fit, drag-to-pan, scroll-wheel zoom, dimensions (`W × H px`) |
| **Presentations** | `.pptx`, `.ppt`, `.odp` | Slide deck viewer: 16:9 canvas, slide titles, bullet points, slide count, Next/Prev slide navigation (`PageUp`/`PageDown`) |
| **Spreadsheets** | `.xlsx`, `.xls`, `.ods`, `.csv`, `.tsv` | Interactive spreadsheet grid: column headers (A, B, C...), row numbering, filter/search, cell table view |
| **Documents** | `.docx`, `.doc`, `.odt`, `.rtf` | Formatted document viewer: headings, paragraphs, word & paragraph count |
| **Code & Text** | `.rs`, `.py`, `.js`, `.ts`, `.c`, `.cpp`, `.go`, `.java`, `.sh`, `.json`, `.toml`, `.yaml`, `.html`, `.css`, etc. | Syntax-highlighted (Syntect), line numbers, monospace font, 512KB streaming limit to prevent UI lockup |
| **Markdown** | `.md`, `.markdown` | Formatted headers, blockquotes, code blocks, bullet points |
| **Directories** | Any directory / folder | File count, folder contents table with file icons, sizes, and relative modification timestamps |
| **Archives** | `.zip`, `.jar`, `.war` | Direct header inspection listing archive contents without extracting |
| **PDF** | `.pdf` | Page estimate count, document metadata, one-click open |
| **Media** | Audio (`mp3`, `wav`, `flac`, `ogg`) & Video (`mp4`, `mkv`, `webm`, `mov`) | Media badge, file size, format details, external launcher |
| **Binary Fallback** | Any unknown binary (`.bin`, `.so`, `.dat`, executables) | 3-column macOS Quick Look-style Hex Viewer (`Offset`, `Hex`, `ASCII`) |

## Keyboard & Mouse Shortcuts

- `Space` / `Esc` / `q` : Close window immediately
- `◀` / `Left Arrow` / `Up Arrow` / `k` : Previous file in folder
- `▶` / `Right Arrow` / `Down Arrow` / `j` : Next file in folder
- `+` / `-` / `0` : Zoom in, zoom out, reset zoom (100%)
- `Enter` / `o` : Open file in default desktop application (`xdg-open`)
- `Mouse Drag` : Pan image
- `Scroll Wheel` : Zoom image or scroll document

## Building & Installation

### Requirements
- Rust & Cargo (`rustc >= 1.75`)

### Install
```bash
./install.sh
```
Or manually:
```bash
cargo build --release
cp target/release/quicklook ~/.local/bin/
```

### Linux Desktop Integration (Out-of-the-Box)

Running `./install.sh` automatically configures your desktop environment:

#### KDE Plasma & Dolphin
- **Zero manual configuration needed**:
  - Automatically installs Dolphin KIO service menus (`~/.local/share/kio/servicemenus/quicklook.desktop`).
  - Sets executable permissions (`chmod +x`) required by KDE Plasma 6 security policies.
  - Automatically writes the **Space** key binding directly into Dolphin's UI profile (`dolphinui.rc`).
  - Refreshes KDE Sycoca service database (`kbuildsycoca6` / `kbuildsycoca5`).
- Selecting any file in Dolphin and pressing **Space** immediately opens the preview.
- Right-clicking any file also shows **Quick Look** in the context menu.

#### GNOME (Nautilus)
- Automatically installed to `~/.local/share/nautilus/scripts/Quick Look`.
- Right-click any file -> Scripts -> Quick Look, or bind a custom shortcut to `quicklook`.

#### Cinnamon (Nemo), MATE (Caja), XFCE (Thunar)
- Pre-configured actions installed to `~/.local/share/nemo/actions/` and Caja scripts folder.

