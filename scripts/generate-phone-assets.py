#!/usr/bin/env python3
"""
Phone asset generator for MightyCode mobile app.
Generates all required assets from assets/icons/mightycode.png.

Usage (from repo root):
    python3 scripts/generate-phone-assets.py

Requires: Pillow (pip install Pillow)
"""

import hashlib
import json
import math
import sys
from pathlib import Path

try:
    from PIL import Image, ImageDraw
except ImportError:
    print("ERROR: Pillow is required. Run: pip install Pillow", file=sys.stderr)
    sys.exit(1)

REPO_ROOT = Path(__file__).parent.parent
SOURCE = REPO_ROOT / "assets" / "icons" / "mightycode.png"
OUT_DIR = REPO_ROOT / "mobile" / "assets" / "images"
SOURCES_JSON = OUT_DIR / "asset-sources.json"


def sha256_of(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def squircle_mask(size: int, supersample: int = 4) -> Image.Image:
    """Anti-aliased continuous-curvature squircle (superellipse, n = 4.85) alpha mask."""
    big = size * supersample
    mask = Image.new("L", (big, big), 0)
    draw = ImageDraw.Draw(mask)
    cx = cy = big / 2.0
    r = big / 2.0 - 0.5
    exp = 2.0 / 4.85
    steps = 256
    pts = []
    for i in range(steps):
        theta = (i / steps) * 2.0 * math.pi
        c, s = math.cos(theta), math.sin(theta)
        x = cx + r * math.copysign(abs(c) ** exp, c)
        y = cy + r * math.copysign(abs(s) ** exp, s)
        pts.append((x, y))
    draw.polygon(pts, fill=255)
    return mask.resize((size, size), Image.LANCZOS)


def clip_to_squircle(src: Image.Image, size: int) -> Image.Image:
    """Resize full-bleed square artwork to size×size and clip to a smooth squircle."""
    img = src.resize((size, size), Image.LANCZOS).convert("RGBA")
    img.putalpha(squircle_mask(size))
    return img


def edge_clamped_centered(src: Image.Image, canvas: int, draw_size: int) -> Image.Image:
    """Place full-bleed square src at draw_size centered in canvas×canvas, clamping edge pixels outward."""
    resized = src.resize((draw_size, draw_size), Image.LANCZOS).convert("RGBA")
    out = Image.new("RGBA", (canvas, canvas))
    offset = (canvas - draw_size) // 2
    # Fill top/bottom/left/right strips by stretching edge rows/columns seamlessly
    top_row = resized.crop((0, 0, draw_size, 1)).resize((draw_size, offset), Image.NEAREST)
    bot_row = resized.crop((0, draw_size - 1, draw_size, draw_size)).resize((draw_size, canvas - offset - draw_size), Image.NEAREST)
    out.paste(resized, (offset, offset))
    out.paste(top_row, (offset, 0))
    out.paste(bot_row, (offset, offset + draw_size))
    left_col = out.crop((offset, 0, offset + 1, canvas)).resize((offset, canvas), Image.NEAREST)
    right_col = out.crop((offset + draw_size - 1, 0, offset + draw_size, canvas)).resize((canvas - offset - draw_size, canvas), Image.NEAREST)
    out.paste(left_col, (0, 0))
    out.paste(right_col, (offset + draw_size, 0))
    return out


def make_ios_icon(src: Image.Image) -> dict:
    """1024×1024 RGB (no alpha) full-bleed square so iOS squircle fills edge-to-edge."""
    img = src.resize((1024, 1024), Image.LANCZOS).convert("RGB")
    img.save(OUT_DIR / "icon.png", "PNG")
    print("  icon.png: 1024×1024 RGB (no alpha, full-bleed)")
    return {"size": [1024, 1024], "mode": "RGB", "note": "iOS icon, no alpha, full-bleed square"}


def make_android_foreground(src: Image.Image) -> dict:
    """512×512 RGBA; raccoon comfortably inside Android's 66% safe zone with seamless full-bleed blue background."""
    inner = round(512 * 0.74)  # 379 px -> mascot sits inside 66% safe zone, blue fills all 512px
    img = edge_clamped_centered(src, 512, inner)
    img.save(OUT_DIR / "android-icon-foreground.png", "PNG")
    print(f"  android-icon-foreground.png: 512×512 RGBA (full-bleed blue, mascot in safe zone via {inner}px inner)")
    return {"size": [512, 512], "mode": "RGBA", "note": "Android adaptive foreground, full-bleed blue with mascot in safe zone"}


def make_android_monochrome(src: Image.Image) -> dict:
    """512×512 RGBA white silhouette (monochrome glyph) in the 66% safe zone."""
    safe = round(512 * 0.66)  # 338 px
    resized = src.resize((safe, safe), Image.LANCZOS).convert("RGBA")
    # Separate mascot & laptop from the blue radial/circuit background:
    # Background pixels have strong blue dominance (B > R + 35 and B > G + 15) and aren't the raccoon's
    # royal-blue suit (which sits lower-center surrounded by dark outlines). Build a clean squircle badge
    # with the mascot luminance silhouette inside, or flood-fill the outer blue background from all 4 edges.
    px = resized.load()
    w, h = resized.size
    visited = [[False] * w for _ in range(h)]
    stack = []
    def is_bg_blue(r: int, g: int, b: int) -> bool:
        return b >= 115 and (b - r) >= 45 and (b - g) >= 12 and r <= 155
    for x in range(w):
        for y in (0, h - 1):
            r, g, b, _ = px[x, y]
            if is_bg_blue(r, g, b) and not visited[y][x]:
                visited[y][x] = True
                stack.append((x, y))
    for y in range(h):
        for x in (0, w - 1):
            r, g, b, _ = px[x, y]
            if is_bg_blue(r, g, b) and not visited[y][x]:
                visited[y][x] = True
                stack.append((x, y))
    while stack:
        cx, cy = stack.pop()
        for nx, ny in ((cx - 1, cy), (cx + 1, cy), (cx, cy - 1), (cx, cy + 1)):
            if 0 <= nx < w and 0 <= ny < h and not visited[ny][nx]:
                r, g, b, _ = px[nx, ny]
                # Also allow crossing light cyan circuit traces inside the outer blue region
                is_cyan_trace = b >= 170 and g >= 165 and r >= 80 and (b - r) >= 30
                if is_bg_blue(r, g, b) or is_cyan_trace:
                    visited[ny][nx] = True
                    stack.append((nx, ny))
    glyph = Image.new("RGBA", (w, h), (255, 255, 255, 0))
    gpx = glyph.load()
    for y in range(h):
        for x in range(w):
            if not visited[y][x]:
                gpx[x, y] = (255, 255, 255, 255)
    canvas = Image.new("RGBA", (512, 512), (0, 0, 0, 0))
    offset = (512 - safe) // 2
    canvas.paste(glyph, (offset, offset), glyph)
    canvas.save(OUT_DIR / "android-icon-monochrome.png", "PNG")
    print("  android-icon-monochrome.png: 512×512 RGBA white mascot silhouette")
    return {"size": [512, 512], "mode": "RGBA", "note": "Android monochrome glyph, white mascot silhouette"}


def make_splash(src: Image.Image) -> dict:
    """400×400 RGBA squircle icon centred on transparent (expo-splash-screen composites the bg)."""
    img = clip_to_squircle(src, 400)
    img.save(OUT_DIR / "splash-icon.png", "PNG")
    print("  splash-icon.png: 400×400 RGBA squircle (expo-splash-screen adds bg)")
    return {"size": [400, 400], "mode": "RGBA", "note": "Splash image (squircle), expo-splash-screen adds background"}


def make_favicon(src: Image.Image) -> dict:
    """48×48 RGBA squircle web favicon."""
    img = clip_to_squircle(src, 48)
    img.save(OUT_DIR / "favicon.png", "PNG")
    print("  favicon.png: 48×48 RGBA squircle")
    return {"size": [48, 48], "mode": "RGBA", "note": "Web favicon (squircle)"}


def main() -> None:
    if not SOURCE.exists():
        print(f"ERROR: source not found: {SOURCE}", file=sys.stderr)
        sys.exit(1)

    OUT_DIR.mkdir(parents=True, exist_ok=True)

    print(f"Source: {SOURCE.relative_to(REPO_ROOT)}")
    source_hash = sha256_of(SOURCE)
    print(f"  sha256: {source_hash[:16]}…")

    src = Image.open(SOURCE).convert("RGBA")
    print(f"  size: {src.size}, mode: {src.mode}")

    print("\nGenerating assets:")
    outputs = {
        "icon.png": make_ios_icon(src),
        "android-icon-foreground.png": make_android_foreground(src),
        "android-icon-monochrome.png": make_android_monochrome(src),
        "splash-icon.png": make_splash(src),
        "favicon.png": make_favicon(src),
    }

    # Record what was written so a test can tell a regenerated file from a
    # hand-edited or stale one.
    for name, info in outputs.items():
        info["sha256"] = sha256_of(OUT_DIR / name)

    manifest = {
        "source": "assets/icons/mightycode.png",
        "source_sha256": source_hash,
        "outputs": outputs,
    }
    with open(SOURCES_JSON, "w") as f:
        json.dump(manifest, f, indent=2)
        f.write("\n")
    print(f"\n  asset-sources.json written")
    print("\nDone. All phone assets generated from raccoon artwork.")


if __name__ == "__main__":
    main()
