import math
import numpy as np
from PIL import Image, ImageDraw, ImageFilter

def create_linear_gradient_np(width, height, stops, angle_deg=135):
    angle_rad = math.radians(angle_deg)
    cos_a = math.cos(angle_rad)
    sin_a = math.sin(angle_rad)

    x = np.arange(width, dtype=np.float32)
    y = np.arange(height, dtype=np.float32)
    xx, yy = np.meshgrid(x, y)

    proj = xx * cos_a + yy * sin_a
    min_p = proj.min()
    max_p = proj.max()
    p_norm = (proj - min_p) / (max_p - min_p if max_p > min_p else 1.0)

    out_arr = np.zeros((height, width, 4), dtype=np.uint8)

    for i in range(len(stops) - 1):
        p0, c0 = stops[i]
        p1, c1 = stops[i+1]
        mask = (p_norm >= p0) & (p_norm <= p1)
        factor = np.clip((p_norm - p0) / (p1 - p0 if p1 > p0 else 1.0), 0.0, 1.0)
        for c in range(4):
            v0 = c0[c] if len(c0) > c else 255
            v1 = c1[c] if len(c1) > c else 255
            channel_val = v0 + factor * (v1 - v0)
            out_arr[mask, c] = np.round(channel_val[mask]).astype(np.uint8)

    return Image.fromarray(out_arr, mode="RGBA")

