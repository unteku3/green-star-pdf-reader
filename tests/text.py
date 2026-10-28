#!/usr/bin/env python3
"""Test paged_text_v1 using generated PDFs; requires the fontTools dev dependency.

Run: python3 tests/text.py target/release/green-star-pdf-reader
No editor checkout, installed font, private document or network access is needed.
"""

import math
from pathlib import Path
import sys
import tempfile

from ipc import Client
from text_fixture import text_fixture


def open_document(client, path, view=1):
    reply, blobs = client.success({"OpenPagedDocument": {"path": str(path)}}, view)
    assert not blobs and reply.get("document"), "Missing document metadata"
    return reply["document"]["pages"]


def extract(client, page, view=1):
    reply, blobs = client.success({"ExtractPageText": {"page": page}}, view)
    assert not blobs and reply.get("page_text"), "Missing text response"
    result = reply["page_text"]
    assert result["page"] == page, "Wrong page identity"
    encoded = result["text"].encode("utf-8")
    assert len(encoded) <= 1024 * 1024 and len(result["glyphs"]) <= 100000
    previous = 0
    scalars = []
    for glyph in result["glyphs"]:
        start, end = glyph["start"], glyph["end"]
        assert previous <= start < end <= len(encoded), "Invalid or unordered UTF-8 range"
        character = encoded[start:end].decode("utf-8")
        assert len(character) == 1, "A glyph must cover one Unicode scalar"
        bounds = glyph["bounds"]
        assert len(bounds) == 4 and all(math.isfinite(value) for value in bounds)
        assert bounds[2] >= 0 and bounds[3] >= 0
        previous = end
        scalars.append(character)
    return result, scalars


def check_geometry(result, scalars, expected, rotation):
    ink = [glyph for line in expected["lines"] for glyph in line["glyphs"]
           if not glyph["character"].isspace() and glyph["character"] != "\u0301"]
    actual_ink = [(character, glyph) for character, glyph in zip(scalars, result["glyphs"])
                  if not character.isspace() and character != "\u0301"]
    assert len(actual_ink) == len(ink)
    for source, (character, glyph) in zip(ink, actual_ink):
        assert character == source["character"]
        # Known generated-font ink center, independently transformed from PDF
        # points. Comparing bounds only with another extractor could hide errors.
        source_x, source_y = source["origin"]
        a, b = source_x + 5.5 - 20, source_y + 7 - 30
        point = {0: (a, 200 - b), 90: (b, a),
                 180: (360 - a, b), 270: (200 - b, 360 - a)}[rotation]
        x, y, width, height = glyph["bounds"]
        assert 0 < width <= 25 and 0 < height <= 25, (rotation, character, glyph)
        assert x - 0.5 <= point[0] <= x + width + 0.5 and y - 0.5 <= point[1] <= y + height + 0.5, (
            f"Rotation {rotation}, {character!r}: known ink center {point} outside {glyph['bounds']}"
        )


def main():
    if len(sys.argv) != 2:
        raise SystemExit("Usage: python3 tests/text.py target/release/green-star-pdf-reader")
    binary = Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory(prefix="pdf-reader-text-") as directory:
        root = Path(directory)
        fixture, expected = text_fixture()
        path = root / "Unicode text кириллица.pdf"
        path.write_bytes(fixture)
        # PDFKit and Poppler both normalize repeated spaces. The decomposed
        # accent must remain decomposed, and the non-BMP scalar must remain whole.
        wanted = expected["text"].replace("  ", " ")
        client = Client(binary)
        try:
            client.success({"Hello": {"api_version": 1}})
            pages = open_document(client, path)
            assert [(page["width"], page["height"]) for page in pages] == [
                (360, 200), (200, 360), (360, 200), (200, 360), (360, 200)]
            for page, rotation in enumerate(expected["rotations"]):
                result, scalars = extract(client, page)
                assert result["text"] == wanted, (rotation, result["text"], wanted)
                assert "".join(scalars) == wanted, "Missing scalar spans"
                check_geometry(result, scalars, expected, rotation)
                assert extract(client, page)[0] == result, "Text extraction is not deterministic"
            empty, scalars = extract(client, expected["blank_page"])
            assert empty["text"] == "" and not scalars, "Image-only page contains text"
            for page in (5, 4294967295):
                reply, blobs = client.call({"ExtractPageText": {"page": page}})
                assert reply.get("error") and not blobs, "Invalid text page was accepted"
            open_document(client, path, view=2)
            assert extract(client, 2, view=2)[0]["text"] == wanted
            client.success("Close", view=2)
            reply, blobs = client.call({"ExtractPageText": {"page": 0}}, view=2)
            assert reply.get("error") and not blobs, "Closed view still exposes text"
            assert extract(client, 0)[0]["text"] == wanted
            client.success("Close")

            oversized = root / "overlimit.pdf"
            oversized.write_bytes(text_fixture(
                lines=["A" * 1000] * 101,
                origins=[(50, 190 - index * 0.5) for index in range(101)],
                font_size=0.2, rotations=[0],
            )[0])
            open_document(client, oversized)
            reply, blobs = client.call({"ExtractPageText": {"page": 0}})
            assert reply.get("error") and "limit" in reply["error"].lower() and not blobs, "Glyph limit not enforced"
            client.tile(0, 64, 64)
            client.success("Close")
            open_document(client, path)
            assert extract(client, 0)[0]["text"] == wanted
            client.success("Close")
            client.close()
        finally:
            client.abort()
    print(f"Text IPC passed ({client.serial} requests): Unicode scalar UTF-8, known ink geometry "
          "at 0/90/180/270, crop origins, image-only page, shared views, invalid inputs, "
          "100k glyph limit, render/text recovery and shutdown")


if __name__ == "__main__":
    main()
