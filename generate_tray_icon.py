"""Generate Bento's monochrome 18 pt menu-bar template assets."""
import argparse
import json
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont


def icon(size):
    scale = 16
    factor = size * scale / 18
    alpha = Image.new("L", (size * scale, size * scale), 0)
    draw = ImageDraw.Draw(alpha)
    # Bento grid: a tall compartment on the left, two square ones on the right.
    for box in [(1, 1, 8, 17), (10, 1, 17, 8), (10, 10, 17, 17)]:
        draw.rounded_rectangle(
            tuple(round(value * factor) for value in box),
            radius=round(1.7 * factor), fill=255,
        )
    # Transparent prompt inside the terminal tile; every RGB channel stays black.
    draw.line([(round(x * factor), round(y * factor)) for x, y in [(11.7, 11.7), (13.2, 13.3), (11.7, 14.9)]], fill=0, width=round(factor), joint="curve")
    draw.line([(round(x * factor), round(14.9 * factor)) for x in [14.0, 15.6]], fill=0, width=round(factor))
    alpha = alpha.resize((size, size), Image.Resampling.LANCZOS)
    result = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    result.putalpha(alpha)
    return result


def preview(path, images):
    canvas = Image.new("RGB", (640, 240), "white")
    draw = ImageDraw.Draw(canvas)
    font = ImageFont.load_default()
    for column, (background, foreground, title) in enumerate([
        ("#f6f6f6", "#161616", "Light menu bar"),
        ("#202124", "#fafafa", "Dark menu bar (template tint)"),
    ]):
        left = column * 320
        draw.rectangle((left, 0, left + 319, 239), fill=background)
        draw.text((left + 20, 18), title, fill=foreground, font=font)
        for row, (size, image) in enumerate(images.items()):
            tinted = Image.new("RGBA", image.size, foreground)
            tinted.putalpha(image.getchannel("A"))
            canvas.paste(tinted, (left + 24, 64 + row * 72), tinted)
            enlarged = tinted.resize((size * 3, size * 3), Image.Resampling.NEAREST)
            canvas.paste(enlarged, (left + 125, 48 + row * 72), enlarged)
            draw.text((left + 24, 108 + row * 72), f"{size} x {size} px", fill=foreground, font=font)
    path.parent.mkdir(parents=True, exist_ok=True)
    canvas.save(path)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--preview", type=Path)
    args = parser.parse_args()
    destination = Path(__file__).resolve().parent / "src-tauri/icons"
    images = {}
    records = []
    for size, name in [(18, "tray-icon.png"), (36, "tray-icon@2x.png")]:
        image = icon(size)
        path = destination / name
        image.save(path)
        assert all(red == green == blue == 0 for red, green, blue, _ in image.getdata())
        assert image.getchannel("A").getextrema() == (0, 255)
        images[size] = image
        records.append({"path": str(path), "size": list(image.size), "blackRgbOnly": True, "alphaRange": [0, 255], "bytes": path.stat().st_size})
    if args.preview:
        preview(args.preview, images)
    print(json.dumps({"icons": records, "preview": str(args.preview) if args.preview else None}, indent=2))
