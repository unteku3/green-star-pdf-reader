#!/usr/bin/env python3
"""Generate a PDF with explicit Unicode maps and known origins; requires fontTools.

The embedded TrueType face is generated from rectangles, not a redistributed font.
Its exact outlines make geometry assertions independent of the PDF text extractor.
"""

import argparse
import json
import io
from pathlib import Path
import zlib

from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen


LINES = ("Alpha Привет 😀", "space  two é e\u0301 end")
CROP = (20, 30, 380, 230)
ROTATIONS = (0, 90, 180, 270)
FONT_SIZE = 20
ORIGINS = ((50, 190), (50, 150))


def text_fixture(lines=LINES, origins=ORIGINS, font_size=FONT_SIZE, rotations=ROTATIONS):
    objects = [b"", b""]

    def add(value):
        objects.append(value)
        return len(objects)

    def stream(value, attributes=b""):
        return add(b"<< /Length " + str(len(value)).encode() + b" " + attributes + b" >>\nstream\n" + value + b"\nendstream")

    characters = list(dict.fromkeys("".join(lines)))
    codes = {character: index + 1 for index, character in enumerate(characters)}
    advances = {character: (300 if character == " " else 0 if character == "\u0301" else 600)
                for character in characters}
    names = {character: f"g{codes[character]}" for character in characters}
    glyphs, metrics = {}, {}
    for character, name in [(None, ".notdef"), *names.items()]:
        pen = TTGlyphPen(None)
        left = 0
        if character not in (None, " "):
            left, bottom, right, top = (-220, 700, -80, 850) if character == "\u0301" else (50, 0, 500, 700)
            pen.moveTo((left, bottom)); pen.lineTo((right, bottom))
            pen.lineTo((right, top)); pen.lineTo((left, top)); pen.closePath()
        glyphs[name] = pen.glyph()
        metrics[name] = (advances.get(character, 600), left)
    builder = FontBuilder(1000, isTTF=True)
    builder.setupGlyphOrder([".notdef", *names.values()])
    builder.setupCharacterMap({ord(character): name for character, name in names.items()})
    builder.setupGlyf(glyphs)
    builder.setupHorizontalMetrics(metrics)
    builder.setupHorizontalHeader(ascent=850, descent=-200)
    builder.setupNameTable({"familyName": "GreenStarQA", "styleName": "Regular",
                           "uniqueFontIdentifier": "GreenStarQA-Regular", "fullName": "GreenStarQA Regular",
                           "psName": "GreenStarQA-Regular"})
    builder.setupOS2(sTypoAscender=850, sTypoDescender=-200, usWinAscent=850, usWinDescent=200)
    builder.setupPost(); builder.setupMaxp()
    font_bytes = io.BytesIO(); builder.save(font_bytes)
    font_file = stream(font_bytes.getvalue(), f"/Length1 {len(font_bytes.getvalue())}".encode())
    descriptor = add(("<< /Type /FontDescriptor /FontName /GreenStarQA-Regular /Flags 4 "
                      "/FontBBox [-220 -200 600 850] /ItalicAngle 0 /Ascent 850 /Descent -200 "
                      f"/CapHeight 700 /StemV 80 /FontFile2 {font_file} 0 R >>").encode())
    mapping = "\n".join(f"<{codes[character]:04X}> <{character.encode('utf-16-be').hex().upper()}>"
                        for character in characters)
    cmap = stream((
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n"
        "/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n"
        "/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\n"
        f"endcodespacerange\n{len(characters)} beginbfchar\n{mapping}\nendbfchar\n"
        "endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend"
    ).encode())
    widths = " ".join(str(advances[character]) for character in characters)
    descendant = add(("<< /Type /Font /Subtype /CIDFontType2 /BaseFont /GreenStarQA-Regular "
                      "/CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> "
                      f"/FontDescriptor {descriptor} 0 R /CIDToGIDMap /Identity /DW 600 /W [1 [{widths}]] >>").encode())
    font = add((
        "<< /Type /Font /Subtype /Type0 /BaseFont /GreenStarQA-Regular /Encoding /Identity-H "
        f"/DescendantFonts [{descendant} 0 R] "
        f"/ToUnicode {cmap} 0 R >>"
    ).encode())
    commands = [b"0 0 0 rg"]
    expected_lines = []
    for text, (x, y) in zip(lines, origins):
        encoded = "".join(f"{codes[character]:04X}" for character in text)
        commands.append(f"BT /F1 {font_size} Tf 1 0 0 1 {x} {y} Tm <{encoded}> Tj ET".encode())
        cursor = x
        positions = []
        for character in text:
            positions.append({"character": character, "origin": [cursor, y],
                              "advance": advances[character] * font_size / 1000})
            cursor += advances[character] * font_size / 1000
        expected_lines.append({"text": text, "origin": [x, y], "glyphs": positions})
    contents = stream(b"\n".join(commands))
    pages = []
    for rotation in rotations:
        pages.append(add((
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 260] /CropBox [20 30 380 230] "
            f"/Rotate {rotation} /Resources << /Font << /F1 {font} 0 R >> >> /Contents {contents} 0 R >>"
        ).encode()))
    pixels = bytes([30, 100, 220] * 8 * 8)
    compressed = zlib.compress(pixels)
    picture = add((f"<< /Type /XObject /Subtype /Image /Width 8 /Height 8 /ColorSpace /DeviceRGB "
                   f"/BitsPerComponent 8 /Filter /FlateDecode /Length {len(compressed)} >>\nstream\n").encode()
                  + compressed + b"\nendstream")
    picture_contents = stream(b"q 128 0 0 128 60 60 cm /Im1 Do Q")
    pages.append(add((
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 260] /CropBox [20 30 380 230] "
        f"/Resources << /XObject << /Im1 {picture} 0 R >> >> /Contents {picture_contents} 0 R >>"
    ).encode()))
    objects[0] = b"<< /Type /Catalog /Pages 2 0 R >>"
    references = " ".join(f"{page} 0 R" for page in pages)
    objects[1] = f"<< /Type /Pages /Kids [{references}] /Count {len(pages)} >>".encode()
    output = bytearray(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n")
    offsets = [0]
    for index, value in enumerate(objects, 1):
        offsets.append(len(output))
        output.extend(f"{index} 0 obj\n".encode() + value + b"\nendobj\n")
    xref = len(output)
    output.extend(f"xref\n0 {len(offsets)}\n0000000000 65535 f \n".encode())
    for offset in offsets[1:]:
        output.extend(f"{offset:010d} 00000 n \n".encode())
    output.extend(f"trailer\n<< /Size {len(offsets)} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode())
    expected = {"text": "\n".join(lines), "crop": CROP, "rotations": rotations,
                "font_size": font_size, "lines": expected_lines, "blank_page": len(rotations)}
    return bytes(output), expected


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    args = parser.parse_args()
    args.directory.mkdir(parents=True, exist_ok=True)
    data, expected = text_fixture()
    (args.directory / "unicode-text.pdf").write_bytes(data)
    (args.directory / "unicode-text.expected.json").write_text(json.dumps(expected, ensure_ascii=False, indent=2) + "\n")
    print(args.directory / "unicode-text.pdf")


if __name__ == "__main__":
    main()
