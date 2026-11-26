"""A rough software render of the pet's poses, for checking them where Blender is not at hand.

    python3 art/pet3d/soft_preview.py OUT_DIR [animation ...]

Needs numpy and Pillow. It uses the same model, poses and animations as build_pet.py and draws
them flat-shaded with a z-buffer: good for silhouettes, proportions and framing, nothing more.
With no animation named it writes OUT_DIR/keys.png (one key frame per animation); with names,
OUT_DIR/<animation>.png (a strip of its frames). Either way it prints each animation's bounds
in the pet's 96 x 80 box and marks any that leave it.
"""

import os
import sys

import numpy as np
from PIL import Image

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import build_pet as bp  # noqa: E402

COLORS = {
    "shell": (0.80, 0.62, 0.93), "graphite": (0.16, 0.17, 0.20), "metal": (0.62, 0.64, 0.68),
    "visor": (0.05, 0.06, 0.09), "ear_light": (0.4, 0.9, 1.0), "led_hi": (0.5, 0.95, 1.0),
    "led_lo": (0.15, 0.4, 0.5), "wood": (0.45, 0.30, 0.18), "paper": (0.92, 0.92, 0.88),
    "ink": (0.12, 0.22, 0.55), "screen": (0.25, 0.55, 0.85), "code": (0.7, 0.95, 1.0),
    "pencil": (0.95, 0.75, 0.15), "cover": (0.55, 0.12, 0.10),
}
EMIT = {"ear_light", "led_hi", "led_lo", "screen", "code"}
PARTS = bp.build_parts()
PROPS = bp.build_props()


def props_of(sets):
    return [p for p in PROPS if any(p.name.startswith("prop_%s_" % s) for s in sets)]
FACES = {}


skin = bp.skinned


def render(pose, face, size=(480, 400), view=None, scale_units=bp.FRAME_UNITS_W, bg=(58, 60, 66), target=None, props=()):
    """Draw a pose. Returns (image, touches_edge)."""
    W, H = size
    yaw, pitch = view if view else (bp.CAM_YAW, bp.CAM_PITCH)
    right, up, fwd = bp.camera_axes(yaw, pitch)
    target = np.array(bp.CAM_TARGET if target is None else target, float)
    ppu = W / scale_units
    light = bp.unit(np.array([0.35, -0.55, 0.75]))
    xf = bp.solve(pose)
    key = (face["shape"], face["lx"], face["ly"], face["sweep"])
    if key not in FACES:
        FACES[key] = bp.face_part(*key)
    color = np.zeros((H, W, 3))
    color[:] = np.array(bg) / 255.0
    depth = np.full((H, W), 1e9)
    cover = np.zeros((H, W), bool)
    for part in PARTS + props_of(props) + [FACES[key]]:
        if not len(part.verts):
            continue
        v = skin(part, xf) - target
        sx = v @ right * ppu + W / 2
        sy = H / 2 - v @ up * ppu
        sz = v @ fwd
        for fi, f in enumerate(part.faces):
            kind = part.slots[part.face_slot[fi]]
            base = np.array(COLORS[kind])
            for a, b, c in [(f[0], f[i], f[i + 1]) for i in range(1, len(f) - 1)]:
                n = np.cross(v[b] - v[a], v[c] - v[a])
                ln = np.linalg.norm(n)
                if ln < 1e-12:
                    continue
                n /= ln
                if n @ fwd > 0:   # faces away from the camera
                    continue
                x0, x1 = int(max(0, np.floor(min(sx[a], sx[b], sx[c])))), int(min(W - 1, np.ceil(max(sx[a], sx[b], sx[c]))))
                y0, y1 = int(max(0, np.floor(min(sy[a], sy[b], sy[c])))), int(min(H - 1, np.ceil(max(sy[a], sy[b], sy[c]))))
                if x1 < x0 or y1 < y0:
                    continue
                xs, ys = np.meshgrid(np.arange(x0, x1 + 1) + 0.5, np.arange(y0, y1 + 1) + 0.5)
                den = (sy[b] - sy[c]) * (sx[a] - sx[c]) + (sx[c] - sx[b]) * (sy[a] - sy[c])
                if abs(den) < 1e-12:
                    continue
                w0 = ((sy[b] - sy[c]) * (xs - sx[c]) + (sx[c] - sx[b]) * (ys - sy[c])) / den
                w1 = ((sy[c] - sy[a]) * (xs - sx[c]) + (sx[a] - sx[c]) * (ys - sy[c])) / den
                w2 = 1 - w0 - w1
                inside = (w0 >= -1e-4) & (w1 >= -1e-4) & (w2 >= -1e-4)
                z = w0 * sz[a] + w1 * sz[b] + w2 * sz[c]
                sub_d = depth[y0:y1 + 1, x0:x1 + 1]
                hit = inside & (z < sub_d)
                if not hit.any():
                    continue
                shade = 1.0 if kind in EMIT else 0.42 + 0.58 * max(0.0, float(n @ light))
                sub_d[hit] = z[hit]
                color[y0:y1 + 1, x0:x1 + 1][hit] = base * shade
                cover[y0:y1 + 1, x0:x1 + 1][hit] = True
    touches = bool(cover[0].any() or cover[-1].any() or cover[:, 0].any() or cover[:, -1].any())
    img = Image.fromarray((np.clip(color, 0, 1) * 255).astype(np.uint8))
    return img, touches


