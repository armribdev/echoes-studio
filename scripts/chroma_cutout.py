from pathlib import Path
import subprocess
import sys

try:
    import numpy as np
except ImportError:
    subprocess.check_call([sys.executable, "-m", "pip", "install", "numpy", "-q"])
    import numpy as np

from PIL import Image

src_dir = Path(r"C:\Users\arman\.cursor\projects\c-Users-arman-dev-echoes-studio\assets")
out_dir = Path(r"c:\Users\arman\dev\echoes-studio\src\assets\instruments")
out_dir.mkdir(parents=True, exist_ok=True)

kinds = ["keys", "synth", "drums", "bass", "guitar", "strings", "vocals", "pad", "fx"]


def chroma_to_alpha(img: Image.Image) -> Image.Image:
    rgba = img.convert("RGBA")
    arr = np.asarray(rgba).astype(np.float32)
    r, g, b, a = arr[..., 0], arr[..., 1], arr[..., 2], arr[..., 3]
    greenness = g - np.maximum(r, b)
    mask = np.clip((greenness - 18.0) / 55.0, 0.0, 1.0)
    pure = (g > 90) & (g > r * 1.25) & (g > b * 1.25)
    mask = np.maximum(mask, pure.astype(np.float32) * 0.85)
    alpha = (a * (1.0 - mask)).astype(np.uint8)
    out = arr.copy()
    out[..., 3] = alpha
    spill = mask > 0.05
    out[spill, 1] = np.minimum(out[spill, 1], np.maximum(out[spill, 0], out[spill, 2]))
    result = Image.fromarray(out.astype(np.uint8), "RGBA")
    bbox = result.getbbox()
    if not bbox:
        return result
    pad = 8
    l, t, rgt, bot = bbox
    l = max(0, l - pad)
    t = max(0, t - pad)
    rgt = min(result.width, rgt + pad)
    bot = min(result.height, bot + pad)
    cropped = result.crop((l, t, rgt, bot))
    side = max(cropped.width, cropped.height)
    canvas = Image.new("RGBA", (side, side), (0, 0, 0, 0))
    canvas.paste(cropped, ((side - cropped.width) // 2, (side - cropped.height) // 2), cropped)
    return canvas.resize((256, 256), Image.Resampling.LANCZOS)


for kind in kinds:
    path = src_dir / f"chroma-{kind}.png"
    if not path.exists():
        print("missing", path)
        continue
    cut = chroma_to_alpha(Image.open(path))
    dest = out_dir / f"instrument-{kind}.png"
    cut.save(dest, "PNG")
    a = np.asarray(cut)[..., 3]
    print(f"{kind}: opaque={(a > 10).mean():.1%} -> {dest.name} {dest.stat().st_size}")
