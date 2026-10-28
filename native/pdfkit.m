#import <Foundation/Foundation.h>
#import <PDFKit/PDFKit.h>
#import <CoreGraphics/CoreGraphics.h>
#include <stdint.h>
#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>

enum { MAX_TEXT_BYTES = 1024 * 1024, MAX_GLYPHS = 100000, RECYCLE_PAGES = 32 };

typedef struct {
    uint16_t *characters;
    CGRect *bounds;
    size_t length;
} GSText;

@interface GSPDFOwner : NSObject
@property(nonatomic, strong) PDFDocument *document;
@property(nonatomic, strong) NSURL *url;
@property(nonatomic, strong) NSMutableIndexSet *extracted;
@property(nonatomic) struct stat identity;
@property(nonatomic) NSUInteger pageCount;
@end
@implementation GSPDFOwner
@end

static void error_message(char *error, size_t capacity, NSString *message) {
    if (capacity) snprintf(error, capacity, "%s", message.UTF8String ?: "PDFKit error");
}

void *gs_pdf_open(const char *path, char *error, size_t capacity) {
    @autoreleasepool {
        @try {
            NSString *string = [[NSFileManager defaultManager]
                stringWithFileSystemRepresentation:path length:strlen(path)];
            if (!string) { error_message(error, capacity, @"Invalid PDF path"); return NULL; }
            GSPDFOwner *owner = [[GSPDFOwner alloc] init];
            owner.url = [NSURL fileURLWithPath:string];
            struct stat identity;
            if (stat(path, &identity)) {
                error_message(error, capacity, @"Could not read PDF file"); return NULL;
            }
            owner.identity = identity;
            owner.document = [[PDFDocument alloc] initWithURL:owner.url];
            if (!owner.document || !owner.document.documentRef) {
                error_message(error, capacity, @"Could not parse PDF document"); return NULL;
            }
            owner.pageCount = CGPDFDocumentGetNumberOfPages(owner.document.documentRef);
            owner.extracted = [[NSMutableIndexSet alloc] init];
            return (__bridge_retained void *)owner;
        } @catch (NSException *exception) {
            error_message(error, capacity, exception.reason ?: @"PDFKit open failed");
            return NULL;
        }
    }
}

void gs_pdf_close(void *pointer) {
    @autoreleasepool {
        GSPDFOwner *owner = (__bridge_transfer GSPDFOwner *)pointer;
        owner.document = nil;
    }
}

CGPDFDocumentRef gs_pdf_document(void *pointer) {
    @autoreleasepool {
        return ((__bridge GSPDFOwner *)pointer).document.documentRef;
    }
}

void gs_pdf_free_text(GSText *text) {
    free(text->characters);
    free(text->bounds);
    *text = (GSText){0};
}

static BOOL recycle(GSPDFOwner *owner, char *error, size_t capacity) {
    struct stat current;
    struct stat original = owner.identity;
    if (stat(owner.url.fileSystemRepresentation, &current)
        || current.st_dev != original.st_dev || current.st_ino != original.st_ino
        || current.st_size != original.st_size
        || current.st_mtimespec.tv_sec != original.st_mtimespec.tv_sec
        || current.st_mtimespec.tv_nsec != original.st_mtimespec.tv_nsec
        || current.st_ctimespec.tv_sec != original.st_ctimespec.tv_sec
        || current.st_ctimespec.tv_nsec != original.st_ctimespec.tv_nsec) {
        error_message(error, capacity, @"PDF changed on disk; reopen it before selecting more text");
        return NO;
    }
    // PDFKit retains per-page character caches. Drop the complete owner before
    // reopening; rendered tiles and returned text are owned outside PDFKit.
    owner.document = nil;
    owner.document = [[PDFDocument alloc] initWithURL:owner.url];
    if (!owner.document || !owner.document.documentRef
        || CGPDFDocumentGetNumberOfPages(owner.document.documentRef) != owner.pageCount) {
        error_message(error, capacity, @"Could not reopen PDF text parser");
        return NO;
    }
    [owner.extracted removeAllIndexes];
    return YES;
}

int gs_pdf_extract(void *pointer, uint32_t number, GSText *result, char *error, size_t capacity) {
    *result = (GSText){0};
    @autoreleasepool {
        @try {
            GSPDFOwner *owner = (__bridge GSPDFOwner *)pointer;
            if (number >= owner.pageCount) {
                error_message(error, capacity, @"PDF page is out of range"); return 0;
            }
            if ((!owner.document || (owner.extracted.count >= RECYCLE_PAGES
                && ![owner.extracted containsIndex:number])) && !recycle(owner, error, capacity)) return 0;
            PDFPage *page = [owner.document pageAtIndex:number];
            if (!page) { error_message(error, capacity, @"Could not read PDF text page"); return 0; }
            NSString *text = page.string ?: @"";
            [owner.extracted addIndex:number];
            NSUInteger count = text.length;
            if (count > MAX_GLYPHS * 2 || [text lengthOfBytesUsingEncoding:NSUTF8StringEncoding] > MAX_TEXT_BYTES) {
                error_message(error, capacity, @"PDF page text exceeds extraction limits"); return 0;
            }
            if (!count) return 1;
            result->characters = malloc(count * sizeof(uint16_t));
            result->bounds = calloc(count, sizeof(CGRect));
            if (!result->characters || !result->bounds) {
                gs_pdf_free_text(result);
                error_message(error, capacity, @"Could not allocate PDF text geometry"); return 0;
            }
            result->length = count;
            [text getCharacters:result->characters range:NSMakeRange(0, count)];
            NSUInteger glyphCount = 0;
            for (NSUInteger index = 0; index < count;) {
                uint16_t first = result->characters[index];
                NSUInteger length = 1;
                if (first >= 0xd800 && first <= 0xdbff && index + 1 < count
                    && result->characters[index + 1] >= 0xdc00 && result->characters[index + 1] <= 0xdfff) length = 2;
                if (++glyphCount > MAX_GLYPHS) {
                    gs_pdf_free_text(result);
                    error_message(error, capacity, @"PDF page text exceeds glyph limit"); return 0;
                }
                // characterBoundsAtIndex does not reliably index page.string:
                // some PDFs omit line breaks and report zero-height Cyrillic.
                // PDFSelection uses the actual NSString range and font metrics.
                if (first != '\n' && first != '\r' && first != 0x2028 && first != 0x2029) {
                    @autoreleasepool {
                        PDFSelection *selection = [page selectionForRange:NSMakeRange(index, length)];
                        CGRect bounds = selection ? [selection boundsForPage:page] : CGRectZero;
                        result->bounds[index] = bounds;
                        if (length == 2) result->bounds[index + 1] = bounds;
                    }
                }
                index += length;
            }
            return 1;
        } @catch (NSException *exception) {
            gs_pdf_free_text(result);
            error_message(error, capacity, exception.reason ?: @"PDFKit text extraction failed");
            return 0;
        }
    }
}
