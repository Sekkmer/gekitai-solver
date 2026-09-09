"""Package the static site without build outputs, tests, or proof archives."""
from pathlib import Path
from zipfile import ZIP_DEFLATED, ZipFile

root = Path(__file__).resolve().parent.parent
site = root / "web"
output = root / "dist" / "gekitai-static.zip"
output.parent.mkdir(exist_ok=True)
with ZipFile(output, "w", ZIP_DEFLATED) as archive:
    for path in sorted(site.rglob("*")):
        if path.is_file():
            archive.write(path, path.relative_to(site))
print(f"{output}: {output.stat().st_size:,} bytes")
