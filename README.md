> [!IMPORTANT]
> Remove this line to confirm you've reviewed this PR before submitting.

# PDF Reader for Green star

A minimal PDF reader for **Green star**, the custom Zed fork, on macOS Apple Silicon. Read documents in an editor tab with smooth scrolling, zoom, Fit width, and text selection. Pages are rendered at the display's resolution as they become visible.

Drag to select text, double-click a word, Shift-click to extend a selection, and press **Cmd+C** to copy. Selections can span pages and stay aligned when zooming. **Escape** clears the selection. There are no agent attachments, OCR, annotation editor or browser dependency. Image-only pages have no selectable text; encrypted PDFs are not supported.

## Install

Version **0.2.0** requires a Green star build that supports both `paged_document_v1` and `paged_text_v1`. Fully quit and reopen the updated editor before installing or updating this package. Older Green star builds cannot install 0.2.0; version 0.1.1 remains available for viewing without text selection.

1. Open the command palette and choose **Zed: Install Extension from GitHub**.
2. Enter `https://github.com/unteku3/green-star-pdf-reader`.
3. Click **Install Extension from GitHub**, review the release and choose **Install and enable**.
4. Open a local PDF file.

Use the mouse wheel or trackpad to scroll, pinch with two fingers or use **Cmd+Plus / Cmd+Minus** to zoom, and **Cmd+0** for Fit width. Trackpad pinch requires the updated Green star host; rebuild and restart the editor to enable it. Disable, enable or uninstall it in **Extensions**. If the older PDF Viewer is installed, disable it before using PDF Reader.

For an existing installation, use **Check for update** in Extensions and confirm the new version. Text is extracted asynchronously as needed. The host's text cache is limited to 8 MiB per view, in addition to the existing 128 MiB shared raster cache. Large cross-page copies fetch text sequentially and have a 16 MiB clipboard limit. These limits do not include PDFKit's internal caches, process overhead or GPU memory. PDFKit can normalize the document's original spacing and reading order.

This is a native extension for Green star, not an extension for the official Zed marketplace. Its executable runs with your user permissions. Downloads use HTTPS and SHA-256 verification. Release binaries are ad-hoc signed, not Apple-notarized.

## Build and verify

Rust and macOS Command Line Tools are required. The renderer uses the operating system's CoreGraphics and PDFKit frameworks; it does not download a rendering library. The build compiles a small Objective-C bridge with the system Clang toolchain.

```sh
cargo test --workspace --locked
cargo build --release --locked
python3 tests/ipc.py target/release/green-star-pdf-reader
python3 -m pip install fonttools
python3 tests/text.py target/release/green-star-pdf-reader
python3 scripts/package.py
```

Publish both files from `dist/` as GitHub Release assets: `viewer-aarch64-apple-darwin.zgv` and `viewer-aarch64-apple-darwin.zgv.sha256`. Their names are part of Green star's installer format.

The process owns one PDFKit document and uses its CoreGraphics document reference for rendering into bounded BGRA tiles. The host owns scrolling, selection, scheduling and caches. Opening the same file in multiple views shares its native document; closing the last view releases it. After 32 distinct text pages, the native document is reopened on demand to reduce accumulated PDFKit data. Allocator and system-framework memory may remain resident after closing a document. Tests generate their own PDFs; no personal documents belong in this repository.

See [PROTOCOL.md](PROTOCOL.md) for the interface and bounds. GPL-3.0-or-later; the vendored protocol comes from the Green star fork. See [LICENSE](LICENSE).
