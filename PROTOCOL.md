# Paged document protocol

This package uses Graphic Extension API v1 with additive `paged_document_v1` and `paged_text_v1` capabilities. `protocol/` is a copy of Green star's `crates/viewer_protocol`. Keep the copies synchronized when changing the interface. Older hosts must be updated before installing version 0.2.0. Updated hosts can still render packages advertising only `paged_document_v1` and must not request text from those packages.

stdin/stdout carry little-endian u32-length-prefixed frames, limited to 64 MiB each. Requests are JSON `{id, view, command}`. Responses echo `id`, contain an optional error and declare the number of following binary frames. stdout is exclusively protocol output; diagnostic messages use stderr.

- `Hello {api_version: 1}` must precede other commands.
- `OpenPagedDocument {path}` opens a local file and returns `document.pages`, an array of `{width, height}` in PDF points. Dimensions use CropBox after the page's built-in rotation. Page indices are zero-based. Password-protected/encrypted, unreadable, invalid and empty documents return errors.
- `RenderPageTile {tile}` accepts `{page, scale, x, y, width, height}`. Scale is physical pixels per PDF point multiplied by 1024. Coordinates are physical pixels from the rotated page's top-left. Full raster dimensions are `ceil(page_points * scale / 1024)`.
- Successful tile responses contain `tile: {request, blob: 0}` and one tightly packed premultiplied BGRA8 frame. It is `width * height * 4` bytes, top row first, on opaque white paper. The renderer clips to the requested tile rather than rasterizing the full page first.
- `ExtractPageText {page}` returns `page_text: {page, text, glyphs}` without binary frames. The string is UTF-8, derived from PDFKit's UTF-16 text. Each glyph is `{start, end, bounds: [x, y, width, height]}`: a Unicode scalar's byte range in `text` and its rectangle in rotated CropBox PDF points with a top-left origin. Surrogate pairs share one scalar record. Whitespace and combining marks can have zero-area rectangles. Empty text/glyphs is a successful result for an image-only page.
- Text responses are limited to 1 MiB of UTF-8 and 100,000 glyphs per page. Exceeding a limit returns an error without disabling raster rendering or other views. PDFKit may normalize original PDF spacing and text order; OCR is not performed.
- `Close` releases a view. The document is released after its last view closes. Other commands return an unsupported-command error.

Each tile dimension is between 1 and 1024 pixels, its rectangle must lie entirely inside the page raster, and scale is between 1 and 65536 (up to 64 physical pixels per point). Documents are limited to 100,000 pages and page dimensions to 1,000,000 points. These are input/allocation bounds, not a total memory limit for Apple's PDF engine.

Native handles are owned and used on a single process thread. PDFKit and CoreGraphics share the same underlying document. Temporary Objective-C selection objects are released during extraction; the native document is renewed after 32 distinct extracted pages, with file identity checks. The host alternates queued text and render requests outside its UI thread, keeps bounded page text and raster caches, and rejects obsolete selection/copy results. The extension maintains no separate raster or extracted-text cache. An ordinary document/request error preserves other open views. Invalid frame lengths or a truncated frame terminate the process; EOF between frames shuts it down normally.

The release package is two frames: UTF-8 JSON manifest followed by the ad-hoc-signed executable. The accompanying checksum is the SHA-256 of the complete package. Neither format contains installation scripts or archive paths.
