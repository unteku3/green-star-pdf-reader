#!/usr/bin/env python3
"""Exercise the real renderer using generated, redistributable PDF fixtures."""
import hashlib
import json
import pathlib
import select
import struct
import subprocess
import sys
import tempfile
import time


def assemble(objects, extra_trailer=b""):
    data = bytearray(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n")
    offsets = [0]
    for number, body in enumerate(objects, 1):
        offsets.append(len(data))
        data += f"{number} 0 obj\n".encode() + body + b"\nendobj\n"
    position = len(data)
    data += f"xref\n0 {len(offsets)}\n0000000000 65535 f \n".encode()
    data += b"".join(f"{offset:010d} 00000 n \n".encode() for offset in offsets[1:])
    data += f"trailer\n<< /Size {len(offsets)} /Root 1 0 R ".encode()
    data += extra_trailer + f">>\nstartxref\n{position}\n%%EOF\n".encode()
    return bytes(data)


def colored_pdf():
    # Crop-box corners: top-left blue, top-right black, bottom-left red, bottom-right green.
    content = b"1 0 0 rg 50 80 100 150 re f\n0 1 0 rg 150 80 100 150 re f\n0 0 1 rg 50 230 100 150 re f\n0 0 0 rg 150 230 100 150 re f\n"
    objects = [b"<< /Type /Catalog /Pages 2 0 R >>", b"<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R 6 0 R] /Count 4 >>"]
    for rotation in [0, 90, 180, 270]:
        objects.append(f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 500 700] /CropBox [50 80 250 380] /Rotate {rotation} /Resources << >> /Contents 7 0 R >>".encode())
    objects.append(f"<< /Length {len(content)} >>\nstream\n".encode() + content + b"endstream")
    return assemble(objects)


def rc4(key, data):
    state = list(range(256))
    cursor = 0
    for index in range(256):
        cursor = (cursor + state[index] + key[index % len(key)]) % 256
        state[index], state[cursor] = state[cursor], state[index]
    first = second = 0
    result = bytearray()
    for value in data:
        first = (first + 1) % 256
        second = (second + state[first]) % 256
        state[first], state[second] = state[second], state[first]
        result.append(value ^ state[(state[first] + state[second]) % 256])
    return bytes(result)


def encrypted_pdf():
    padding = bytes.fromhex("28bf4e5e4e758a4164004e56fffa01082e2e00b6d0683e802f0ca9fe6453697a")
    pad = lambda value: (value + padding)[:32]
    identifier = hashlib.md5(b"generated PDF Reader fixture").digest()
    owner = rc4(hashlib.md5(pad(b"owner")).digest()[:5], pad(b"password"))
    key = hashlib.md5(pad(b"password") + owner + struct.pack("<i", -4) + identifier).digest()[:5]
    user = rc4(key, padding)
    objects = [b"<< /Type /Catalog /Pages 2 0 R >>", b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
               b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << >> >>",
               f"<< /Filter /Standard /V 1 /R 2 /Length 40 /O <{owner.hex()}> /U <{user.hex()}> /P -4 >>".encode()]
    return assemble(objects, f"/Encrypt 4 0 R /ID [<{identifier.hex()}> <{identifier.hex()}>] ".encode())


class Client:
    def __init__(self, binary):
        self.process = subprocess.Popen([str(binary)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.serial = 0

    def exact(self, size):
        data = bytearray()
        deadline = time.monotonic() + 15
        while len(data) < size:
            remaining = deadline - time.monotonic()
            assert remaining > 0 and select.select([self.process.stdout], [], [], remaining)[0], "IPC response timed out"
            part = self.process.stdout.read1(size - len(data))
            assert part, "Unexpected renderer EOF"
            data.extend(part)
        return bytes(data)

    def frame(self):
        size = struct.unpack("<I", self.exact(4))[0]
        assert size <= 64 * 1024**2
        return self.exact(size)

    def call(self, command, view=1):
        self.serial += 1
        data = json.dumps(dict(id=self.serial, view=view, command=command)).encode()
        self.process.stdin.write(struct.pack("<I", len(data)) + data)
        self.process.stdin.flush()
        response = json.loads(self.frame())
        assert response["id"] == self.serial
        blobs = [self.frame() for _ in range(response["blobs"])]
        return response, blobs

    def success(self, command, view=1):
        response, blobs = self.call(command, view)
        assert response["error"] is None, response
        return response, blobs

    def tile(self, page, width, height, scale=1024, x=0, y=0, view=1):
        request = dict(page=page, scale=scale, x=x, y=y, width=width, height=height)
        response, blobs = self.success({"RenderPageTile": {"tile": request}}, view)
        assert response["tile"] == {"request": request, "blob": 0}
        assert len(blobs) == 1 and len(blobs[0]) == width * height * 4
        return blobs[0]

    def close(self):
        self.process.stdin.close()
        assert self.process.wait(timeout=5) == 0, self.process.stderr.read().decode(errors="replace")

    def abort(self):
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait()


def pixel(data, width, x, y):
    position = (y * width + x) * 4
    return tuple(data[position:position + 4])


def main():
    binary = pathlib.Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory(prefix="pdf-reader-ipc-") as directory:
        root = pathlib.Path(directory)
        sample = root / "Учебник с пробелами.pdf"
        sample.write_bytes(colored_pdf())
        broken = root / "broken.pdf"
        broken.write_bytes(b"%PDF-1.4\nthis is not a PDF")
        empty = root / "empty.pdf"
        empty.write_bytes(assemble([b"<< /Type /Catalog /Pages 2 0 R >>", b"<< /Type /Pages /Kids [] /Count 0 >>"]))
        encrypted = root / "encrypted.pdf"
        encrypted.write_bytes(encrypted_pdf())
        client = Client(binary)
        try:
            assert client.call({"OpenPagedDocument": {"path": str(sample)}})[0]["error"] == "Handshake required"
            assert client.call({"Hello": {"api_version": 99}})[0]["error"]
            client.success({"Hello": {"api_version": 1}})
            response, _ = client.success({"OpenPagedDocument": {"path": str(sample)}})
            assert response["document"]["pages"] == [{"width": 200.0, "height": 300.0}, {"width": 300.0, "height": 200.0}] * 2
            blue, black, red, green = (255, 0, 0, 255), (0, 0, 0, 255), (0, 0, 255, 255), (0, 255, 0, 255)
            expected = [(blue, black, red, green), (red, blue, green, black), (green, red, black, blue), (black, green, blue, red)]
            for page, corners in enumerate(expected):
                width, height = (200, 300) if page % 2 == 0 else (300, 200)
                data = client.tile(page, width, height)
                actual = tuple(pixel(data, width, x, y) for x, y in [(10, 10), (width - 10, 10), (10, height - 10), (width - 10, height - 10)])
                assert actual == corners, (page, actual, corners)
                # Directly clipped tiles must exactly match the same area of a full render.
                full = client.tile(page, width * 2, height * 2, scale=2048)
                for x, y, tile_width, tile_height in [(17, 35, 130, 211), (width * 2 - 41, height * 2 - 53, 41, 53)]:
                    tile = client.tile(page, tile_width, tile_height, scale=2048, x=x, y=y)
                    reference = b"".join(full[((y + row) * width * 2 + x) * 4:((y + row) * width * 2 + x + tile_width) * 4] for row in range(tile_height))
                    assert tile == reference, ("clipped tile differs", page, x, y)
            client.tile(0, 1024, 1024, scale=8192, x=512, y=1000)
            bad_tile = dict(page=0, scale=1024, x=0, y=0, width=100, height=100)
            for field, value in [("page", 100), ("scale", 0), ("scale", 4294967295), ("width", 1025), ("height", 0), ("x", 4294967295), ("y", 299)]:
                response, blobs = client.call({"RenderPageTile": {"tile": dict(bad_tile, **{field: value})}})
                assert response["error"] and not blobs
            for path in [broken, empty, encrypted, root / "missing.pdf", root]:
                response, _ = client.call({"OpenPagedDocument": {"path": str(path)}}, view=2)
                assert response["error"], path
                if path == encrypted:
                    assert "Encrypted" in response["error"], response
            client.success({"OpenPagedDocument": {"path": str(sample)}}, view=2)
            client.success("Close", view=1)
            assert client.call({"RenderPageTile": {"tile": bad_tile}}, view=1)[0]["error"]
            client.tile(0, 100, 100, view=2)
            assert client.call({"RenderFormula": {"latex": "x", "display": False}}, view=2)[0]["error"]
            client.success("Close", view=2)
            client.success({"OpenPagedDocument": {"path": str(sample)}}, view=3)
            client.tile(0, 100, 100, view=3)
            client.success("Close", view=3)
            client.close()
        finally:
            client.abort()
        truncated = subprocess.run([str(binary)], input=struct.pack("<I", 10) + b"abc", capture_output=True, timeout=5)
        assert truncated.returncode != 0
    print("IPC passed: crop/rotation/BGRA, direct tiles, Retina scales, Unicode paths, invalid inputs, encryption, shared views, reopen and shutdown")


if __name__ == "__main__":
    main()
