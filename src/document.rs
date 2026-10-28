use anyhow::{Context as _, Result, ensure};
use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    os::unix::ffi::OsStrExt,
    path::Path,
    ptr::NonNull,
};
use viewer_protocol::{
    DocumentMetadata, MAX_PAGE_TEXT_BYTES, MAX_PAGE_TEXT_GLYPHS, PageSize, PageText,
    RASTER_SCALE_DENOMINATOR, TILE_SIZE, TextGlyph, TileRequest,
};

#[cfg(not(target_os = "macos"))]
compile_error!("PDF Reader requires macOS CoreGraphics");

const CROP_BOX: c_int = 1;
const MAX_PAGES: usize = 100_000;
const MAX_PAGE_POINTS: f64 = 1_000_000.0;
const MAX_SCALE: u32 = 64 * RASTER_SCALE_DENOMINATOR;
const BGRA_PREMULTIPLIED: u32 = 0x2002;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Point {
    x: f64,
    y: f64,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Size {
    width: f64,
    height: f64,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Rect {
    origin: Point,
    size: Size,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Affine {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    tx: f64,
    ty: f64,
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPDFDocumentIsEncrypted(document: *mut c_void) -> bool;
    fn CGPDFDocumentGetNumberOfPages(document: *mut c_void) -> usize;
    fn CGPDFDocumentGetPage(document: *mut c_void, number: usize) -> *mut c_void;
    fn CGPDFPageGetBoxRect(page: *mut c_void, box_kind: c_int) -> Rect;
    fn CGPDFPageGetRotationAngle(page: *mut c_void) -> c_int;
    fn CGPDFPageGetDrawingTransform(
        page: *mut c_void,
        box_kind: c_int,
        rect: Rect,
        rotation: c_int,
        preserve_aspect: bool,
    ) -> Affine;
    fn CGColorSpaceCreateDeviceRGB() -> *mut c_void;
    fn CGColorSpaceRelease(color_space: *mut c_void);
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits: usize,
        row_bytes: usize,
        color_space: *mut c_void,
        info: u32,
    ) -> *mut c_void;
    fn CGContextRelease(context: *mut c_void);
    fn CGContextSetRGBFillColor(context: *mut c_void, red: f64, green: f64, blue: f64, alpha: f64);
    fn CGContextFillRect(context: *mut c_void, rect: Rect);
    fn CGContextTranslateCTM(context: *mut c_void, x: f64, y: f64);
    fn CGContextScaleCTM(context: *mut c_void, x: f64, y: f64);
    fn CGContextConcatCTM(context: *mut c_void, transform: Affine);
    fn CGContextSetInterpolationQuality(context: *mut c_void, quality: c_int);
    fn CGContextSetShouldAntialias(context: *mut c_void, enabled: bool);
    fn CGContextDrawPDFPage(context: *mut c_void, page: *mut c_void);
}

unsafe extern "C" {
    fn gs_pdf_open(path: *const c_char, error: *mut c_char, capacity: usize) -> *mut c_void;
    fn gs_pdf_close(owner: *mut c_void);
    fn gs_pdf_document(owner: *mut c_void) -> *mut c_void;
    fn gs_pdf_extract(
        owner: *mut c_void,
        page: u32,
        text: *mut NativeText,
        error: *mut c_char,
        capacity: usize,
    ) -> c_int;
    fn gs_pdf_free_text(text: *mut NativeText);
}

#[repr(C)]
struct NativeText {
    characters: *mut u16,
    bounds: *mut Rect,
    length: usize,
}
impl Default for NativeText {
    fn default() -> Self {
        Self {
            characters: std::ptr::null_mut(),
            bounds: std::ptr::null_mut(),
            length: 0,
        }
    }
}
impl Drop for NativeText {
    fn drop(&mut self) {
        unsafe { gs_pdf_free_text(self) };
    }
}

struct NativeDocument(NonNull<c_void>);
impl NativeDocument {
    fn document(&self) -> Result<NonNull<c_void>> {
        NonNull::new(unsafe { gs_pdf_document(self.0.as_ptr()) })
            .context("PDF native document is unavailable")
    }
}
impl Drop for NativeDocument {
    fn drop(&mut self) {
        unsafe { gs_pdf_close(self.0.as_ptr()) }
    }
}
struct ColorSpace(NonNull<c_void>);
impl Drop for ColorSpace {
    fn drop(&mut self) {
        unsafe { CGColorSpaceRelease(self.0.as_ptr()) }
    }
}
struct BitmapContext(NonNull<c_void>);
impl Drop for BitmapContext {
    fn drop(&mut self) {
        unsafe { CGContextRelease(self.0.as_ptr()) }
    }
}

pub struct Document {
    native: NativeDocument,
    metadata: DocumentMetadata,
}

impl Document {
    pub fn open(path: &Path) -> Result<Self> {
        let path = CString::new(path.as_os_str().as_bytes()).context("Invalid PDF path")?;
        let mut error = [0 as c_char; 1024];
        let native = NativeDocument(
            NonNull::new(unsafe { gs_pdf_open(path.as_ptr(), error.as_mut_ptr(), error.len()) })
                .ok_or_else(|| native_error(&error))?,
        );
        let document = native.document()?;
        ensure!(
            !unsafe { CGPDFDocumentIsEncrypted(document.as_ptr()) },
            "Encrypted PDFs are not supported"
        );
        let count = unsafe { CGPDFDocumentGetNumberOfPages(document.as_ptr()) };
        ensure!(
            (1..=MAX_PAGES).contains(&count),
            "PDF must contain between 1 and {MAX_PAGES} pages"
        );
        let mut pages = Vec::with_capacity(count);
        for index in 0..count {
            let page = NonNull::new(unsafe { CGPDFDocumentGetPage(document.as_ptr(), index + 1) })
                .context("Could not read PDF page")?;
            let bounds = unsafe { CGPDFPageGetBoxRect(page.as_ptr(), CROP_BOX) };
            let rotation = unsafe { CGPDFPageGetRotationAngle(page.as_ptr()) }.rem_euclid(360);
            let mut width = bounds.size.width;
            let mut height = bounds.size.height;
            ensure!(
                bounds.origin.x.is_finite()
                    && bounds.origin.y.is_finite()
                    && width.is_finite()
                    && height.is_finite()
                    && width > 0.0
                    && height > 0.0
                    && width <= MAX_PAGE_POINTS
                    && height <= MAX_PAGE_POINTS,
                "Invalid PDF page dimensions"
            );
            ensure!(rotation % 90 == 0, "Unsupported PDF page rotation");
            if rotation == 90 || rotation == 270 {
                std::mem::swap(&mut width, &mut height);
            }
            pages.push(PageSize { width, height });
        }
        Ok(Self {
            native,
            metadata: DocumentMetadata { pages },
        })
    }

    pub fn metadata(&self) -> &DocumentMetadata {
        &self.metadata
    }

    pub fn render(&self, request: TileRequest) -> Result<Vec<u8>> {
        let dimensions = self
            .metadata
            .pages
            .get(request.page as usize)
            .context("PDF page is out of range")?;
        let (_, _, length) = validate_tile(*dimensions, request)?;
        let scale = request.scale as f64 / RASTER_SCALE_DENOMINATOR as f64;
        let page = NonNull::new(unsafe {
            CGPDFDocumentGetPage(self.native.document()?.as_ptr(), request.page as usize + 1)
        })
        .context("Could not read PDF page")?;
        let mut pixels = vec![255u8; length];
        let color_space = ColorSpace(
            NonNull::new(unsafe { CGColorSpaceCreateDeviceRGB() })
                .context("Could not create RGB color space")?,
        );
        let context = BitmapContext(
            NonNull::new(unsafe {
                CGBitmapContextCreate(
                    pixels.as_mut_ptr().cast(),
                    request.width as usize,
                    request.height as usize,
                    8,
                    request.width as usize * 4,
                    color_space.0.as_ptr(),
                    BGRA_PREMULTIPLIED,
                )
            })
            .context("Could not allocate PDF tile context")?,
        );
        let context_pointer = context.0.as_ptr();
        let rectangle = Rect {
            origin: Point { x: 0.0, y: 0.0 },
            size: Size {
                width: dimensions.width,
                height: dimensions.height,
            },
        };
        unsafe {
            CGContextSetRGBFillColor(context_pointer, 1.0, 1.0, 1.0, 1.0);
            CGContextFillRect(
                context_pointer,
                Rect {
                    origin: Point { x: 0.0, y: 0.0 },
                    size: Size {
                        width: request.width as f64,
                        height: request.height as f64,
                    },
                },
            );
            // Quartz uses a bottom-left origin; protocol tiles are measured from the page's top-left.
            CGContextTranslateCTM(
                context_pointer,
                -(request.x as f64),
                request.y as f64 + request.height as f64 - dimensions.height * scale,
            );
            // CGPDFPageGetDrawingTransform only scales down. Normalize crop and
            // rotation in PDF points, then apply the requested raster scale explicitly.
            CGContextScaleCTM(context_pointer, scale, scale);
            CGContextConcatCTM(
                context_pointer,
                CGPDFPageGetDrawingTransform(page.as_ptr(), CROP_BOX, rectangle, 0, true),
            );
            CGContextSetInterpolationQuality(context_pointer, 3);
            CGContextSetShouldAntialias(context_pointer, true);
            CGContextDrawPDFPage(context_pointer, page.as_ptr());
        }
        // CoreGraphics borrows the Vec's allocation until the bitmap context is released.
        drop(context);
        Ok(pixels)
    }

    pub fn extract(&self, number: u32) -> Result<PageText> {
        let dimensions = *self
            .metadata
            .pages
            .get(number as usize)
            .context("PDF page is out of range")?;
        let mut text = NativeText::default();
        let mut error = [0 as c_char; 1024];
        ensure!(
            unsafe {
                gs_pdf_extract(
                    self.native.0.as_ptr(),
                    number,
                    &mut text,
                    error.as_mut_ptr(),
                    error.len(),
                )
            } != 0,
            "{}",
            native_error(&error)
        );
        if text.length == 0 {
            return Ok(PageText {
                page: number,
                text: String::new(),
                glyphs: Vec::new(),
            });
        }
        ensure!(
            text.length <= MAX_PAGE_TEXT_GLYPHS * 2
                && !text.characters.is_null()
                && !text.bounds.is_null(),
            "Invalid PDF text extraction buffer"
        );
        // Extraction may recycle the PDFKit owner; obtain its current borrowed
        // CoreGraphics page only after the request has completed.
        let page = NonNull::new(unsafe {
            CGPDFDocumentGetPage(self.native.document()?.as_ptr(), number as usize + 1)
        })
        .context("Could not read PDF text page")?;
        let transform = unsafe {
            CGPDFPageGetDrawingTransform(
                page.as_ptr(),
                CROP_BOX,
                Rect {
                    origin: Point::default(),
                    size: Size {
                        width: dimensions.width,
                        height: dimensions.height,
                    },
                },
                0,
                true,
            )
        };
        let characters = unsafe { std::slice::from_raw_parts(text.characters, text.length) };
        let rectangles = unsafe { std::slice::from_raw_parts(text.bounds, text.length) };
        decode_text(number, characters, rectangles, transform, dimensions)
    }
}

fn native_error(error: &[c_char]) -> anyhow::Error {
    anyhow::anyhow!(
        unsafe { CStr::from_ptr(error.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    )
}

fn decode_text(
    page: u32,
    characters: &[u16],
    rectangles: &[Rect],
    transform: Affine,
    dimensions: PageSize,
) -> Result<PageText> {
    ensure!(
        characters.len() == rectangles.len(),
        "PDF text geometry length mismatch"
    );
    let mut result = PageText {
        page,
        text: String::new(),
        glyphs: Vec::new(),
    };
    let mut index = 0;
    while index < characters.len() {
        ensure!(
            result.glyphs.len() < MAX_PAGE_TEXT_GLYPHS,
            "PDF page text exceeds glyph limit"
        );
        let first = characters[index];
        let mut bounds = display_bounds(rectangles[index], transform, dimensions);
        let scalar = if (0xd800..=0xdbff).contains(&first)
            && let Some(second) = characters
                .get(index + 1)
                .copied()
                .filter(|unit| (0xdc00..=0xdfff).contains(unit))
        {
            index += 1;
            bounds = union_bounds(
                bounds,
                display_bounds(rectangles[index], transform, dimensions),
            );
            0x10000 + (((first as u32 - 0xd800) << 10) | (second as u32 - 0xdc00))
        } else {
            first as u32
        };
        let character = char::from_u32(scalar).unwrap_or(char::REPLACEMENT_CHARACTER);
        let start = result.text.len() as u32;
        ensure!(
            result.text.len() + character.len_utf8() <= MAX_PAGE_TEXT_BYTES,
            "PDF page text exceeds byte limit"
        );
        result.text.push(character);
        result.glyphs.push(TextGlyph {
            start,
            end: result.text.len() as u32,
            bounds,
        });
        index += 1;
    }
    Ok(result)
}

fn display_bounds(rect: Rect, transform: Affine, dimensions: PageSize) -> [f32; 4] {
    let x = [rect.origin.x, rect.origin.x + rect.size.width];
    let y = [rect.origin.y, rect.origin.y + rect.size.height];
    if x.into_iter().chain(y).any(|value| !value.is_finite())
        || rect.size.width == 0.0
        || rect.size.height == 0.0
    {
        return [0.0; 4];
    }
    let mut left = f64::INFINITY;
    let mut top = f64::INFINITY;
    let mut right = f64::NEG_INFINITY;
    let mut bottom = f64::NEG_INFINITY;
    for x in x {
        for y in y {
            let horizontal = transform.a * x + transform.c * y + transform.tx;
            let vertical = dimensions.height - (transform.b * x + transform.d * y + transform.ty);
            left = left.min(horizontal);
            right = right.max(horizontal);
            top = top.min(vertical);
            bottom = bottom.max(vertical);
        }
    }
    left = left.clamp(0.0, dimensions.width);
    right = right.clamp(0.0, dimensions.width);
    top = top.clamp(0.0, dimensions.height);
    bottom = bottom.clamp(0.0, dimensions.height);
    [
        left as f32,
        top as f32,
        (right - left).max(0.0) as f32,
        (bottom - top).max(0.0) as f32,
    ]
}

fn union_bounds(first: [f32; 4], second: [f32; 4]) -> [f32; 4] {
    if first[2] == 0.0 || first[3] == 0.0 {
        return second;
    }
    if second[2] == 0.0 || second[3] == 0.0 {
        return first;
    }
    let left = first[0].min(second[0]);
    let top = first[1].min(second[1]);
    [
        left,
        top,
        (first[0] + first[2]).max(second[0] + second[2]) - left,
        (first[1] + first[3]).max(second[1] + second[3]) - top,
    ]
}

fn validate_tile(page: PageSize, request: TileRequest) -> Result<(u32, u32, usize)> {
    ensure!(
        (1..=MAX_SCALE).contains(&request.scale),
        "Invalid raster scale"
    );
    ensure!(
        (1..=TILE_SIZE).contains(&request.width) && (1..=TILE_SIZE).contains(&request.height),
        "Invalid tile size"
    );
    let scale = request.scale as f64 / RASTER_SCALE_DENOMINATOR as f64;
    let width = (page.width * scale).ceil();
    let height = (page.height * scale).ceil();
    ensure!(
        width.is_finite()
            && height.is_finite()
            && width >= 1.0
            && height >= 1.0
            && width <= u32::MAX as f64
            && height <= u32::MAX as f64,
        "Page raster dimensions are out of range"
    );
    let (width, height) = (width as u32, height as u32);
    ensure!(
        request
            .x
            .checked_add(request.width)
            .is_some_and(|right| right <= width)
            && request
                .y
                .checked_add(request.height)
                .is_some_and(|bottom| bottom <= height),
        "Tile is outside the page"
    );
    let length = (request.width as usize)
        .checked_mul(request.height as usize)
        .and_then(|length| length.checked_mul(4))
        .context("Tile allocation overflow")?;
    Ok((width, height, length))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> Affine {
        Affine {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            tx: 0.0,
            ty: 0.0,
        }
    }

    fn rectangle(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect {
            origin: Point { x, y },
            size: Size { width, height },
        }
    }

    #[test]
    fn utf16_scalars_keep_utf8_offsets_whitespace_and_surrogate_union() {
        let units: Vec<_> = "A😀 е\u{301}\n".encode_utf16().collect();
        let mut bounds = vec![Rect::default(); units.len()];
        bounds[1] = rectangle(10.0, 20.0, 6.0, 8.0);
        bounds[2] = rectangle(16.0, 20.0, 4.0, 8.0);
        let result = decode_text(
            0,
            &units,
            &bounds,
            identity(),
            PageSize {
                width: 100.0,
                height: 100.0,
            },
        )
        .expect("decode valid UTF-16");
        assert_eq!(result.text, "A😀 е\u{301}\n");
        assert_eq!(result.glyphs.len(), 6);
        assert_eq!((result.glyphs[1].start, result.glyphs[1].end), (1, 5));
        assert_eq!(result.glyphs[1].bounds, [10.0, 72.0, 10.0, 8.0]);
        for glyph in &result.glyphs {
            assert_eq!(
                result.text[glyph.start as usize..glyph.end as usize]
                    .chars()
                    .count(),
                1
            );
        }
    }

    #[test]
    fn malformed_utf16_is_replaced_without_invalid_offsets() {
        let units = [0xd800, b'A' as u16, 0xdc00];
        let result = decode_text(
            1,
            &units,
            &[Rect::default(); 3],
            identity(),
            PageSize {
                width: 100.0,
                height: 100.0,
            },
        )
        .expect("replace malformed code units");
        assert_eq!(result.text, "�A�");
        assert_eq!((result.glyphs[2].start, result.glyphs[2].end), (4, 7));
    }

    #[test]
    fn text_bounds_rotate_crop_clip_and_reject_nonfinite_geometry() {
        let transform = Affine {
            a: 0.0,
            b: -1.0,
            c: 1.0,
            d: 0.0,
            tx: -30.0,
            ty: 380.0,
        };
        let page = PageSize {
            width: 200.0,
            height: 360.0,
        };
        assert_eq!(
            display_bounds(rectangle(50.0, 190.0, 12.0, 20.0), transform, page),
            [160.0, 30.0, 20.0, 12.0]
        );
        assert_eq!(
            display_bounds(rectangle(50.0, 220.0, 12.0, 20.0), transform, page),
            [190.0, 30.0, 10.0, 12.0]
        );
        assert_eq!(
            display_bounds(rectangle(f64::INFINITY, 0.0, 1.0, 1.0), transform, page),
            [0.0; 4]
        );
    }

    #[test]
    fn text_glyph_limit_is_explicit() {
        let units = vec![b'a' as u16; MAX_PAGE_TEXT_GLYPHS + 1];
        let bounds = vec![Rect::default(); units.len()];
        assert!(
            decode_text(
                0,
                &units,
                &bounds,
                identity(),
                PageSize {
                    width: 100.0,
                    height: 100.0
                }
            )
            .expect_err("reject excessive glyph count")
            .to_string()
            .contains("glyph limit")
        );
    }

    #[test]
    fn tile_geometry_uses_ceil_at_fractional_scale() {
        let page = PageSize {
            width: 100.25,
            height: 120.75,
        };
        let tile = TileRequest {
            page: 0,
            scale: 1536,
            x: 150,
            y: 181,
            width: 1,
            height: 1,
        };
        assert_eq!(validate_tile(page, tile).unwrap(), (151, 182, 4));
    }

    #[test]
    fn rejects_unbounded_and_overflowing_tiles_before_allocation() {
        let page = PageSize {
            width: 100.0,
            height: 100.0,
        };
        let tile = TileRequest {
            page: 0,
            scale: 1024,
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        };
        for invalid in [
            TileRequest { scale: 0, ..tile },
            TileRequest {
                scale: u32::MAX,
                ..tile
            },
            TileRequest { width: 0, ..tile },
            TileRequest {
                width: TILE_SIZE + 1,
                ..tile
            },
            TileRequest {
                x: u32::MAX,
                ..tile
            },
            TileRequest { y: 1, ..tile },
        ] {
            assert!(validate_tile(page, invalid).is_err());
        }
        assert!(
            validate_tile(
                PageSize {
                    width: f64::INFINITY,
                    height: 1.0
                },
                tile
            )
            .is_err()
        );
    }
}