def extent(pose, props=()):
    """The pose's bounds in the pet's box, CSS px: (left, top, right, bottom)."""
    right, up, _ = bp.camera_axes()
    target = np.array(bp.CAM_TARGET)
    ppu = bp.FRAME_CSS[0] / bp.FRAME_UNITS_W
    xf = bp.solve(pose)
    lo, hi = np.array([1e9, 1e9]), np.array([-1e9, -1e9])
    for part in PARTS + props_of(props):
        v = skin(part, xf) - target
        xy = np.column_stack([v @ right * ppu + bp.FRAME_CSS[0] / 2, bp.FRAME_CSS[1] / 2 - v @ up * ppu])
        lo, hi = np.minimum(lo, xy.min(axis=0)), np.maximum(hi, xy.max(axis=0))
    return lo[0], lo[1], hi[0], hi[1]


def check_framing(names):
    for name in names:
        n = bp.ANIMATIONS[name][1]
        box = np.array([extent(bp.sample(name, bp.frame_time(name, i))[0], bp.props_for(name)) for i in range(n)])
        l, t, r, b = box[:, 0].min(), box[:, 1].min(), box[:, 2].max(), box[:, 3].max()
        bad = l < 1 or t < 1 or r > bp.FRAME_CSS[0] - 1 or b > bp.FRAME_CSS[1] - 0.5
        print("  %-9s x %5.1f..%5.1f  y %5.1f..%5.1f %s" % (name, l, r, t, b, " <-- outside the box" if bad else ""))


def strip(name, size=(240, 200), every=1):
    _, n, _, _, _ = bp.ANIMATIONS[name]
    frames = list(range(0, n, every))
    sheet = Image.new("RGB", (size[0] * len(frames), size[1]))
    for k, i in enumerate(frames):
        pose, face, _ = bp.sample(name, bp.frame_time(name, i))
        img, _ = render(pose, face, size, props=bp.props_for(name))
        sheet.paste(img, (k * size[0], 0))
    return sheet


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else "/tmp/pet3d_soft"
    names = sys.argv[2:] or list(bp.ANIMATIONS)
    os.makedirs(out, exist_ok=True)
    check_framing(names)
    if not sys.argv[2:]:
        cols = 5
        rows = (len(names) + cols - 1) // cols
        sheet = Image.new("RGB", (384 * cols, 320 * rows))
        for k, name in enumerate(names):
            pose, face, _ = bp.sample(name, bp.KEY_FRAME[name])
            img, _ = render(pose, face, (384, 320), props=bp.props_for(name))
            sheet.paste(img, (384 * (k % cols), 320 * (k // cols)))
        sheet.save(os.path.join(out, "keys.png"))
    else:
        for name in names:
            n = bp.ANIMATIONS[name][1]
            strip(name, every=max(1, n // 8)).save(os.path.join(out, name + ".png"))
    print("wrote", out)


if __name__ == "__main__":
    main()
