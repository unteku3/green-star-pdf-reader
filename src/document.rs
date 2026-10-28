use anyhow::{Context as _, Result, ensure};
use std::{
    ffi::{CString, c_char, c_int, c_void},
    os::unix::ffi::OsStrExt,
    path::Path,
    ptr::NonNull,
};
use viewer_protocol::{
    DocumentMetadata, PageSize, RASTER_SCALE_DENOMINATOR, TILE_SIZE, TileRequest,
};

#[cfg(not(target_os = "macos"))]
compile_error!("PDF Reader requires macOS CoreGraphics");

const CROP_BOX: c_int = 1;
const MAX_PAGES: usize = 100_000;
const MAX_PAGE_POINTS: f64 = 1_000_000.0;
const MAX_SCALE: u32 = 64 * RASTER_SCALE_DENOMINATOR;
const BGRA_PREMULTIPLIED: u32 = 0x2002;

#[repr(C)]
#[derive(Clone, Copy)]
struct Point {
    x: f64,
    y: f64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Size {
    width: f64,
    height: f64,
}
#[repr(C)]
#[derive(Clone, Copy)]
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
    fn CGDataProviderCreateWithFilename(filename: *const c_char) -> *mut c_void;
    fn CGDataProviderRelease(provider: *mut c_void);
    fn CGPDFDocumentCreateWithProvider(provider: *mut c_void) -> *mut c_void;
    fn CGPDFDocumentRelease(document: *mut c_void);
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

struct Provider(NonNull<c_void>);
impl Drop for Provider {
    fn drop(&mut self) {
        unsafe { CGDataProviderRelease(self.0.as_ptr()) }
    }
}
struct NativeDocument(NonNull<c_void>);
impl Drop for NativeDocument {
    fn drop(&mut self) {
        unsafe { CGPDFDocumentRelease(self.0.as_ptr()) }
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
    // The provider remains alive until after its document is released.
    _provider: Provider,
}

impl Document {
    pub fn open(path: &Path) -> Result<Self> {
        let path = CString::new(path.as_os_str().as_bytes()).context("Invalid PDF path")?;
        let provider = Provider(
            NonNull::new(unsafe { CGDataProviderCreateWithFilename(path.as_ptr()) })
                .context("Could not read PDF file")?,
        );
        let native = NativeDocument(
            NonNull::new(unsafe { CGPDFDocumentCreateWithProvider(provider.0.as_ptr()) })
                .context("Could not parse PDF document")?,
        );
        ensure!(
            !unsafe { CGPDFDocumentIsEncrypted(native.0.as_ptr()) },
            "Encrypted PDFs are not supported"
        );
        let count = unsafe { CGPDFDocumentGetNumberOfPages(native.0.as_ptr()) };
        ensure!(
            (1..=MAX_PAGES).contains(&count),
            "PDF must contain between 1 and {MAX_PAGES} pages"
        );
        let mut pages = Vec::with_capacity(count);
        for index in 0..count {
            let page = NonNull::new(unsafe { CGPDFDocumentGetPage(native.0.as_ptr(), index + 1) })
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
            _provider: provider,
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
            CGPDFDocumentGetPage(self.native.0.as_ptr(), request.page as usize + 1)
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