def draw_icon():
    size = 1024
    scale = 2  # 2x supersampling is plenty sharp and executes in < 0.3s
    hr_size = size * scale
    m = 10.24 * scale

    # 1. Base Canvas
    raw_img = Image.new("RGBA", (hr_size, hr_size), (0, 0, 0, 0))

    # Squircle Background: Deep Luxury Dark Gradient (#090D16 -> #16192E)
    bg_grad = create_linear_gradient_np(hr_size, hr_size, [
        (0.0, (9, 13, 22, 255)),
        (1.0, (22, 25, 46, 255))
    ], angle_deg=135)

    hr_bg_mask = Image.new("L", (hr_size, hr_size), 0)
    draw_bg = ImageDraw.Draw(hr_bg_mask)
    draw_bg.rounded_rectangle([0, 0, hr_size - 1, hr_size - 1], radius=int(22 * m), fill=255)
    raw_img.paste(bg_grad, (0, 0), hr_bg_mask)

    # Subtle outer border
    draw_border = ImageDraw.Draw(raw_img)
    draw_border.rounded_rectangle([0, 0, hr_size - 1, hr_size - 1], radius=int(22 * m), outline=(42, 50, 70, 255), width=int(1.2 * m))

    # 2. Ambient Glow Behind the F
    glow_size = int(60 * m)
    glow_img = Image.new("RGBA", (glow_size, glow_size), (0, 0, 0, 0))
    draw_glow = ImageDraw.Draw(glow_img)
    draw_glow.ellipse([0, 0, glow_size - 1, glow_size - 1], fill=(56, 189, 248, 140))
    glow_blurred = glow_img.filter(ImageFilter.GaussianBlur(radius=int(18 * m)))
    raw_img.paste(glow_blurred, (int(20 * m), int(20 * m)), glow_blurred)

    # 3. Bold, Clean, Minimalist Iconic "F" Lettermark
    f_mask = Image.new("L", (hr_size, hr_size), 0)
    draw_f = ImageDraw.Draw(f_mask)
    f_w = int(12.5 * m)
    half_w = f_w // 2

    # Vertical spine
    draw_f.line([(32 * m, 26 * m), (32 * m, 74 * m)], fill=255, width=f_w)
    draw_f.ellipse([(32 * m - half_w, 26 * m - half_w), (32 * m + half_w, 26 * m + half_w)], fill=255)
    draw_f.ellipse([(32 * m - half_w, 74 * m - half_w), (32 * m + half_w, 74 * m + half_w)], fill=255)

    # Top horizontal arm
    draw_f.line([(32 * m, 26 * m), (72 * m, 26 * m)], fill=255, width=f_w)
    draw_f.ellipse([(72 * m - half_w, 26 * m - half_w), (72 * m + half_w, 26 * m + half_w)], fill=255)

    # Middle horizontal arm
    draw_f.line([(32 * m, 49 * m), (58 * m, 49 * m)], fill=255, width=f_w)
    draw_f.ellipse([(58 * m - half_w, 49 * m - half_w), (58 * m + half_w, 49 * m + half_w)], fill=255)

    # Vibrant Electric Gradient for the F: Cyan -> Electric Blue -> Violet (#38BDF8 -> #3B82F6 -> #818CF8)
    f_grad = create_linear_gradient_np(hr_size, hr_size, [
        (0.0, (56, 189, 248, 255)),   # Cyan
        (0.5, (59, 130, 246, 255)),   # Blue
        (1.0, (129, 140, 248, 255))   # Violet
    ], angle_deg=135)

    # F Shadow
    f_shadow = Image.new("RGBA", (hr_size, hr_size), (0, 0, 0, 0))
    draw_fs = ImageDraw.Draw(f_shadow)
    draw_fs.line([(32 * m, 28 * m), (32 * m, 76 * m)], fill=(0, 0, 0, 160), width=f_w)
    draw_fs.ellipse([(32 * m - half_w, 28 * m - half_w), (32 * m + half_w, 28 * m + half_w)], fill=(0, 0, 0, 160))
    draw_fs.ellipse([(32 * m - half_w, 76 * m - half_w), (32 * m + half_w, 76 * m + half_w)], fill=(0, 0, 0, 160))
    draw_fs.line([(32 * m, 28 * m), (72 * m, 28 * m)], fill=(0, 0, 0, 160), width=f_w)
    draw_fs.ellipse([(72 * m - half_w, 28 * m - half_w), (72 * m + half_w, 28 * m + half_w)], fill=(0, 0, 0, 160))
    draw_fs.line([(32 * m, 51 * m), (58 * m, 51 * m)], fill=(0, 0, 0, 160), width=f_w)
    draw_fs.ellipse([(58 * m - half_w, 51 * m - half_w), (58 * m + half_w, 51 * m + half_w)], fill=(0, 0, 0, 160))
    f_shadow = f_shadow.filter(ImageFilter.GaussianBlur(radius=int(6 * m)))
    raw_img.paste(f_shadow, (0, 0), f_shadow)

    # Paste F gradient
    raw_img.paste(f_grad, (0, 0), f_mask)

    # Subtle inner highlight / top rim on F
    draw_f_rim = ImageDraw.Draw(raw_img)
    draw_f_rim.line([(32 * m, 26 * m), (72 * m, 26 * m)], fill=(255, 255, 255, 180), width=int(1.8 * m))

    # 4. Standard macOS Dock Icon 82% Scaling with Ambient Drop Shadow
    target_hr_size = int(hr_size * 0.82)
    scaled_raw = raw_img.resize((target_hr_size, target_hr_size), Image.Resampling.LANCZOS)

    final_canvas = Image.new("RGBA", (hr_size, hr_size), (0, 0, 0, 0))
    offset = (hr_size - target_hr_size) // 2

    # Drop shadow
    shadow_mask = Image.new("RGBA", (hr_size, hr_size), (0, 0, 0, 0))
    shadow_raw = Image.new("RGBA", (target_hr_size, target_hr_size), (0, 0, 0, 180))
    shadow_mask.paste(shadow_raw, (offset, offset + int(8 * m)), scaled_raw.split()[3])
    shadow_blurred = shadow_mask.filter(ImageFilter.GaussianBlur(radius=int(14 * m)))
    final_canvas.paste(shadow_blurred, (0, 0), shadow_blurred)

    # Paste squircle
    final_canvas.paste(scaled_raw, (offset, offset), scaled_raw)

    out = final_canvas.resize((size, size), Image.Resampling.LANCZOS)
    out.save("app-icon.png")
    print("✅ app-icon.png generated successfully (Bold Minimalist F, 1024x1024)")

if __name__ == "__main__":
    draw_icon()
