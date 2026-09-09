"""Export the known, independently verified clean-first proof as a compact policy."""

import argparse
import gzip
import hashlib
import json
import struct
from pathlib import Path

SOURCE_SHA256 = "c01f23955ed3c7bc797e0c3665688fd6ab5a8ae0807ef40747e79c45b70d1010"
HEADER_BYTES = 47
RECORD_BYTES = 11
ALL_MOVES = 255


def encode_choices(rows):
    """Sorted exact state-code deltas (unsigned LEB128), then one move byte."""
    encoded = bytearray()
    previous = 0
    for state, move in rows:
        if not (0 <= move < 36 and state > previous):
            raise ValueError("Invalid or unsorted strategy choice")
        delta = state - previous
        previous = state
        while delta >= 128:
            encoded.append((delta & 127) | 128)
            delta >>= 7
        encoded.extend((delta, move))
    return encoded


def check_roundtrip(compressed, rows):
    """Compare every decoded state and move with the original certificate."""
    raw = gzip.decompress(compressed)
    offset = state = 0
    for expected, move in rows:
        delta = shift = 0
        while True:
            value = raw[offset]
            offset += 1
            delta |= (value & 127) << shift
            shift += 7
            if value < 128:
                break
        state += delta
        if state != expected or raw[offset] != move:
            raise ValueError("Strategy roundtrip mismatch")
        offset += 1
    if offset != len(raw):
        raise ValueError("Unexpected trailing policy bytes")


def export(source, destination):
    certificate = source.read_bytes()
    digest = hashlib.sha256(certificate).hexdigest()
    if digest != SOURCE_SHA256 or certificate[:8] != b"BRCERT02":
        raise ValueError("Export requires the known independently verified clean-first certificate")
    count = struct.unpack_from("<Q", certificate, 39)[0]
    if HEADER_BYTES + count * RECORD_BYTES != len(certificate):
        raise ValueError("Certificate length mismatch")
    records = struct.iter_unpack("<QHB", memoryview(certificate)[HEADER_BYTES:])
    rows = sorted((state, move) for state, _rank, move in records if move != ALL_MOVES)
    encoded = encode_choices(rows)
    compressed = gzip.compress(encoded, compresslevel=6, mtime=0)
    check_roundtrip(compressed, rows)

    sha = hashlib.sha256(compressed).hexdigest()
    filename = f"strategy-{sha[:12]}.bin.gz"
    metadata = {
        "format": 1,
        "file": filename,
        "sha256": sha,
        "sourceSha256": digest,
        "entries": len(rows),
        "bytes": len(compressed),
        "decodedBytes": len(encoded),
        "bound": 25,
    }
    destination.mkdir(parents=True, exist_ok=True)
    (destination / filename).write_bytes(compressed)
    (destination / "strategy.json").write_text(json.dumps(metadata, indent=2) + "\n")
    print(json.dumps(metadata))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("certificate", type=Path)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    export(args.certificate, args.destination)


if __name__ == "__main__":
    main()
