"""Catbot desktop pet, 3D: a slender robot cat built, rigged, animated and rendered in Blender.
It also renders the app icon (the same cat's head) with {"mode": "icon"}.

Run (Blender 4.0 or newer, nothing but what Blender ships):

    blender -b --factory-startup --python-exit-code 1 -P art/pet3d/build_pet.py -- \
        --config art/pet3d/render.json --out art/pet3d/renders

What it does, top to bottom:

1. MODEL (plain numpy, no bpy): every part is a "tube" (a lofted, rounded cross-section along a
   path), built in the rest pose in world coordinates and bound to a bone.
2. POSES (plain numpy): a pose is a flat dict of numbers (body position, spine angles, paw
   targets, tail angles). `solve()` turns it into one world transform per bone; the legs are
   solved with two-bone IK so paws stay where the pose puts them.
3. ANIMATIONS (plain numpy): one function per animation, giving the pose, the LED face and the
   ear lights for every frame.
4. BLENDER: meshes, armature, materials, lights, orthographic camera; then for every frame it
   poses the rig and renders the passes (base, tint mask, gloss), and joins the frames into
   sprite sheets with a manifest. A preview mode renders stills and a contact sheet instead.

Parts 1-3 import without Blender (see soft_preview.py, a rough software renderer used to check
poses where Blender is not at hand).

Axes: +X is forward (the cat faces right), +Z is up, the camera sits on the -Y side, so -Y is the
"near" side and +Y the "far" side. Angles are degrees. A positive pitch turns clockwise as the
camera sees it (nose down, tail up, a hanging leg swinging back).
"""

import json
import math
import os
import struct
import sys
import time
import zlib

import numpy as np

try:  # Blender only; the model, poses and animations below work without it.
    import bpy
    import bmesh
    from mathutils import Matrix, Vector
except ImportError:  # pragma: no cover - outside Blender
    bpy = None


# =============================================================================================
# Frame and camera
# =============================================================================================

FRAME_CSS = (96, 80)      # the pet's box in the pet window, CSS px
FRAME_UNITS_W = 10.6      # world units across the frame's width
FRAME_UNITS_H = FRAME_UNITS_W * FRAME_CSS[1] / FRAME_CSS[0]
GROUND_MARGIN = 0.28      # world units between the frame's bottom edge and the ground
CAM_TARGET = (-0.62, 0.0, FRAME_UNITS_H / 2 - GROUND_MARGIN)
CAM_YAW = 18.0            # the camera stands a little towards the cat's front, so the face shows
CAM_PITCH = 6.0           # and a little above

VIEWS = {  # name: (yaw, pitch) for the preview stills
    "hero": (42.0, 16.0),
    "front": (90.0, 4.0),
    "side": (0.0, 0.0),
}


def camera_axes(yaw=CAM_YAW, pitch=CAM_PITCH):
    """(right, up, forward) unit vectors of a camera that looks at the cat from the -Y side,
    turned `yaw` degrees towards the cat's front and raised `pitch` degrees."""
    y, p = math.radians(yaw), math.radians(pitch)
    to_cam = np.array([math.sin(y) * math.cos(p), -math.cos(y) * math.cos(p), math.sin(p)])
    fwd = -to_cam
    right = np.cross(fwd, [0.0, 0.0, 1.0])
    right /= np.linalg.norm(right)
    up = np.cross(right, fwd)
    return right, up, fwd


# =============================================================================================
# Small math
# =============================================================================================

def _rot(axis, deg):
    c, s = math.cos(math.radians(deg)), math.sin(math.radians(deg))
    if axis == 0:
        return np.array([[1, 0, 0], [0, c, -s], [0, s, c]], float)
    if axis == 1:
        return np.array([[c, 0, s], [0, 1, 0], [-s, 0, c]], float)
    return np.array([[c, -s, 0], [s, c, 0], [0, 0, 1]], float)


def RX(d):
    return _rot(0, d)


def RY(d):
    return _rot(1, d)


def RZ(d):
    return _rot(2, d)


def euler(roll, pitch, yaw):
    return RZ(yaw) @ RY(pitch) @ RX(roll)


def unit(v):
    v = np.asarray(v, float)
    n = np.linalg.norm(v)
    return v / n if n > 1e-9 else v


def basis(d, side=(0.0, 1.0, 0.0)):
    """A frame whose first axis is `d` and whose second is as near `side` as it can be."""
    x = unit(d)
    s = np.asarray(side, float) - np.dot(side, x) * x
    if np.linalg.norm(s) < 1e-6:
        s = np.array([0.0, 0.0, 1.0]) - x[2] * x
    s = unit(s)
    return np.column_stack([x, s, np.cross(x, s)])


def smooth(t):
    t = min(1.0, max(0.0, t))
    return t * t * (3 - 2 * t)


def lerp(a, b, t):
    return a + (b - a) * t


# =============================================================================================
# Skeleton (rest pose)
# =============================================================================================

TAIL_N = 8
TAIL_SEG = 0.42
LEGS_FRONT = ("fn", "ff")   # front near, front far
LEGS_HIND = ("hn", "hf")
LEG_Y = {"fn": -0.48, "ff": 0.48, "hn": -0.50, "hf": 0.50}

BZ = 2.92        # the height of the spine at the hips; the body is laid out from here
WRIST_H = 0.34   # the wrist's height over the paw's contact point
ANKLE_H = 0.30   # the hind paw joint's height over its contact point

# bone: (parent, head, tail) -- the tail only gives the bone a direction to look at in Blender.
BONES = {
    "root": (None, (0.0, 0.0, 0.0), (0.6, 0.0, 0.0)),
    "hips": ("root", (-1.6, 0.0, BZ), (-2.4, 0.0, BZ + 0.10)),
    "waist": ("hips", (-0.85, 0.0, BZ + 0.03), (0.2, 0.0, BZ + 0.03)),
    "chest": ("waist", (0.2, 0.0, BZ + 0.03), (1.4, 0.0, BZ + 0.30)),
    "neck": ("chest", (1.4, 0.0, BZ + 0.35), (2.0, 0.0, BZ + 0.95)),
    "head": ("neck", (2.0, 0.0, BZ + 0.95), (3.0, 0.0, BZ + 1.35)),
    "ear_n": ("head", (2.40, -0.66, BZ + 1.92), (2.22, -1.05, BZ + 3.05)),
    "ear_f": ("head", (2.40, 0.66, BZ + 1.92), (2.22, 1.05, BZ + 3.05)),
}
TAIL_BASE = (-2.50, 0.0, BZ + 0.12)
for _i in range(TAIL_N):
    _h = (TAIL_BASE[0] - TAIL_SEG * _i, 0.0, TAIL_BASE[2])
    BONES["tail%d" % (_i + 1)] = (
        "hips" if _i == 0 else "tail%d" % _i, _h, (_h[0] - TAIL_SEG, 0.0, _h[2]))
for _leg in LEGS_FRONT:
    _y = LEG_Y[_leg]
    BONES["scap_" + _leg] = ("chest", (0.95, _y, BZ + 0.20), (0.95, _y, BZ - 0.30))
    BONES["upper_" + _leg] = ("scap_" + _leg, (0.95, _y, BZ - 0.30), (0.68, _y, 1.50))
    BONES["lower_" + _leg] = ("upper_" + _leg, (0.68, _y, 1.50), (0.95, _y, WRIST_H))
    BONES["paw_" + _leg] = ("lower_" + _leg, (0.95, _y, WRIST_H), (1.36, _y, 0.10))
for _leg in LEGS_HIND:
    _y = LEG_Y[_leg]
    BONES["thigh_" + _leg] = ("hips", (-1.7, _y, BZ - 0.25), (-1.36, _y, 1.66))
    BONES["shin_" + _leg] = ("thigh_" + _leg, (-1.36, _y, 1.66), (-2.06, _y, 1.08))
    BONES["meta_" + _leg] = ("shin_" + _leg, (-2.06, _y, 1.08), (-1.92, _y, ANKLE_H))
    BONES["paw_" + _leg] = ("meta_" + _leg, (-1.92, _y, ANKLE_H), (-1.50, _y, 0.10))


def head_of(bone):
    return np.array(BONES[bone][1], float)


SCRUFF = np.array([1.15, 0.0, BZ + 0.64])   # where a hand holds the cat when it is carried (on the chest)


# =============================================================================================
# Meshes: everything is a tube
# =============================================================================================

class Part:
    """One mesh in the rest pose. `slots` are its material kinds; `face_slot` picks one per face.
    `weights` is {bone: weight per vertex} (a rigid part has one bone, all ones)."""

    def __init__(self, name, verts, faces, slots, weights, face_slot=None, closed=True):
        self.name = name
        self.verts = np.asarray(verts, float)
        self.faces = faces
        self.slots = list(slots)
        self.weights = weights
        self.face_slot = face_slot or [0] * len(faces)
        self.closed = closed


def _path_sampler(path):
    """Points and tangents along a polyline, by arc length; three or more points are rounded
    off with a Catmull-Rom curve."""
    pts = np.asarray(path, float)
    if len(pts) > 2:
        ext = np.vstack([2 * pts[0] - pts[1], pts, 2 * pts[-1] - pts[-2]])
        dense = []
        for i in range(len(pts) - 1):
            p0, p1, p2, p3 = ext[i], ext[i + 1], ext[i + 2], ext[i + 3]
            for u in np.linspace(0, 1, 12, endpoint=False):
                dense.append(0.5 * ((2 * p1) + (-p0 + p2) * u + (2 * p0 - 5 * p1 + 4 * p2 - p3) * u * u
                                    + (-p0 + 3 * p1 - 3 * p2 + p3) * u ** 3))
        dense.append(pts[-1])
        pts = np.array(dense)
    seg = np.linalg.norm(np.diff(pts, axis=0), axis=1)
    acc = np.concatenate([[0.0], np.cumsum(seg)])
    acc /= acc[-1]

    def at(t):
        return np.array([np.interp(t, acc, pts[:, k]) for k in range(3)])

    def tangent(t):
        return unit(at(min(1.0, t + 1e-3)) - at(max(0.0, t - 1e-3)))

    return at, tangent


def tube(path, prof, n_len=12, n_seg=18, round0=0.3, round1=0.3, power=2.0, t0=0.0, t1=1.0,
         side=(0.0, 1.0, 0.0), shift=(0.0, 0.0), grow=0.0, floor=None):
    """A lofted tube along `path`.

    prof: [(t, half_width, half_up[, half_down])] cross-section sizes along the path; width is
          along `side`, up/down along the path's normal (up when the path runs forward).
    round0/round1: how much of each end is rounded into a dome (a fraction of the length).
    power: 2 is an ellipse, more is squarer.
    t0..t1: build only this stretch of the tube (cut ends are flat).
    shift: move the cross-sections sideways/up; grow: add to every radius.
    floor: flatten everything below this height (paw soles).
    Returns (verts, faces) with faces wound to face outwards."""
    at, tangent = _path_sampler(path)
    prof = np.array([(p[0], p[1], p[2], p[3] if len(p) > 3 else p[2]) for p in prof], float)
    side = np.asarray(side, float)

    def dome(t):
        k = 1.0
        if round0 > 0 and t < round0:
            k *= math.sqrt(max(0.0, 1 - (1 - t / round0) ** 2))
        if round1 > 0 and t > 1 - round1:
            k *= math.sqrt(max(0.0, 1 - (1 - (1 - t) / round1) ** 2))
        return k

    pole0 = round0 > 0 and t0 <= 0.0   # the end closes to a point
    pole1 = round1 > 0 and t1 >= 1.0
    us = np.linspace(0, 1, n_len + 1)
    us = 0.55 * (0.5 - 0.5 * np.cos(np.pi * us)) + 0.45 * us   # a little denser at the ends
    ts = [t0 + (t1 - t0) * u for u in us]
    rings = ts[(1 if pole0 else 0):(len(ts) - 1 if pole1 else len(ts))]

    verts, faces = [], []
    angles = [2 * math.pi * j / n_seg for j in range(n_seg)]

    def ring(t):
        c, tan = at(t), tangent(t)
        s = unit(side - np.dot(side, tan) * tan)
        n = np.cross(tan, s)
        a = np.interp(t, prof[:, 0], prof[:, 1])
        bu = np.interp(t, prof[:, 0], prof[:, 2])
        bd = np.interp(t, prof[:, 0], prof[:, 3])
        k = dome(t)
        out = []
        for th in angles:
            cs, sn = math.cos(th), math.sin(th)
            e = 2.0 / power
            cs2 = math.copysign(abs(cs) ** e, cs)
            sn2 = math.copysign(abs(sn) ** e, sn)
            b = bu if sn >= 0 else bd
            out.append(c + s * ((a * k + grow) * cs2 + shift[0]) + n * ((b * k + grow) * sn2 + shift[1]))
        return out

    for t in rings:
        verts.extend(ring(t))
    nr = len(rings)
    for i in range(nr - 1):
        for j in range(n_seg):
            j2 = (j + 1) % n_seg
            faces.append((i * n_seg + j, i * n_seg + j2, (i + 1) * n_seg + j2, (i + 1) * n_seg + j))

    def cap(first):
        t = t0 if first else t1
        base = len(verts)
        if (pole0 if first else pole1):
            c = at(t)
            tan, s = tangent(t), None
            s = unit(side - np.dot(side, tan) * tan)
            verts.append(c + s * shift[0] + np.cross(tan, s) * shift[1])
            r0 = 0 if first else (nr - 1) * n_seg
            idx = [r0 + j for j in range(n_seg)]
            centre = base
        else:  # a flat cut: its own ring, so the rim stays crisp
            verts.extend(ring(t))
            verts.append(np.mean(verts[base:base + n_seg], axis=0))
            idx = [base + j for j in range(n_seg)]
            centre = base + n_seg
        for j in range(n_seg):
            a, b = idx[j], idx[(j + 1) % n_seg]
            faces.append((centre, b, a) if first else (centre, a, b))

    cap(True)
    cap(False)
    verts = np.array(verts)
    if floor is not None:
        verts[:, 2] = np.maximum(verts[:, 2], floor)
    return verts, faces


def ball(c, r, n=12):
    r3 = (r, r, r) if np.isscalar(r) else r
    c = np.asarray(c, float)
    return tube([c - [r3[0], 0, 0], c + [r3[0], 0, 0]], [(0, r3[1], r3[2]), (1, r3[1], r3[2])],
                n_len=n, n_seg=max(10, n + 4), round0=0.5, round1=0.5)


def rigid(bone, n):
    return {bone: np.ones(n)}


def chain_weights(param, joints, width):
    """Smooth weights along a chain: `joints` is [(bone_before, at_param), ..., (last_bone, None)];
    each joint blends over +-width of the vertex's `param`."""
    names = [j[0] for j in joints]
    w = {n: np.zeros(len(param)) for n in names}
    for i, p in enumerate(param):
        left = 1.0
        for k in range(len(joints) - 1):
            s = smooth((p - (joints[k][1] - width)) / (2 * width))
            w[names[k]][i] = left * (1 - s)
            left *= s
        w[names[-1]][i] = left
    return w


# ---- the head is an ellipsoid; the visor and the LEDs are laid on its surface -----------------

HEAD_C = np.array([2.66, 0.0, BZ + 1.40])
HEAD_R = np.array([1.08, 1.16, 0.93])
VISOR_PHI = 10.0          # the visor's centre, degrees above straight ahead
VISOR_HALF = (70.0, 29.0)  # half-extent: side to side, and up/down
EYE_LAT = 37.0            # each eye's centre, degrees to the side of straight ahead
EYE_PHI = 11.0
EYE_PITCH = 3.4           # degrees between LED centres
EYE_GRID = 5              # LEDs run -5..5 each way: an 11 x 11 matrix per eye


def on_head(lat, phi, k=1.0):
    """A point on the head's surface (scaled by k): `lat` degrees towards the far side (+Y),
    `phi` degrees up from straight ahead."""
    la, ph = math.radians(lat), math.radians(phi)
    d = np.array([math.cos(la) * math.cos(ph), math.sin(la), math.cos(la) * math.sin(ph)])
    return HEAD_C + k * HEAD_R * d


def head_patch(half, k, n_r=14, n_s=64, power=5.0):
    """A rounded-rectangle patch on the head, centred on the visor."""
    verts, faces = [on_head(0, VISOR_PHI, k)], []
    for i in range(1, n_r + 1):
        rho = i / n_r
        for j in range(n_s):
            th = 2 * math.pi * j / n_s
            cs, sn = math.cos(th), math.sin(th)
            e = 2.0 / power
            a = math.copysign(abs(cs) ** e, cs) * rho * half[0]
            b = math.copysign(abs(sn) ** e, sn) * rho * half[1]
            verts.append(on_head(a, VISOR_PHI + b, k))
    for j in range(n_s):
        faces.append((0, 1 + j, 1 + (j + 1) % n_s))
    for i in range(n_r - 1):
        for j in range(n_s):
            a, b = 1 + i * n_s + j, 1 + i * n_s + (j + 1) % n_s
            faces.append((a, a + n_s, b + n_s, b))
    return np.array(verts), _face_out(np.array(verts), faces, HEAD_C)


def _face_out(verts, faces, centre):
    """Wind every face so it looks away from `centre`."""
    out = []
    for f in faces:
        p = verts[list(f[:3])]
        n = np.cross(p[1] - p[0], p[2] - p[0])
        out.append(tuple(f) if np.dot(n, p.mean(axis=0) - centre) >= 0 else tuple(reversed(f)))
    return out


# ---- LED face ---------------------------------------------------------------------------------

def eye_pixels(shape, lx=0, ly=0, sweep=None):
    """The lit LEDs of one eye: {(col, row): level}, level 2 bright, 1 dim. Columns run towards
    the far side, rows up. `lx`, `ly` shift the shape (where it looks); `sweep` is the bright
    column of the "working" eye."""
    lit = {}
    g = EYE_GRID
    for c in range(-g, g + 1):
        for r in range(-g, g + 1):
            x, y = c - lx, r - ly
            on = False
            if shape == "open":        # content: a tall soft oval
                on = (x / 3.3) ** 2 + (y / 4.3) ** 2 <= 1.0
            elif shape == "wide":      # curious / needs you: a big round eye
                on = (x / 4.5) ** 2 + (y / 4.5) ** 2 <= 1.0
            elif shape == "half":      # focused: the oval with a flat lid
                on = (x / 3.6) ** 2 + (y / 4.3) ** 2 <= 1.0 and -3.2 <= y <= 1.2
            elif shape == "squint":    # a blink half way
                on = (x / 3.5) ** 2 + (y / 1.9) ** 2 <= 1.0
            elif shape == "blink":
                on = abs(y) <= 0.5 and abs(x) <= 3.5
            elif shape == "sleepy":    # closed, relaxed: an arc that sags in the middle
                on = abs(x) <= 3.6 and abs(y - (-1.8 + 1.7 * (x / 3.6) ** 2)) <= 0.62
            elif shape == "happy":     # an arch
                on = abs(x) <= 3.7 and abs(y - (1.6 - 2.6 * (x / 3.7) ** 2)) <= 0.85
            elif shape == "squeeze":   # shut tight (a yawn): a thin line, its ends pulled down
                on = abs(x) <= 3.8 and abs(y - (0.4 - 1.5 * (x / 3.8) ** 2)) <= 0.5
            else:
                raise ValueError("unknown eye shape %r" % shape)
            if on:
                lit[(c, r)] = 2
    if sweep is not None:
        for (c, r) in list(lit):
            lit[(c, r)] = 2 if abs(c - sweep) <= 0.5 else 1
    return lit


def face_part(shape, lx=0, ly=0, sweep=None, big=False):
    """Both eyes as one mesh of little LED squares on the visor: slot 0 bright, slot 1 dim.
    `big` makes the eyes a quarter larger with no gaps between the LEDs, for the smallest icons,
    where the matrix would only be noise."""
    lit = eye_pixels(shape, lx, ly, sweep)
    verts, faces, slot = [], [], []
    pitch = EYE_PITCH * (1.25 if big else 1.0)
    h = (0.51 if big else 0.43) * pitch
    dphi = 1.0 / math.cos(math.radians(EYE_LAT))
    for sign in (-1, 1):
        for (c, r), level in sorted(lit.items()):
            la = sign * EYE_LAT + c * pitch
            ph = EYE_PHI + r * pitch * dphi
            b = len(verts)
            for da, dp in ((-h, -h), (h, -h), (h, h), (-h, h)):
                verts.append(on_head(la + da, ph + dp * dphi, 1.056))
            faces.append((b, b + 1, b + 2, b + 3))
            slot.append(0 if level == 2 else 1)
    verts = np.array(verts) if verts else np.zeros((0, 3))
    faces = _face_out(verts, faces, HEAD_C)
    name = "face_%s_%d_%d_%s%s" % (shape, lx, ly, "x" if sweep is None else sweep, "_big" if big else "")
    return Part(name, verts, faces, ["led_hi", "led_lo"], rigid("head", len(verts)), slot, closed=False)


# ---- the body ---------------------------------------------------------------------------------

def _ear(sgn):
    """An ear's tube: (path start, tip, profile, tube options). sgn -1 is the near ear."""
    base = HEAD_C + [-0.22, sgn * 0.62, 0.52]
    tip = HEAD_C + [-0.46, sgn * 1.12, 1.92]
    ear_side = (0.66, sgn * 0.75, 0.0)   # the ear's flat faces forward and outward
    prof = [(0, 0.64, 0.25), (0.5, 0.40, 0.17), (1, 0.13, 0.09)]
    kw = dict(n_seg=14, round0=0.0, round1=0.14, side=ear_side)
    return base - 0.22 * (tip - base), tip, prof, kw   # the path starts inside the head


def icon_parts():
    """Extra meshes used only by the smallest app icons: ear lights that take the top third of
    each ear, so they survive at 16 px. Hidden everywhere else."""
    parts = []
    for tag, sgn in (("n", -1.0), ("f", 1.0)):
        lower, tip, prof, kw = _ear(sgn)
        v, f = tube([lower, tip], prof, n_len=8, t0=0.64, t1=1.0, grow=0.012, **kw)
        parts.append(Part("ear_biglight_%s" % tag, v, f, ["ear_light"], rigid("ear_" + tag, len(v))))
    return parts


# ---- props: the things a cat works at (stage 29.8) ---------------------------------------------

def box(c, size, R=None, pivot=None):
    """A box: centre `c`, full sizes `size`, turned by `R` about `pivot` (default: its centre).
    Returns (verts, faces) wound outwards."""
    c = np.asarray(c, float)
    hx, hy, hz = (np.asarray(size, float) / 2).tolist()
    corners = np.array([[sx * hx, sy * hy, sz * hz] for sz in (-1, 1) for sy in (-1, 1) for sx in (-1, 1)]) + c
    if R is not None:
        o = c if pivot is None else np.asarray(pivot, float)
        corners = (corners - o) @ np.asarray(R).T + o
    faces = [(0, 2, 3, 1), (4, 5, 7, 6), (0, 1, 5, 4), (2, 6, 7, 3), (0, 4, 6, 2), (1, 3, 7, 5)]
    return corners, _face_out(corners, faces, corners.mean(axis=0))


def _local(origin, R):
    """A function placing points given in a prop's own frame (origin, turn R) in the world."""
    origin, R = np.asarray(origin, float), np.asarray(R, float)
    return lambda v: origin + R @ np.asarray(v, float)


# The desk the cat codes at: a low table with a laptop turned towards the camera.
DESK_TOP = 1.50                       # the desk's top surface (world z)
LAPTOP_YAW = 50.0                     # the screen faces back towards the cat and the camera
LAPTOP_HINGE = np.array([3.00, 0.30, DESK_TOP])
LAPTOP_R = RZ(LAPTOP_YAW)
KEYS_AT = LAPTOP_HINGE + LAPTOP_R @ np.array([-0.62, 0.0, 0.10])   # the middle of the keyboard

# The drafting table the cat designs at: a board sloping up away from it, turned (like the
# laptop) so its face shows to the camera.
BOARD_TILT = 38.0
BOARD_YAW = 34.0
BOARD_LOW = np.array([1.78, -0.10, 1.40])  # the board's near (low) edge, middle
BOARD_R = RZ(BOARD_YAW) @ RY(-BOARD_TILT)  # pitched so the far edge rises

# The book the cat reads: open, propped up on a little stand, turned towards it and the camera.
BOOK_AT = np.array([2.70, -0.05, 0.16])    # the bottom of the spine
BOOK_R = RZ(48.0) @ RY(-28.0)              # leaning back, away from the cat


def build_props():
    """The props, each named prop_<set>_<piece>: "desk" (coding), "drafting" (designing), "book"
    (researching). They stand still in the world, except the pencil, held in the near front paw.
    Hidden unless an animation asks for its set (ANIM_PROPS)."""
    parts = []

    def add(name, vf, kind, bone="root"):
        v, f = vf
        parts.append(Part(name, v, f, [kind], rigid(bone, len(v))))

    # -- desk: top, four legs; the laptop: base, keyboard, lid, screen, a glowing line of code
    add("prop_desk_top", box((3.20, 0.15, DESK_TOP - 0.07), (2.50, 2.00, 0.14)), "wood")
    for k, (x, y) in enumerate(((2.08, -0.72), (4.32, -0.72), (2.08, 1.02), (4.32, 1.02))):
        add("prop_desk_leg%d" % k, box((x, y, (DESK_TOP - 0.14) / 2), (0.15, 0.15, DESK_TOP - 0.14)), "wood")
    at = _local(LAPTOP_HINGE, LAPTOP_R)
    add("prop_desk_laptop", box(at((-0.60, 0.0, 0.04)), (1.20, 1.56, 0.08), LAPTOP_R), "graphite")
    add("prop_desk_keys", box(at((-0.66, 0.0, 0.09)), (0.78, 1.32, 0.02), LAPTOP_R), "metal")
    lid_R = LAPTOP_R @ RY(-14.0)           # leaning back a little, away from the cat
    lid = _local(LAPTOP_HINGE + [0, 0, 0.08], lid_R)
    add("prop_desk_lid", box(lid((0.03, 0.0, 0.60)), (0.06, 1.56, 1.20), lid_R), "graphite")
    add("prop_desk_screen", box(lid((-0.006, 0.0, 0.62)), (0.012, 1.40, 1.04), lid_R), "screen")
    for k, (y, z, w) in enumerate(((-0.30, 0.95, 0.56), (-0.18, 0.80, 0.80), (-0.24, 0.65, 0.40), (-0.12, 0.50, 0.70))):
        add("prop_desk_code%d" % k, box(lid((-0.016, y + w / 2 - 0.3, z)), (0.006, w, 0.05), lid_R), "code")

    # -- drafting table: a sloping board on two legs, a sheet of paper, a drawing of a house
    brd = _local(BOARD_LOW, BOARD_R)
    add("prop_drafting_board", box(brd((0.95, 0.0, -0.05)), (1.90, 1.90, 0.10), BOARD_R), "wood")
    add("prop_drafting_ledge", box(brd((0.02, 0.0, 0.05)), (0.08, 1.90, 0.10), BOARD_R), "wood")
    for k, (u, v) in enumerate(((0.25, -0.80), (0.25, 0.80), (1.65, -0.80), (1.65, 0.80))):
        top = brd((u, v, -0.10))
        add("prop_drafting_leg%d" % k, box((top[0], top[1], top[2] / 2), (0.10, 0.10, top[2])), "wood")
    add("prop_drafting_paper", box(brd((1.00, 0.0, 0.015)), (1.50, 1.50, 0.02), BOARD_R), "paper")
    # the drawing, on the paper: walls, a roof, a door, a window, a ground line
    lines = [((0.55, -0.42), (0.55, 0.42)), ((1.25, -0.42), (1.25, 0.42)), ((0.55, -0.42), (1.25, -0.42)),
             ((0.55, 0.42), (1.25, 0.42)), ((1.25, -0.50), (1.62, 0.0)), ((1.25, 0.50), (1.62, 0.0)),
             ((0.55, -0.10), (0.85, -0.10)), ((0.55, 0.10), (0.85, 0.10)), ((0.85, -0.10), (0.85, 0.10)),
             ((0.95, 0.18), (1.12, 0.18)), ((0.95, 0.34), (1.12, 0.34)), ((0.95, 0.18), (0.95, 0.34)),
             ((1.12, 0.18), (1.12, 0.34)), ((0.40, -0.62), (0.40, 0.62))]
    for k, ((x0, y0), (x1, y1)) in enumerate(lines):
        a, b = np.array([x0, y0]), np.array([x1, y1])
        mid, d = (a + b) / 2, b - a
        turn = RZ(math.degrees(math.atan2(d[1], d[0])))
        R = BOARD_R @ turn
        add("prop_drafting_line%d" % k, box(brd((mid[0] - 0.05, mid[1], 0.03)), (float(np.linalg.norm(d)) + 0.03, 0.04, 0.012), R), "ink")
    # the pencil, in the near front paw (rest pose: held under the paw, point forward and down)
    paw = head_of("paw_fn")
    v, f = tube([paw + [0.05, -0.20, -0.02], paw + [0.62, -0.20, -0.30]], [(0, 0.055, 0.055), (0.85, 0.055, 0.055), (1, 0.012, 0.012)],
                n_len=4, n_seg=8, round0=0.1, round1=0.0)
    add("prop_drafting_pencil", (v, f), "pencil", bone="paw_fn")

    # -- book: open, leaning on a little stand; two pages of text, a red cover, a ribbon
    bk = _local(BOOK_AT, BOOK_R)
    for k, sgn in enumerate((-1, 1)):
        half = BOOK_R @ RZ(-sgn * 12.0)          # the halves open in a shallow V
        hb = _local(BOOK_AT, half)
        add("prop_book_cover%d" % k, box(hb((0.05, sgn * 0.56, 0.62)), (0.05, 1.10, 1.30), half), "cover")
        add("prop_book_pages%d" % k, box(hb((-0.02, sgn * 0.53, 0.62)), (0.08, 1.02, 1.20), half), "paper")
        for j in range(6):
            w = 0.78 if j % 3 else 0.56
            add("prop_book_text%d_%d" % (k, j), box(hb((-0.07, sgn * (0.12 + w / 2), 1.02 - 0.16 * j)), (0.01, w, 0.045), half), "ink")
    add("prop_book_ribbon", box(bk((-0.06, 0.02, 0.20)), (0.01, 0.06, 0.70), BOOK_R), "cover")
    add("prop_book_stand", box((BOOK_AT[0] + 0.35, BOOK_AT[1] + 0.30, 0.08), (0.9, 1.6, 0.16), RZ(48.0)), "wood")
    return parts


def build_parts():
    """Every mesh of the cat except the LED face (see face_part)."""
    parts = []

    def add(name, vf, kind, bone=None, weights=None, closed=True):
        v, f = vf
        parts.append(Part(name, v, f, [kind], weights if weights is not None else rigid(bone, len(v)),
                          closed=closed))

    # -- torso: one continuous shell that bends with the spine: deep chest, tucked waist, round
    #    hips, the back a little higher over the shoulders and hips. Its seams are inlaid lines.
    body_path = [(-2.62, 0, BZ - 0.04), (-1.6, 0, BZ + 0.05), (-0.6, 0, BZ - 0.03), (0.6, 0, BZ + 0.02),
                 (1.35, 0, BZ + 0.10), (2.0, 0, BZ + 0.13)]
    body_prof = [(0.0, 0.48, 0.48, 0.54), (0.22, 0.61, 0.60, 0.72), (0.46, 0.47, 0.50, 0.50),
                 (0.74, 0.66, 0.62, 0.92), (1.0, 0.52, 0.56, 0.72)]
    body_kw = dict(n_seg=28, round0=0.2, round1=0.2)
    spine = [("hips", -0.85), ("waist", 0.2), ("chest", None)]
    v, f = tube(body_path, body_prof, n_len=44, **body_kw)
    add("body_shell", (v, f), "shell", weights=chain_weights(v[:, 0], spine, 0.5))
    for k, t in enumerate((0.17, 0.355, 0.575, 0.80)):
        v, f = tube(body_path, body_prof, n_len=2, t0=t, t1=t + 0.012, grow=0.006, **body_kw)
        add("body_seam%d" % k, (v, f), "graphite", weights=chain_weights(v[:, 0], spine, 0.5))

    # -- neck: a tapered shell sleeve from chest to head, one seam, a ball joint at the skull
    neck_path = [(1.25, 0, BZ + 0.20), (1.68, 0, BZ + 0.62), (2.3, 0, BZ + 1.12)]
    neck_prof = [(0, 0.50, 0.50), (1, 0.40, 0.40)]
    neck_chain = [("chest", 0.12), ("neck", 0.70), ("head", None)]
    v, f = tube(neck_path, neck_prof, n_len=16, n_seg=18, round0=0.1, round1=0.1)
    add("neck_sleeve", (v, f), "shell", weights=chain_weights((v[:, 0] - 1.25) / 1.05, neck_chain, 0.16))
    v, f = tube(neck_path, neck_prof, n_len=2, n_seg=18, round0=0.1, round1=0.1, t0=0.44, t1=0.47, grow=0.006)
    add("neck_seam", (v, f), "graphite", weights=chain_weights((v[:, 0] - 1.25) / 1.05, neck_chain, 0.16))
    add("neck_joint", ball(head_of("head"), 0.45), "metal", "head")

    # -- head
    add("head_shell", tube([HEAD_C - [HEAD_R[0], 0, 0], HEAD_C + [HEAD_R[0], 0, 0]],
                           [(0, HEAD_R[1], HEAD_R[2]), (1, HEAD_R[1], HEAD_R[2])],
                           n_len=22, n_seg=32, round0=0.5, round1=0.5), "shell", "head")
    add("muzzle", ball(HEAD_C + [0.86, 0, -0.44], (0.40, 0.46, 0.30), n=12), "shell", "head")
    add("nose", ball(HEAD_C + [1.235, 0, -0.38], (0.07, 0.11, 0.075), n=8), "graphite", "head")
    v, f = head_patch((VISOR_HALF[0] + 4.0, VISOR_HALF[1] + 3.6), 1.02)
    add("visor_bezel", (v, f), "metal", "head", closed=False)
    v, f = head_patch(VISOR_HALF, 1.04)
    add("visor", (v, f), "visor", "head", closed=False)
    # a seam over the top of the head, ear to ear
    add("head_seam", tube([HEAD_C - [HEAD_R[0], 0, 0], HEAD_C + [HEAD_R[0], 0, 0]],
                          [(0, HEAD_R[1], HEAD_R[2]), (1, HEAD_R[1], HEAD_R[2])],
                          n_len=2, n_seg=32, round0=0.5, round1=0.5, t0=0.27, t1=0.29, grow=0.006),
        "graphite", "head")

    # -- ears: shell, a dark inner face, and a light at the tip
    for tag, sgn in (("n", -1.0), ("f", 1.0)):
        lower, tip, prof, kw = _ear(sgn)
        ear_side = kw["side"]
        add("ear_%s" % tag, tube([lower, tip], prof, n_len=10, t0=0.0, t1=0.84, **kw), "shell", "ear_" + tag)
        add("ear_ring_%s" % tag, tube([lower, tip], prof, n_len=1, t0=0.84, t1=0.86, grow=-0.012, **kw),
            "graphite", "ear_" + tag)
        add("ear_light_%s" % tag, tube([lower, tip], prof, n_len=6, t0=0.86, t1=1.0, **kw),
            "ear_light", "ear_" + tag)
        # the inner face: a thinner dark tube pushed out through the ear's front
        add("ear_inner_%s" % tag,
            tube([lower, tip], [(0, 0.46, 0.16), (0.5, 0.27, 0.11), (1, 0.05, 0.05)], n_len=8,
                 t0=0.24, t1=0.80, shift=(0.0, -sgn * 0.085), n_seg=12, round0=0.0, round1=0.14,
                 side=ear_side),
            "graphite", "ear_" + tag)

    # -- tail: a whip of short shell segments that nearly touch; the joints show only as seams
    for i in range(TAIL_N):
        b = "tail%d" % (i + 1)
        h = head_of(b)
        r0 = lerp(0.235, 0.095, i / TAIL_N)
        r1 = lerp(0.235, 0.095, (i + 1) / TAIL_N)
        last = i == TAIL_N - 1
        add("tail_joint%d" % (i + 1), ball(h, r0 * 0.74, n=8), "metal", b)
        add("tail_shell%d" % (i + 1),
            tube([h + [-0.012, 0, 0], h + [-TAIL_SEG + 0.012, 0, 0]], [(0, r0, r0), (1, r1, r1)],
                 n_len=8, n_seg=14, round0=0.16, round1=0.6 if last else 0.16), "shell", b)

    def paw(leg, x0, x1):
        y = LEG_Y[leg]
        return tube([(x0, y, 0.165), (x1, y, 0.15)],
                    [(0, 0.165, 0.16), (0.62, 0.20, 0.165), (1, 0.165, 0.14)], n_len=10, n_seg=14,
                    round0=0.5, round1=0.45, floor=0.0)

    # -- front legs: each piece wide at the top and fine at the bottom, joints sunk between them
    for leg in LEGS_FRONT:
        y = LEG_Y[leg]
        S, E, W = head_of("upper_" + leg), head_of("lower_" + leg), head_of("paw_" + leg)
        out = -0.06 if y < 0 else 0.06
        add("scapula_" + leg, ball((0.93, y + out, BZ + 0.10), (0.50, 0.17, 0.62), n=10), "shell", "scap_" + leg)
        add("shoulder_" + leg, ball(S + [0, out * 0.6, 0], 0.22), "metal", "scap_" + leg)
        add("upper_shell_" + leg, tube([lerp(S, E, -0.06) + [0, out, 0], lerp(S, E, 0.97)],
                                       [(0, 0.27, 0.46), (0.5, 0.24, 0.33), (1, 0.165, 0.18)], n_len=12,
                                       n_seg=14, round0=0.36, round1=0.16), "shell", "upper_" + leg)
        add("elbow_" + leg, ball(E, 0.12, n=8), "metal", "lower_" + leg)
        add("lower_shell_" + leg, tube([lerp(E, W, 0.03), lerp(E, W, 0.97)],
                                       [(0, 0.185, 0.215), (1, 0.11, 0.115)], n_len=10, n_seg=14,
                                       round0=0.2, round1=0.16), "shell", "lower_" + leg)
        add("wrist_" + leg, ball(W, 0.085, n=8), "metal", "paw_" + leg)
        add("paw_shell_" + leg, paw(leg, 0.80, 1.38), "shell", "paw_" + leg)

    # -- hind legs: a broad haunch plate over the hip, then fine tapering pieces
    for leg in LEGS_HIND:
        y = LEG_Y[leg]
        H, K, A, Pj = (head_of(b + leg) for b in ("thigh_", "shin_", "meta_", "paw_"))
        out = -0.07 if y < 0 else 0.07
        haunch_path = [lerp(H, K, -0.42) + [0, out, 0], lerp(H, K, 0.97)]
        haunch_prof = [(0, 0.30, 0.82), (0.4, 0.32, 0.74), (0.75, 0.24, 0.36), (1, 0.18, 0.20)]
        haunch_kw = dict(n_seg=18, round0=0.45, round1=0.14)
        add("hip_joint_" + leg, ball(H + [0, out * 0.6, 0], 0.22), "metal", "thigh_" + leg)
        add("haunch_" + leg, tube(haunch_path, haunch_prof, n_len=14, **haunch_kw), "shell", "thigh_" + leg)
        add("haunch_seam_" + leg, tube(haunch_path, haunch_prof, n_len=2, t0=0.56, t1=0.58, grow=0.006,
                                       **haunch_kw), "graphite", "thigh_" + leg)
        add("knee_" + leg, ball(K, 0.125, n=8), "metal", "shin_" + leg)
        add("shin_shell_" + leg, tube([lerp(K, A, 0.03), lerp(K, A, 0.97)],
                                      [(0, 0.21, 0.27), (1, 0.135, 0.14)], n_len=10, n_seg=14,
                                      round0=0.2, round1=0.16), "shell", "shin_" + leg)
        add("hock_" + leg, ball(A, 0.095, n=8), "metal", "meta_" + leg)
        add("meta_shell_" + leg, tube([lerp(A, Pj, 0.05), lerp(A, Pj, 0.95)],
                                      [(0, 0.14, 0.15), (1, 0.115, 0.12)], n_len=8, n_seg=12,
                                      round0=0.2, round1=0.2), "shell", "meta_" + leg)
        add("ankle_" + leg, ball(Pj, 0.08, n=8), "metal", "paw_" + leg)
        add("paw_shell_" + leg, paw(leg, -2.06, -1.48), "shell", "paw_" + leg)
    return parts


# =============================================================================================
# Poses
# =============================================================================================

def _stand():
    p = {
        # hips: where they are (an offset from rest) and how they turn
        "hx": 0.0, "hz": 0.0, "hp": 0.0, "hy": 0.0, "hr": 0.0,
        # waist and chest bend, relative to the bone before
        "wp": 0.0, "wy": 0.0, "cp": 0.0, "cy": 0.0, "cr": 0.0,
        # neck and head: world angles (they do not follow the spine)
        "np": 0.0, "ny": 0.0, "kp": 0.0, "ky": -16.0, "kr": 0.0,
        # ears, relative to the head: pitch (forward +) and roll (outward +)
        "en_p": 0.0, "en_r": 0.0, "ef_p": 0.0, "ef_r": 0.0,
        # the whole cat, paws and all, turned about an upright line through its middle
        # (SPIN_AT): 0 faces right, 360 is once round (the tail chase)
        "spin": 0.0,
    }
    for leg, x in (("fn", 1.06), ("ff", 0.84), ("hn", -1.96), ("hf", -1.72)):
        p[leg + "_x"], p[leg + "_y"], p[leg + "_z"], p[leg + "_pp"] = x, 0.0, 0.0, 0.0
        if leg in LEGS_FRONT:
            p[leg + "_sh"] = 0.0     # shoulder blade: how far it rides up
        else:
            p[leg + "_mp"] = 0.0     # metatarsus pitch (0 = as modelled, -70 = flat on the ground)
    set_tail(p, [38, 58, 70, 62, 40, 14])
    return p


def set_tail(p, elev, heading=None):
    """The tail's shape in world angles, root to tip, spread over its segments: `elev` degrees
    above level (0 = straight back), `heading` degrees swung round towards the camera (90 = at
    the camera, 180 = forward). Lists of any length; they are resampled along the tail."""
    heading = heading or [0.0]
    for i in range(TAIL_N):
        u = i / (TAIL_N - 1)
        p["te%d" % (i + 1)] = float(np.interp(u, np.linspace(0, 1, len(elev)), elev))
        p["th%d" % (i + 1)] = float(np.interp(u, np.linspace(0, 1, len(heading)), heading))
    return p


TIP = str(TAIL_N)          # the tail's last segment, as in p["te" + TIP]
TIP2 = str(TAIL_N - 1)


def P(base=None, tail=None, tail_h=None, **kw):
    """A pose: the standing pose (or `base`) with some numbers changed."""
    p = dict(base if base is not None else STAND)
    for k, v in kw.items():
        if k not in p:
            raise KeyError("unknown pose key %r" % k)
        p[k] = float(v)
    if tail is not None:
        set_tail(p, tail, tail_h if tail_h is not None else [p["th%d" % (i + 1)] for i in range(TAIL_N)])
    return p


def mix(a, b, t):
    return {k: lerp(a[k], b[k], t) for k in a}


SPIN_AT = -0.50                 # where the cat turns about when it spins (world x)
ELBOW_POLE = (-1.0, 0.0, 0.8)   # elbows point back, and up when the leg is folded forward
KNEE_POLE = (1.0, 0.0, 0.5)     # knees point forward


def _two_bone(S, T, a, b, pole):
    """The middle joint of a two-bone leg from S towards T (lengths a, b), and where its end
    really lands (T if it can be reached). The joint bends towards `pole`."""
    d = T - S
    dist = np.linalg.norm(d)
    reach = min(max(dist, abs(a - b) + 1e-3), a + b - 1e-3)
    dirv = d / dist if dist > 1e-6 else np.array([0.0, 0.0, -1.0])
    ca = (a * a + reach * reach - b * b) / (2 * a * reach)
    sa = math.sqrt(max(0.0, 1 - ca * ca))
    pole = np.asarray(pole, float)
    bend = pole - np.dot(pole, dirv) * dirv
    bend = unit(bend) if np.linalg.norm(bend) > 1e-6 else np.cross([0.0, 1.0, 0.0], dirv)
    mid = S + dirv * (a * ca) + bend * (a * sa)
    end = S + dirv * reach
    return mid, end


def solve(p):
    """World transform of every bone for a pose: {bone: (R 3x3, p 3)}; a rest-pose point v bound
    to the bone goes to R @ (v - head_of(bone)) + p."""
    out = {"root": (np.eye(3), np.zeros(3))}

    def child(bone, parent, R, offset=(0.0, 0.0, 0.0)):
        Rp, pp = out[parent]
        pos = Rp @ (head_of(bone) - head_of(parent) + np.asarray(offset, float)) + pp
        out[bone] = (R, pos)

    R_h = euler(p["hr"], p["hp"], p["hy"])
    out["hips"] = (R_h, head_of("hips") + [p["hx"], 0.0, p["hz"]])
    R_w = R_h @ euler(0, p["wp"], p["wy"])
    child("waist", "hips", R_w)
    R_c = R_w @ euler(p["cr"], p["cp"], p["cy"])
    child("chest", "waist", R_c)
    child("neck", "chest", euler(0, p["np"], p["ny"]))
    R_k = euler(p["kr"], p["kp"], p["ky"])
    child("head", "neck", R_k)
    child("ear_n", "head", R_k @ euler(p["en_r"], p["en_p"], 0))     # the near ear rolls out to -Y
    child("ear_f", "head", R_k @ euler(-p["ef_r"], p["ef_p"], 0))

    for i in range(TAIL_N):
        b = "tail%d" % (i + 1)
        child(b, "hips" if i == 0 else "tail%d" % i, RZ(p["th%d" % (i + 1)]) @ RY(p["te%d" % (i + 1)]))

    for leg in LEGS_FRONT:
        up, lo, paw, scap = "upper_" + leg, "lower_" + leg, "paw_" + leg, "scap_" + leg
        child(scap, "chest", R_c, offset=np.linalg.inv(R_c) @ np.array([0.0, 0.0, p[leg + "_sh"]]))
        S = out[scap][0] @ (head_of(up) - head_of(scap)) + out[scap][1]
        R_paw = RY(p[leg + "_pp"])
        contact = np.array([p[leg + "_x"], LEG_Y[leg] + p[leg + "_y"], p[leg + "_z"]])
        T = contact + R_paw @ np.array([0.0, 0.0, WRIST_H])
        a = np.linalg.norm(head_of(lo) - head_of(up))
        b = np.linalg.norm(head_of(paw) - head_of(lo))
        E, T = _two_bone(S, T, a, b, ELBOW_POLE)
        out[up] = (basis(E - S) @ basis(head_of(lo) - head_of(up)).T, S)
        out[lo] = (basis(T - E) @ basis(head_of(paw) - head_of(lo)).T, E)
        out[paw] = (R_paw, T)

    for leg in LEGS_HIND:
        th, sh, me, paw = "thigh_" + leg, "shin_" + leg, "meta_" + leg, "paw_" + leg
        H = R_h @ (head_of(th) - head_of("hips")) + out["hips"][1]
        R_paw, R_me = RY(p[leg + "_pp"]), RY(p[leg + "_mp"])
        contact = np.array([p[leg + "_x"], LEG_Y[leg] + p[leg + "_y"], p[leg + "_z"]])
        ankle = contact + R_paw @ np.array([0.0, 0.0, ANKLE_H])
        hock = ankle + R_me @ (head_of(me) - head_of(paw))
        a = np.linalg.norm(head_of(sh) - head_of(th))
        b = np.linalg.norm(head_of(me) - head_of(sh))
        K, hock2 = _two_bone(H, hock, a, b, KNEE_POLE)
        out[th] = (basis(K - H) @ basis(head_of(sh) - head_of(th)).T, H)
        out[sh] = (basis(hock2 - K) @ basis(head_of(me) - head_of(sh)).T, K)
        out[me] = (R_me, hock2)
        out[paw] = (R_paw, hock2 + R_me @ (head_of(paw) - head_of(me)))

    if p.get("spin"):   # the finished pose, turned as one piece
        turn, about = RZ(p["spin"]), np.array([SPIN_AT, 0.0, 0.0])
        out = {b: ((R, pos) if b == "root" else (turn @ R, turn @ (pos - about) + about))
               for b, (R, pos) in out.items()}
    return out


def skinned(part, xf):
    """A part's vertices in a solved pose (what Blender's armature modifier does)."""
    out = np.zeros_like(part.verts)
    total = np.zeros(len(part.verts))
    for bone, w in part.weights.items():
        R, pos = xf[bone]
        out += w[:, None] * ((part.verts - head_of(bone)) @ R.T + pos)
        total += w
    return out / total[:, None]


def fit_square(parts, pose, yaw, pitch, margin=0.05):
    """The camera target and width (world units) that frame `parts` in a square picture."""
    right, up, _ = camera_axes(yaw, pitch)
    xf = solve(pose)
    pts = np.vstack([skinned(p, xf) for p in parts if len(p.verts)])
    u, v = pts @ right, pts @ up
    cu, cv = (u.min() + u.max()) / 2, (v.min() + v.max()) / 2
    centre = pts.mean(axis=0)
    target = centre + right * (cu - centre @ right) + up * (cv - centre @ up)
    return target, max(u.max() - u.min(), v.max() - v.min()) * (1 + 2 * margin)


def fit_head_crop(parts, pose, yaw, pitch, head_share=0.5, margin=0.03):
    """A tighter square for a whole figure: the head (with its ears) takes `head_share` of the
    square's height and sits at the top; sideways the square centres on whatever of the body
    falls inside it, the tail aside (it may leave the picture), keeping the whole head in."""
    right, up, _ = camera_axes(yaw, pitch)
    xf = solve(pose)
    head = np.vstack([skinned(p, xf) for p in parts if set(p.weights) <= {"head", "ear_n", "ear_f"}])
    body = np.vstack([skinned(p, xf) for p in parts
                      if len(p.verts) and not any(b.startswith("tail") for b in p.weights)])
    hu, hv = head @ right, head @ up
    units = (hv.max() - hv.min()) / head_share
    top = hv.max() + margin * units
    bu, bv = body @ right, body @ up
    inside = bu[bv >= top - units]
    cu = (inside.min() + inside.max()) / 2
    lo, hi = hu.max() + margin * units - units / 2, hu.min() - margin * units + units / 2
    cu = min(max(cu, lo), hi) if lo <= hi else (hu.min() + hu.max()) / 2
    cv = top - units / 2
    centre = body.mean(axis=0)
    return centre + right * (cu - centre @ right) + up * (cv - centre @ up), units


def point_on(xf, bone, v):
    """Where a rest-pose point bound to `bone` is in a solved pose."""
    R, pos = xf[bone]
    return R @ (np.asarray(v, float) - head_of(bone)) + pos


def hang_leg(p, leg, dx, dz, pp=0.0, mp=0.0, dy=0.0):
    """Put a paw at an offset from its shoulder / hip joint (for legs that are off the ground)."""
    xf = solve(p)
    if leg in LEGS_FRONT:
        j = point_on(xf, "scap_" + leg, head_of("upper_" + leg))
        c = j + [dx, dy, dz] - RY(pp) @ np.array([0.0, 0.0, WRIST_H])
    else:
        j = point_on(xf, "hips", head_of("thigh_" + leg))
        c = j + [dx, dy, dz] - RY(pp) @ np.array([0.0, 0.0, ANKLE_H])
        p[leg + "_mp"] = float(mp)
    p[leg + "_x"], p[leg + "_z"], p[leg + "_pp"] = float(c[0]), float(c[2]), float(pp)
    p[leg + "_y"] = float(c[1] - LEG_Y[leg])
    return p


def reach(p, leg, at, pp=0.0, mp=0.0):
    """Put a paw at a world point `at` (for a paw off the ground: a wave, a lick)."""
    xf = solve(p)
    if leg in LEGS_FRONT:
        j = point_on(xf, "scap_" + leg, head_of("upper_" + leg))
    else:
        j = point_on(xf, "hips", head_of("thigh_" + leg))
    d = np.asarray(at, float) - j
    return hang_leg(p, leg, d[0], d[2], pp=pp, mp=mp, dy=d[1])


def pin(p, bone, v, target):
    """Move the whole body so a point of it lands on `target` (x, z)."""
    at = point_on(solve(p), bone, v)
    p["hx"] += target[0] - at[0]
    p["hz"] += target[1] - at[2]
    return p


STAND = _stand()

# -- key poses ---------------------------------------------------------------------------------

SIT = P(hx=0.30, hz=-1.88, hp=-34, wp=-11, cp=-9, np=30, kp=0, ky=-18,
        fn_x=0.98, ff_x=0.78, hn_x=-0.30, hf_x=-0.10, hn_mp=-74, hf_mp=-74,
        tail=[-8, -3, 0, 0, 0, 4], tail_h=[28, 64, 100, 132, 156, 168])


def _loaf():
    """The sleeping curl: the spine bent into a C that opens towards the camera, haunch high,
    head tucked down by the front paws, tail brought round the front."""
    p = P(hx=-0.25, hz=-2.08, hp=3, hy=24, hr=-8, wp=2, wy=-26, cp=4, cy=-30, np=34, ny=-40,
          kp=22, ky=-62, kr=10, en_p=6, ef_p=6, en_r=16, ef_r=16,
          tail=[-30, -8, 0, 0, 0, 4], tail_h=[50, 95, 135, 165, 185, 195])
    hang_leg(p, "fn", 0.80, -0.44, dy=-0.55)
    hang_leg(p, "ff", 0.95, -0.44, dy=-0.75)
    hang_leg(p, "hn", 1.05, -0.52, mp=-74, dy=-0.10)
    hang_leg(p, "hf", 1.00, -0.52, mp=-74, dy=0.10)
    for leg in LEGS_FRONT + LEGS_HIND:
        p[leg + "_z"] = 0.0
    return p


LOAF = _loaf()

CROUCH = P(hx=-0.25, hz=-0.85, hp=-4, wp=4, cp=6, np=12, kp=4, ky=-8,
           tail=[8, 4, 10, 26, 46, 62], en_p=10, ef_p=10)


# =============================================================================================
# Animations
# =============================================================================================

CYAN = (0.30, 0.88, 1.0)
AMBER = (1.0, 0.60, 0.10)


def face(shape="open", lx=0, ly=0, sweep=None, color=CYAN, hi=1.0, lo=0.28, big=False):
    return {"shape": shape, "lx": lx, "ly": ly, "sweep": sweep, "color": color, "hi": hi, "lo": lo,
            "big": big}


def lights(near=0.22, far=0.22, color=CYAN):
    """Ear tip lights: brightness 0..1 each."""
    return {"near": near, "far": far, "color": color}


def wave(t, k=1, phase=0.0):
    return math.sin(2 * math.pi * (k * t + phase))


def bump(t, centre, width):
    """A smooth 0..1..0 bump around `centre` (t wraps at 1)."""
    d = abs(((t - centre + 0.5) % 1.0) - 0.5)
    return 0.5 + 0.5 * math.cos(math.pi * d / width) if d < width else 0.0


def blink_shape(t, at, base="open", span=0.035):
    """`base`, but closing and opening again around time `at`."""
    d = abs(t - at)
    if d < span * 0.5:
        return "blink"
    if d < span * 1.4:
        return "squint"
    return base


def keys(t, frames):
    """Blend between key poses: frames is [(time, pose), ...], t in 0..1, eased."""
    if t <= frames[0][0]:
        return dict(frames[0][1])
    for (t0, a), (t1, b) in zip(frames, frames[1:]):
        if t <= t1:
            return mix(a, b, smooth((t - t0) / (t1 - t0)))
    return dict(frames[-1][1])


def sway_tail(p, t, amount=1.0, k=1, phase=0.0):
    """A wave running down the tail, added to the pose's tail."""
    for i in range(TAIL_N):
        w = (i + 1) / TAIL_N
        p["te%d" % (i + 1)] += amount * 9 * w * wave(t, k, phase - 0.66 * i / TAIL_N)
        p["th%d" % (i + 1)] += amount * 16 * w * wave(t, k, phase - 0.66 * i / TAIL_N + 0.25)
    return p


def anim_idle(t):
    """Standing, breathing; the tail sways, one ear flicks, the eyes glance and blink."""
    b = wave(t, 2)
    p = P(hz=0.035 * b, cp=-0.8 * b, np=-1.5 * b, kp=1.2 * wave(t, 1, 0.1),
          ky=-16 - 9 * bump(t, 0.38, 0.16), kr=5 * bump(t, 0.38, 0.14))
    sway_tail(p, t, 1.0)
    flick = bump(t, 0.70, 0.045)
    p["en_p"], p["en_r"] = -16 * flick, 10 * flick
    look = -1 if 0.30 < t < 0.47 else 0
    return p, face(blink_shape(t, 0.86), lx=look), lights()


def _walk_foot(ph, x0, stride, duty=0.64, lift=0.42):
    """One paw through a walk cycle: (x, z, paw pitch). On the ground for `duty` of the cycle,
    sliding back; then lifted and carried forward."""
    ph %= 1.0
    if ph < duty:
        u = ph / duty
        return x0 + stride * (0.5 - u), 0.0, 6 * bump(u, 1.0, 0.25)
    u = (ph - duty) / (1 - duty)
    x = x0 + stride * (-0.5 + smooth(u))
    z = lift * math.sin(math.pi * u) ** 1.3
    pitch = 34 * math.sin(math.pi * min(1.0, u * 1.35)) * (1 - u) - 10 * bump(u, 0.9, 0.2)
    return x, z, pitch


WALK_STRIDE = 2.0   # world units the ground moves under the cat per cycle


def anim_walk(t):
    """A low, slinky prowl: each paw placed in turn, shoulder blades rolling, head held low and
    steady, tail trailing with a slow wave."""
    p = P(hz=-0.42 + 0.035 * wave(t, 2, 0.1), hx=0.05 * wave(t, 2, 0.3), hp=1.5 * wave(t, 1, 0.05),
          wp=2.2 * wave(t, 1, 0.30), cp=-2.5 * wave(t, 1, 0.55) + 2, cy=3 * wave(t, 1, 0.5),
          hy=3 * wave(t, 1), np=20 + 1.5 * wave(t, 2, 0.2), kp=4 + 1.0 * wave(t, 2, 0.45), ky=-10,
          en_p=6, ef_p=6, tail=[-10, -16, -4, 16, 40, 62])
    sway_tail(p, t, 0.8)
    # lateral sequence: near hind, near front, far hind, far front
    for leg, ph, x0 in (("hn", 0.0, -1.80), ("fn", 0.25, 1.12), ("hf", 0.5, -1.80), ("ff", 0.75, 1.12)):
        x, z, pitch = _walk_foot(t + ph, x0, WALK_STRIDE)
        p[leg + "_x"], p[leg + "_z"], p[leg + "_pp"] = x, z, pitch
        cycle = (t + ph) % 1.0
        if leg in LEGS_FRONT:
            p[leg + "_sh"] = 0.20 * bump(cycle, 0.32, 0.30) - 0.05
        else:
            p[leg + "_mp"] = 14 * bump(cycle, 0.70, 0.16) - 6 * bump(cycle, 0.95, 0.12)
    return p, face(blink_shape(t, 0.55), lx=1), lights()


def _sit_idle(t, base=SIT):
    b = wave(t, 2)
    p = P(base, hp=base["hp"] - 0.5 * b, cp=base["cp"] - 0.9 * b, np=base["np"] - 1.2 * b)
    tip = bump(t, 0.55, 0.10) - bump(t, 0.68, 0.08)
    p["te" + TIP] += 26 * bump(t, 0.6, 0.12)
    p["th" + TIP] += 14 * tip
    p["te" + TIP2] += 8 * bump(t, 0.6, 0.12)
    return p


def anim_sit(t):
    """Sitting upright, tail wrapped round the paws; breathing, a tail-tip flick, a slow blink."""
    p = _sit_idle(t)
    p["ky"] = SIT["ky"] - 14 * bump(t, 0.25, 0.18)
    p["kr"] = -6 * bump(t, 0.25, 0.15)
    flick = bump(t, 0.80, 0.05)
    p["ef_p"], p["ef_r"] = -14 * flick, 8 * flick
    return p, face(blink_shape(t, 0.46, span=0.05)), lights()


def anim_sit_down(t):
    """Standing to sitting: the rump folds down first, the chest follows and settles."""
    over = P(SIT, hz=SIT["hz"] - 0.10, np=SIT["np"] + 6, hp=SIT["hp"] + 5)
    half = mix(STAND, SIT, 0.5)
    half = P(half, hz=half["hz"] - 0.25, hp=half["hp"] - 6, np=half["np"] - 4)
    p = keys(t, [(0.0, STAND), (0.42, half), (0.78, over), (1.0, SIT)])
    return p, face("open"), lights()


def anim_sleep(t):
    """Curled up asleep: spine in a C, paws tucked, tail brought round, head down; slow
    breathing, eyes closed to dim arcs, ear lights barely glowing."""
    b = 0.5 - 0.5 * math.cos(2 * math.pi * t)    # one slow breath per loop
    p = P(LOAF, hz=LOAF["hz"] + 0.05 * b, cp=LOAF["cp"] - 1.6 * b, wp=LOAF["wp"] - 0.8 * b,
          kp=LOAF["kp"] - 1.2 * b,
          np=LOAF["np"] - 1.0 * b)
    p["en_p"] = LOAF["en_p"] - 10 * bump(t, 0.62, 0.05)
    p["te" + TIP] += 10 * bump(t, 0.30, 0.10)
    glow = 0.10 + 0.10 * b
    return p, face("sleepy", hi=0.42 + 0.14 * b), lights(glow, glow)


def anim_lie_down(t):
    """Standing to the sleeping loaf: a crouch, then the chest and paws slide forward."""
    low = mix(CROUCH, LOAF, 0.55)
    p = keys(t, [(0.0, STAND), (0.35, CROUCH), (0.70, low), (1.0, LOAF)])
    shape = "open" if t < 0.5 else ("squint" if t < 0.85 else "sleepy")
    return p, face(shape, hi=1.0 if t < 0.85 else 0.5), lights(0.2 * (1 - t) + 0.1, 0.2 * (1 - t) + 0.1)


WORK = P(SIT, hp=-31, hx=0.20, cp=-2, np=48, kp=18, ky=-12, fn_x=1.36, ff_x=1.14, en_p=10, ef_p=10)


def anim_working(t):
    """Sitting hunched over its work, kneading with the front paws; the head makes small scans,
    the eyes narrow with a bright bar sweeping across, the ear lights blink in turn."""
    p = P(WORK, ky=WORK["ky"] + 9 * wave(t, 1), kp=WORK["kp"] + 2 * wave(t, 2, 0.2),
          np=WORK["np"] + 1.5 * wave(t, 2))
    near, far = bump(t, 0.25, 0.22), bump(t, 0.75, 0.22)
    p["fn_z"], p["fn_pp"] = 0.26 * near, 22 * near
    p["ff_z"], p["ff_pp"] = 0.26 * far, 22 * far
    p["cp"] += 1.2 * wave(t, 2, 0.1)
    p["te" + TIP] += 14 * bump(t, 0.5, 0.2)
    step = int(t * 12) % 12
    sweep = [-3, -2, -1, 0, 1, 2, 3, 2, 1, 0, -1, -2][step]
    blink = int(t * 8) % 2
    return p, face("half", sweep=sweep, lo=0.32), lights(1.0 if blink == 0 else 0.12, 1.0 if blink == 1 else 0.12)


WAIT = P(SIT, hp=-38, hx=0.36, hz=-1.86, fn_x=0.84, ff_x=0.64, np=24, kp=-6, ky=-52, kr=0, en_p=12, ef_p=12,
         tail=[-6, -3, 0, 0, 0, 6], tail_h=[26, 60, 96, 128, 150, 160])


def anim_waiting(t):
    """Sits up tall and looks out at you: ears pricked, eyes wide and amber, pulsing; a front paw
    lifts and pats the air, the tail tip flicks."""
    lift = bump(t, 0.34, 0.26)
    pat = wave(t, 4) * lift
    p = P(WAIT, kr=7 * wave(t, 1, 0.05), kp=WAIT["kp"] - 3 * lift, hz=WAIT["hz"] + 0.03 * wave(t, 2))
    p["fn_z"] = 1.05 * lift + 0.10 * pat
    p["fn_x"] = WAIT["fn_x"] + 0.55 * lift
    p["fn_pp"] = 38 * lift + 14 * pat
    p["te" + TIP] += 34 * bump(t, 0.80, 0.10)
    p["te" + TIP2] += 12 * bump(t, 0.80, 0.10)
    p["th" + TIP] -= 18 * bump(t, 0.86, 0.08)
    pulse = 0.78 + 0.22 * wave(t, 2)
    return p, face("wide", color=AMBER, hi=pulse), lights(pulse, pulse, AMBER)


def anim_happy(t):
    """Two little bounces a loop, tail straight up with a curled tip, head tipping side to side,
    eyes in happy arches."""
    u = (t * 2) % 1.0
    air = max(0.0, math.sin(math.pi * (u - 0.22) / 0.56)) if 0.22 < u < 0.78 else 0.0
    squash = bump(u, 0.10, 0.16) + bump(u, 0.90, 0.14)
    hop = 0.52 * air
    p = P(hz=hop - 0.26 * squash, hp=-5 * air + 3 * squash, cp=-4 * air, np=-8 * air + 6 * squash,
          kr=11 * wave(t, 1, 0.1), ky=-26, kp=-4 * air,
          en_p=8 * air - 6 * squash, ef_p=8 * air - 6 * squash,
          tail=[62, 84, 96, 92, 58, 6])
    for leg in LEGS_FRONT + LEGS_HIND:
        p[leg + "_z"] = hop * (1.55 if leg in LEGS_FRONT else 1.0)
        p[leg + "_pp"] = 24 * air
    p["te" + TIP2] += 14 * wave(t, 2)
    p["te" + TIP] += 30 * wave(t, 2, 0.1)
    return p, face("happy", color=CYAN), lights(0.9, 0.9)


def _air_pose(flutter=0.0):
    p = P(hx=0.05, hz=0.55, hp=-16, wp=-4, cp=-5, np=-8, kp=-6, ky=-8, en_p=-18, ef_p=-18,
          en_r=6, ef_r=6, tail=[16 + 3 * flutter, 10, 6, 12, 24, 38 + 6 * flutter])
    hang_leg(p, "fn", 1.55, -1.25 + 0.05 * flutter, pp=-28)
    hang_leg(p, "ff", 1.25, -1.50 - 0.05 * flutter, pp=-22)
    hang_leg(p, "hn", -1.75, -1.15 - 0.05 * flutter, pp=58, mp=52)
    hang_leg(p, "hf", -1.45, -1.40 + 0.05 * flutter, pp=50, mp=46)
    return p


def anim_jump(t):
    """Crouch with a wiggle, launch off the hind legs, and stretch out long in the air."""
    wiggle = wave(t, 3) * (1 - smooth(t / 0.45))
    crouch = P(CROUCH, hz=-1.0, hp=-9, hy=5 * wiggle, np=6, kp=-2)
    launch = P(hx=0.55, hz=0.10, hp=-24, wp=-5, cp=-6, np=-10, kp=-8, ky=-8, en_p=-12, ef_p=-12,
               hn_x=-2.35, hf_x=-2.15, hn_pp=30, hf_pp=26, hn_mp=24, hf_mp=20,
               tail=[4, 2, 8, 20, 34, 46])
    hang_leg(launch, "fn", 1.15, -1.55, pp=-30)
    hang_leg(launch, "ff", 0.90, -1.70, pp=-24)
    p = keys(t, [(0.0, P(CROUCH, hz=-0.6)), (0.36, crouch), (0.66, launch), (1.0, anim_air(0.0)[0])])
    return p, face("wide" if t > 0.3 else "open", lx=1, ly=1), lights(0.6, 0.6)


def anim_air(t):
    """In the air, stretched long: a small flutter of legs, ears and tail."""
    p = _air_pose(wave(t, 1))
    p["en_p"] += 5 * wave(t, 2)
    p["ef_p"] += 5 * wave(t, 2, 0.2)
    return p, face("wide", lx=1), lights(0.6, 0.6)


def anim_land(t):
    """Front paws first, a deep soft crouch that soaks up the fall, then up to standing."""
    reach = P(hx=-0.05, hz=0.05, hp=17, wp=4, cp=6, np=-14, kp=-10, ky=-10, en_p=-14, ef_p=-14,
              fn_x=1.45, ff_x=1.20, fn_pp=-10, ff_pp=-8, tail=[40, 52, 50, 34, 16, 0])
    hang_leg(reach, "hn", -0.95, -1.45, pp=34, mp=30)
    hang_leg(reach, "hf", -0.70, -1.60, pp=28, mp=26)
    absorb = P(CROUCH, hz=-1.15, hx=0.12, hp=3, cp=10, np=22, kp=10, fn_x=1.30, ff_x=1.05,
               en_p=14, ef_p=14, en_r=10, ef_r=10, tail=[50, 66, 62, 40, 10, -14])
    rise = P(hz=0.06, np=-4, tail=[40, 60, 72, 66, 46, 20])
    p = keys(t, [(0.0, reach), (0.30, absorb), (0.46, absorb), (0.80, rise), (1.0, anim_idle(0.0)[0])])
    shape = "squint" if 0.22 < t < 0.52 else "open"
    return p, face(shape), lights()


DRAG_ANCHOR = (0.55, 5.05)   # where the scruff is held, world (x, z)


def anim_drag(t):
    """Carried by the scruff: the body hangs and swings a little, legs dangling, tail curling
    and swishing slowly, eyes wide."""
    sw = wave(t, 1)
    lag = wave(t, 1, -0.18)
    p = P(hp=-50 + 5 * sw, wp=5 + 2 * lag, cp=7, np=-12 + 3 * lag, kp=-4 + 2 * lag, ky=-24, kr=4 * sw,
          en_p=-12, ef_p=-12, en_r=12, ef_r=12,
          tail=[-70 + 8 * lag, -48 + 12 * lag, -18 + 16 * lag, 14 + 18 * lag, 44 + 16 * lag, 66 + 12 * lag])
    for i in range(TAIL_N):
        p["th%d" % (i + 1)] = 10 * (i + 1) / TAIL_N * wave(t, 1, 0.3)
    pin(p, "chest", SCRUFF, DRAG_ANCHOR)
    hang_leg(p, "fn", 0.70 + 0.14 * lag, -1.62, pp=44 + 6 * lag)
    hang_leg(p, "ff", 0.46 + 0.14 * lag, -1.74, pp=38 + 6 * lag)
    hang_leg(p, "hn", 0.80 + 0.20 * lag, -1.62, pp=40 + 8 * lag, mp=10 + 8 * lag)
    hang_leg(p, "hf", 0.52 + 0.20 * lag, -1.74, pp=34 + 8 * lag, mp=6 + 8 * lag)
    return p, face(blink_shape(t, 0.5, base="wide"), ly=1), lights(0.45, 0.45)


def anim_stretch(t):
    """The long cat stretch: front paws walk out, chest sinks to the floor, rump and tail high,
    a shiver, then back up."""
    bow = P(hx=-0.35, hz=-0.10, hp=27, wp=5, cp=7, np=-34, kp=-12, ky=-14, fn_x=2.08, ff_x=1.90,
            fn_pp=-4, ff_pp=-4, en_p=-10, ef_p=-10, tail=[66, 82, 92, 84, 52, 12])
    half = mix(STAND, bow, 0.5)
    half["fn_z"] = 0.25
    p = keys(t, [(0.0, STAND), (0.14, half), (0.34, bow), (0.66, bow), (0.86, P(hz=0.08, np=-6)), (1.0, anim_idle(0.0)[0])])
    hold = bump(t, 0.50, 0.18)
    p["hx"] -= 0.10 * hold
    p["hp"] += 2.5 * hold + 0.7 * hold * wave(t, 14)
    p["te" + TIP2] += 6 * hold * wave(t, 14)
    shape = "squint" if 0.26 < t < 0.72 else "open"
    return p, face(shape), lights()


# -- stage 29.8: grooming, yawning, waving, a pounce crouch, and work at a desk -----------------

def anim_groom(t):
    """Sitting, it licks the near front paw (little head bobs), then wipes the paw over its face
    and up behind the ear, and puts it down again."""
    lift = smooth(t / 0.16) * (1 - smooth((t - 0.86) / 0.14))
    lick = bump(t, 0.36, 0.22)
    wipe = bump(t, 0.68, 0.18)
    p = _sit_idle(t)
    p["kp"] = SIT["kp"] + 14 * lick + 6 * lick * wave(t, 9) - 4 * wipe
    p["ky"] = SIT["ky"] + 6 * lick - 26 * wipe
    p["kr"] = -16 * wipe
    p["np"] = SIT["np"] + 10 * lick - 4 * wipe
    p["en_p"], p["en_r"] = -18 * wipe, 14 * wipe
    if lift > 0.01:
        rest = np.array([p["fn_x"], LEG_Y["fn"] + p["fn_y"], p["fn_z"]])
        lick_at = np.array([2.90, -0.55, 3.10])          # in front of the muzzle
        wipe_at = np.array([2.05, -1.45, 4.20])          # up the near cheek, by the ear
        up_at = lick_at + (wipe_at - lick_at) * smooth(wipe * 1.4)
        reach(p, "fn", rest + (up_at - rest) * lift, pp=-90 * lift)
    shape = "happy" if lick > 0.3 else ("blink" if wipe > 0.4 else ("squint" if wipe > 0.15 else "open"))
    return p, face(shape), lights(0.3, 0.3)


def anim_yawn(t):
    """A big yawn: the head tips back, ears fold, eyes squeeze shut, the chest lifts; then it
    shakes its head a little and settles."""
    y = bump(t, 0.40, 0.32)
    p = P(hz=0.06 * y, hp=-4 * y, cp=-5 * y, np=-16 * y, kp=-30 * y, ky=-16 - 10 * y,
          en_p=-22 * y, ef_p=-22 * y, en_r=18 * y, ef_r=18 * y,
          tail=[38 + 8 * y, 58, 70 + 10 * y, 62, 40 + 20 * y, 14 + 30 * y])
    shake = bump(t, 0.84, 0.10)
    p["kr"] = 10 * shake * wave(t, 10)
    shape = "squeeze" if y > 0.45 else ("squint" if y > 0.15 else "open")
    return p, face(shape, hi=0.8), lights(0.22 - 0.12 * y, 0.22 - 0.12 * y)


def anim_wave(t):
    """Hello: the near front paw goes up high and waves, head tipped, eyes happy."""
    up = smooth(t / 0.2) * (1 - smooth((t - 0.82) / 0.18))
    w = wave(t, 3) * bump(t, 0.5, 0.36)
    p = P(hz=0.32 * up, hp=-14 * up, cp=-4 * up, np=-8 * up, kr=8 * up, ky=-24 * up - 16,
          en_p=8 * up, ef_p=8 * up, tail=[48, 70, 84, 80, 60, 30])
    sway_tail(p, t, 0.6, k=2)
    if up > 0.01:
        rest = np.array([p["fn_x"], LEG_Y["fn"] + p["fn_y"], p["fn_z"]])
        high = np.array([2.30 + 0.16 * w, -1.38 + 0.06 * w, 5.15 + 0.06 * abs(w)])   # up beside the face
        reach(p, "fn", rest + (high - rest) * up, pp=-70 * up + 35 * w)
    return p, face("happy" if up > 0.3 else "open"), lights(0.25 + 0.6 * up, 0.25 + 0.6 * up)


def anim_crouch(t):
    """Ready to pounce: low on its front, rump up and wiggling, tail tip twitching, eyes wide
    and fixed on the target."""
    wig = wave(t, 2)
    p = P(CROUCH, hz=-1.05, hp=-9, hy=7 * wig, wp=2, np=4, kp=-4, ky=-6, hr=2 * wig,
          tail=[2, -2, 2, 10, 26, 40], en_p=14, ef_p=14)
    p["hn_mp"] = -10 + 6 * wig
    p["hf_mp"] = -10 - 6 * wig
    p["te" + TIP] += 30 * wave(t, 4)
    p["th" + TIP] += 20 * wave(t, 4, 0.25)
    return p, face("wide", lx=1, ly=-1), lights(0.7, 0.7)


AT_DESK = P(SIT, hp=-30, hx=-0.15, hz=-1.80, cp=-4, np=42, kp=16, ky=-6, en_p=10, ef_p=10)


def anim_coding(t):
    """At its desk, paws on the laptop's keys, typing in quick bursts; eyes scanning the
    screen, ear lights blinking with the keys."""
    p = P(AT_DESK, ky=AT_DESK["ky"] + 4 * wave(t, 1), np=AT_DESK["np"] + 1.0 * wave(t, 2))
    for leg, dy, ph in (("fn", -0.30, 0.0), ("ff", 0.30, 0.5)):
        key = KEYS_AT + LAPTOP_R @ np.array([0.10, dy, 0.0])
        tap = max(0.0, wave(t, 6, ph)) * (1 - bump(t, 0.80, 0.10))
        p[leg + "_x"] = float(key[0]) + 0.05 * wave(t, 3, ph)
        p[leg + "_y"] = float(key[1] - LEG_Y[leg])
        p[leg + "_z"] = float(key[2]) + 0.16 * tap
        p[leg + "_pp"] = 18 * tap
    step = int(t * 12) % 12
    sweep = [-3, -2, -1, 0, 1, 2, 3, 2, 1, 0, -1, -2][step]
    blink = int(t * 12) % 2
    return p, face("half", lx=1, ly=-1, sweep=sweep, lo=0.32), lights(1.0 if blink else 0.15, 0.15 if blink else 1.0)


AT_BOARD = P(SIT, hp=-28, hx=0.05, hz=-1.80, cp=-4, np=44, kp=20, ky=-4, en_p=8, ef_p=8)


def board_point(u, v, lift=0.0):
    """A point on the drafting board's paper: u up the slope, v across, `lift` off it."""
    n = BOARD_R @ np.array([0.0, 0.0, 1.0])
    return BOARD_LOW + BOARD_R @ np.array([u, v, 0.03]) + n * lift


def anim_designing(t):
    """At the drafting table, a pencil in the near paw: short strokes across the drawing, a
    pause to look it over with the head tipped, the far paw holding the paper."""
    p = P(AT_BOARD, kr=-8 * bump(t, 0.75, 0.16), ky=AT_BOARD["ky"] - 10 * bump(t, 0.75, 0.16))
    stroke = (t * 4) % 1.0
    drawing = 1 - bump(t, 0.75, 0.16)
    u = 0.55 + 0.45 * stroke * drawing + 0.20 * (int(t * 4) % 2)
    v = -0.30 + 0.25 * wave(t, 2)
    pen = board_point(u, v, 0.16 + 0.30 * (1 - drawing) + 0.06 * bump(stroke, 0.0, 0.15))
    p["fn_x"], p["fn_y"], p["fn_z"] = float(pen[0]), float(pen[1] - LEG_Y["fn"]), float(pen[2])
    p["fn_pp"] = 30.0
    hold = board_point(0.35, 0.45, 0.10)
    p["ff_x"], p["ff_y"], p["ff_z"] = float(hold[0]), float(hold[1] - LEG_Y["ff"]), float(hold[2])
    p["ff_pp"] = -BOARD_TILT
    shape = "open" if drawing > 0.5 else blink_shape(t, 0.75, span=0.05)
    return p, face(shape, lx=1, ly=-2), lights(0.5, 0.5)


AT_BOOK = P(SIT, hp=-34, hx=-0.05, hz=-1.86, np=44, kp=20, ky=-8, en_p=6, ef_p=6)


def anim_researching(t):
    """Reading the open book on the floor: the eyes run along the lines, the head follows a
    little, now and then a paw pats the page and the ears twitch."""
    line = (t * 3) % 1.0
    p = P(AT_BOOK, ky=AT_BOOK["ky"] - 10 + 18 * line, kp=AT_BOOK["kp"] + 3 * (int(t * 3) % 3))
    pat = bump(t, 0.85, 0.10)
    page = BOOK_AT + BOOK_R @ RZ(12.0) @ np.array([-0.25, -0.55, 0.55])
    p["fn_x"] = lerp(AT_BOOK["fn_x"], float(page[0]) - 0.30, pat)
    p["fn_z"] = lerp(0.0, float(page[2]) - 0.30, pat)
    p["fn_y"] = lerp(0.0, float(page[1] - LEG_Y["fn"]), pat)
    p["fn_pp"] = -40 * pat
    flick = bump(t, 0.45, 0.05)
    p["en_p"] += -14 * flick
    look = int(-2 + 4 * line)
    return p, face(blink_shape(t, 0.62), lx=look, ly=-2), lights(0.3, 0.3)


# -- stage 31: a long nap, a dangle, a tail chase and a look around ------------------------------

def hold(t, start, end, ramp):
    """0, rising to 1 by `start`, staying until `end`, back to 0: each change takes `ramp`."""
    return smooth((t - start + ramp) / ramp) * (1 - smooth((t - end) / ramp))


def _curl():
    """The long nap's curl: tighter than the loaf, nose down on the front paws, the tail brought
    all the way round to the nose, ears eased out."""
    p = P(LOAF, hy=30, wy=-32, cy=-36, ny=-52, np=40, kp=30, ky=-84, kr=16, hr=-10,
          en_p=2, ef_p=2, en_r=24, ef_r=24,
          tail=[-30, -10, -2, 0, 0, 2], tail_h=[55, 100, 142, 174, 198, 214])
    hang_leg(p, "fn", 0.78, -0.44, dy=-0.62)
    hang_leg(p, "ff", 0.92, -0.44, dy=-0.84)
    hang_leg(p, "hn", 1.05, -0.52, mp=-74, dy=-0.14)
    hang_leg(p, "hf", 1.00, -0.52, mp=-74, dy=0.08)
    for leg in LEGS_FRONT + LEGS_HIND:
        p[leg + "_z"] = 0.0
    return p


CURL = _curl()


def anim_nap(t):
    """The long nap: curled tight, one slow deep breath a loop, eyes dim arcs, ear lights all
    but out; the tail tip twitches once and an ear flicks at a dream."""
    b = 0.5 - 0.5 * math.cos(2 * math.pi * t)
    p = P(CURL, hz=CURL["hz"] + 0.06 * b, cp=CURL["cp"] - 2.0 * b, wp=CURL["wp"] - 1.0 * b,
          kp=CURL["kp"] - 1.0 * b, np=CURL["np"] - 0.8 * b)
    p["ef_p"] = CURL["ef_p"] - 12 * bump(t, 0.70, 0.05)
    p["te" + TIP] += 12 * bump(t, 0.34, 0.08)
    p["th" + TIP] -= 10 * bump(t, 0.40, 0.08)
    glow = 0.05 + 0.07 * b
    return p, face("sleepy", hi=0.34 + 0.14 * b), lights(glow, glow)


def anim_curl_up(t):
    """Standing to the nap's curl: a low crouch, the rump goes down and round, the head follows
    the tail in and the eyes close."""
    low = mix(CROUCH, CURL, 0.5)
    low = P(low, kp=low["kp"] - 8, hz=low["hz"] + 0.10)
    nearly = P(mix(CROUCH, CURL, 0.88), kp=CURL["kp"] - 12)
    p = keys(t, [(0.0, STAND), (0.28, CROUCH), (0.58, low), (0.84, nearly), (1.0, CURL)])
    shape = "open" if t < 0.45 else ("squint" if t < 0.80 else "sleepy")
    dim = 1.0 if t < 0.80 else 0.34
    return p, face(shape, hi=dim), lights(0.22 * (1 - t) + 0.05, 0.22 * (1 - t) + 0.05)


def anim_wake(t):
    """Out of the nap: an ear flicks, the head comes up with the eyes half open, the ears prick,
    and it pushes up to standing."""
    up = P(CURL, np=CURL["np"] - 22, kp=-4, ky=-40, kr=4, ny=-24, en_p=10, ef_p=10, en_r=6, ef_r=6)
    rise = P(mix(CROUCH, up, 0.35), kp=0, kr=0, en_p=8, ef_p=8)
    p = keys(t, [(0.0, CURL), (0.12, CURL), (0.40, up), (0.52, up), (0.80, rise), (1.0, anim_idle(0.0)[0])])
    p["en_p"] -= 14 * bump(t, 0.08, 0.06)
    shape = "sleepy" if t < 0.16 else ("squint" if t < 0.34 else blink_shape(t, 0.46))
    glow = 0.05 + 0.6 * bump(t, 0.50, 0.30) + 0.17 * smooth((t - 0.6) / 0.4)
    return p, face(shape, hi=0.4 + 0.6 * smooth((t - 0.10) / 0.25)), lights(glow, glow)


def anim_dangle(t):
    """Carried about by the scruff: the whole cat swings like a pendulum under the hand, hind
    end furthest, legs limp and a beat behind, tail streaming after them, ears back, eyes wide."""
    sw = wave(t, 1)
    lag = wave(t, 1, -0.16)
    late = wave(t, 1, -0.30)
    p = P(hp=-58 + 11 * sw, wp=7 + 5 * lag, cp=9 + 3 * lag, np=-14 + 4 * lag, kp=-6 + 3 * lag,
          ky=-26, kr=6 * sw, en_p=-16 + 4 * late, ef_p=-16 + 4 * late, en_r=14, ef_r=14,
          tail=[-74 + 12 * late, -54 + 18 * late, -26 + 24 * late, 6 + 26 * late, 36 + 24 * late, 58 + 20 * late])
    for i in range(TAIL_N):
        p["th%d" % (i + 1)] = 12 * (i + 1) / TAIL_N * wave(t, 1, 0.3)
    pin(p, "chest", SCRUFF, DRAG_ANCHOR)
    hang_leg(p, "fn", 0.62 + 0.26 * lag, -1.50, pp=50 + 12 * lag)
    hang_leg(p, "ff", 0.40 + 0.26 * late, -1.62, pp=44 + 12 * late)
    hang_leg(p, "hn", 0.78 + 0.36 * lag, -1.46, pp=46 + 14 * lag, mp=14 + 12 * lag)
    hang_leg(p, "hf", 0.50 + 0.36 * late, -1.58, pp=40 + 14 * late, mp=10 + 12 * late)
    return p, face(blink_shape(t, 0.72, base="wide"), lx=int(round(sw)), ly=1), lights(0.45, 0.45)


def _chase_pose(u, steps):
    """The tail chase's running pose, `u` 0..1 through the spin: the spine bent in a C towards
    the near side, head round after the tail, the tail curled just out of reach, and the paws
    pattering in diagonal pairs, `steps` each."""
    p = P(hz=-0.30, hp=4, hy=16, wy=-24, cy=-28, ny=-46, np=14, kp=10, ky=-96, kr=8,
          en_p=12, ef_p=12, tail=[6, 10, 14, 18, 22, 26], tail_h=[30, 62, 92, 118, 138, 150])
    patter = wave(u, steps)
    for leg, dx, dy, side in (("fn", 0.10, -0.52, 1), ("ff", 0.22, -0.40, -1),
                              ("hn", 0.20, 0.12, -1), ("hf", 0.10, 0.20, 1)):
        hang_leg(p, leg, dx, -2.0, dy=dy)
        lift = max(0.0, side * patter)
        p[leg + "_z"] = 0.34 * lift
        p[leg + "_pp"] = 20 * lift
        if leg in LEGS_HIND:
            p[leg + "_mp"] = 10 * lift
    p["te" + TIP] += 16 * wave(u, steps, 0.2)
    p["th" + TIP] += 14 * wave(u, steps, 0.45)
    p["hz"] += 0.05 * wave(u, steps * 2)
    return p


def anim_tail_chase(t):
    """It notices its tail, and goes after it: twice round on the spot, paws pattering, then a
    stop, a dizzy wobble of the head and back to standing."""
    stand = anim_idle(0.0)[0]
    into = smooth(t / 0.16)
    out = 1 - smooth((t - 0.84) / 0.16)
    u = min(1.0, max(0.0, (t - 0.12) / 0.72))
    p = mix(stand, _chase_pose(u, 6), min(into, out))
    turn = u * u * (3 - 2 * u)
    p["spin"] = -720.0 * turn
    dizzy = bump(t, 0.92, 0.09)
    p["kr"] += 12 * dizzy * wave(t, 8)
    p["ky"] += 8 * dizzy * wave(t, 8, 0.25)
    shape = "wide" if 0.06 < t < 0.84 else ("squint" if t < 0.95 and t >= 0.84 else "open")
    return p, face(shape, lx=-1 if t < 0.84 else 0), lights(0.7 * min(into, out) + 0.22, 0.7 * min(into, out) + 0.22)


def anim_look_around(t):
    """Sitting, it looks about: round towards the viewer, back, then away to the far side, the
    eyes leading the head each time; the ears prick and each flicks once."""
    near = hold(t, 0.16, 0.34, 0.12)
    far = hold(t, 0.56, 0.74, 0.12)
    p = _sit_idle(t)
    p["ky"] = SIT["ky"] - 46 * near + 27 * far
    p["ny"] = -14 * near + 8 * far
    p["kr"] = -7 * near + 6 * far
    p["kp"] = SIT["kp"] - 5 * (near + far)
    alert = max(near, far)
    flick_n, flick_f = bump(t, 0.30, 0.045), bump(t, 0.68, 0.045)
    p["en_p"], p["en_r"] = 9 * alert - 16 * flick_n, 10 * flick_n
    p["ef_p"], p["ef_r"] = 9 * alert - 16 * flick_f, 10 * flick_f
    look = -1 if 0.03 < t < 0.20 else (1 if 0.42 < t < 0.60 else 0)
    shape = blink_shape(t, 0.455, span=0.04)
    if shape == "open":
        shape = blink_shape(t, 0.90, span=0.04)
    return p, face(shape, lx=look), lights(0.22 + 0.3 * alert, 0.22 + 0.3 * alert)


# What each animation shows besides the cat: the prop sets (build_props) it needs.
ANIM_PROPS = {"coding": ("desk",), "designing": ("drafting",), "researching": ("book",)}


def props_for(name):
    """The prop sets an animation shows."""
    return ANIM_PROPS.get(name, ())


# name: (function, frames, fps, loop, next)
ANIMATIONS = {
    "idle": (anim_idle, 36, 12, True, None),
    "walk": (anim_walk, 16, 16, True, None),
    "sit_down": (anim_sit_down, 9, 12, False, "sit"),
    "sit": (anim_sit, 36, 12, True, None),
    "lie_down": (anim_lie_down, 11, 12, False, "sleep"),
    "sleep": (anim_sleep, 32, 8, True, None),
    "working": (anim_working, 24, 12, True, None),
    "waiting": (anim_waiting, 24, 12, True, None),
    "happy": (anim_happy, 16, 12, True, None),
    "jump": (anim_jump, 8, 14, False, "air"),
    "air": (anim_air, 8, 12, True, None),
    "land": (anim_land, 10, 14, False, "idle"),
    "drag": (anim_drag, 24, 12, True, None),
    "stretch": (anim_stretch, 30, 12, False, "idle"),
    "groom": (anim_groom, 40, 12, False, "sit"),
    "yawn": (anim_yawn, 22, 12, False, "idle"),
    "wave": (anim_wave, 18, 12, False, "idle"),
    "crouch": (anim_crouch, 16, 12, True, None),
    "coding": (anim_coding, 24, 12, True, None),
    "designing": (anim_designing, 32, 12, True, None),
    "researching": (anim_researching, 36, 12, True, None),
    # stage 31
    "curl_up": (anim_curl_up, 14, 12, False, "nap"),
    "nap": (anim_nap, 32, 8, True, None),
    "wake": (anim_wake, 18, 12, False, "idle"),
    "dangle": (anim_dangle, 24, 12, True, None),
    "tail_chase": (anim_tail_chase, 40, 16, False, "idle"),
    "look_around": (anim_look_around, 36, 12, False, "sit"),
}

# What the app's poses (data-pet) play: an optional one-shot intro, then the animation.
POSES = {
    "idle": (None, "idle"), "walk": (None, "walk"), "sit": ("sit_down", "sit"),
    "sleep": ("lie_down", "sleep"), "working": (None, "working"), "waiting": (None, "waiting"),
    "happy": (None, "happy"), "jump": ("jump", "air"), "land": (None, "land"), "drag": (None, "drag"),
    # stage 29.8
    "stretch": ("stretch", "idle"), "groom": ("groom", "sit"), "yawn": ("yawn", "idle"),
    "wave": ("wave", "idle"), "crouch": (None, "crouch"), "coding": (None, "coding"),
    "designing": (None, "designing"), "researching": (None, "researching"),
    # stage 31
    "nap": ("curl_up", "nap"), "wake": ("wake", "idle"), "dangle": (None, "dangle"),
    "chase": ("tail_chase", "idle"), "look": ("look_around", "sit"),
}

# The frame of each animation shown in the preview (0..1 through it).
KEY_FRAME = {"idle": 0.0, "walk": 0.19, "sit_down": 0.5, "sit": 0.0, "lie_down": 0.5, "sleep": 0.5,
             "working": 0.25, "waiting": 0.34, "happy": 0.25, "jump": 0.62, "air": 0.0, "land": 0.33,
             "drag": 0.25, "stretch": 0.5, "groom": 0.62, "yawn": 0.42, "wave": 0.45, "crouch": 0.0,
             "coding": 0.08, "designing": 0.30, "researching": 0.85,
             "curl_up": 0.6, "nap": 0.5, "wake": 0.45, "dangle": 0.25, "tail_chase": 0.4,
             "look_around": 0.25}


def frame_time(name, i):
    """Where frame i sits in its animation, 0..1: loops stop short of 1 (frame 0 follows the
    last), one-shots run from 0 to exactly 1."""
    _, n, _, loop, _ = ANIMATIONS[name]
    return i / n if loop else (i / (n - 1) if n > 1 else 0.0)


def sample(name, t):
    return ANIMATIONS[name][0](t)


# =============================================================================================
# PNG in and out (plain zlib, so sheets are written exactly as computed)
# =============================================================================================

def write_png(path, rgba):
    """Write an (h, w, 4) uint8 array, top row first, as a straight-alpha PNG."""
    rgba = np.ascontiguousarray(rgba, dtype=np.uint8)
    h, w, _ = rgba.shape
    raw = np.zeros((h, w * 4 + 1), np.uint8)
    raw[:, 1:] = rgba.reshape(h, w * 4)

    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    with open(path, "wb") as fh:
        fh.write(b"\x89PNG\r\n\x1a\n")
        fh.write(chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0)))
        fh.write(chunk(b"IDAT", zlib.compress(raw.tobytes(), 6)))
        fh.write(chunk(b"IEND", b""))


def srgb_to_linear(c):
    c = np.asarray(c, float)
    return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)


def to_bytes(a):
    return np.clip(np.rint(a * 255.0), 0, 255).astype(np.uint8)


def tinted(base, mask_a, gloss, tint):
    """What the UI shows: the shell multiplied by `tint` where the mask says, the gloss screened
    back on top. All float (h, w, 4) / (h, w) in display (sRGB) values, as CSS blends them."""
    m = mask_a[..., None]
    rgb = base[..., :3] * (1 - m + m * np.asarray(tint, float))
    if gloss is not None:
        rgb = 1 - (1 - rgb) * (1 - gloss[..., :3] * gloss[..., 3:4])
    return np.concatenate([rgb, base[..., 3:4]], axis=2)


def over(dst, src, x, y):
    """Alpha-composite float RGBA `src` onto float RGB `dst` at (x, y)."""
    h, w = src.shape[:2]
    a = src[..., 3:4]
    dst[y:y + h, x:x + w] = dst[y:y + h, x:x + w] * (1 - a) + src[..., :3] * a


def halve(img):
    """Box-filter an image to half size (alpha-weighted)."""
    h, w = img.shape[0] // 2 * 2, img.shape[1] // 2 * 2
    q = img[:h, :w].reshape(h // 2, 2, w // 2, 2, 4)
    a = q[..., 3:4]
    asum = a.sum(axis=(1, 3))
    rgb = (q[..., :3] * a).sum(axis=(1, 3)) / np.maximum(asum, 1e-6)
    return np.concatenate([rgb, asum / 4.0], axis=2)


# =============================================================================================
# Blender
# =============================================================================================

T0 = time.time()


def log(msg):
    print("[pet3d %6.1fs] %s" % (time.time() - T0, msg), flush=True)


def try_set(obj, attr, value):
    """Set a property that only some Blender versions have."""
    try:
        if hasattr(obj, attr):
            setattr(obj, attr, value)
            return True
    except Exception as exc:  # a value this version does not accept
        log("note: could not set %s.%s = %r (%s)" % (type(obj).__name__, attr, value, exc))
    return False


# ---- materials --------------------------------------------------------------------------------

SHELL_GREY = 0.86


def _node_tree(idblock):
    if bpy.app.version < (5, 0, 0):   # from 5.0 every material and world has nodes
        idblock.use_nodes = True
    nt = idblock.node_tree
    if nt is None:
        raise RuntimeError("%s has no node tree" % idblock.name)
    nt.nodes.clear()
    return nt


def _set_input(node, names, value):
    for name in names:
        if name in node.inputs:
            node.inputs[name].default_value = value
            return
    log("note: %s has none of the inputs %s" % (node.bl_idname, names))


def mat_principled(name, color, roughness, metallic=0.0, coat=0.0, coat_roughness=0.08):
    mat = bpy.data.materials.new(name)
    nt = _node_tree(mat)
    out = nt.nodes.new("ShaderNodeOutputMaterial")
    bsdf = nt.nodes.new("ShaderNodeBsdfPrincipled")
    nt.links.new(bsdf.outputs["BSDF"], out.inputs["Surface"])
    _set_input(bsdf, ["Base Color"], (color[0], color[1], color[2], 1.0))
    _set_input(bsdf, ["Roughness"], roughness)
    _set_input(bsdf, ["Metallic"], metallic)
    if coat > 0:
        _set_input(bsdf, ["Coat Weight", "Clearcoat"], coat)
        _set_input(bsdf, ["Coat Roughness", "Clearcoat Roughness"], coat_roughness)
    return mat


def mat_emission(name, color, strength=1.0):
    mat = bpy.data.materials.new(name)
    nt = _node_tree(mat)
    out = nt.nodes.new("ShaderNodeOutputMaterial")
    em = nt.nodes.new("ShaderNodeEmission")
    nt.links.new(em.outputs["Emission"], out.inputs["Surface"])
    em.inputs["Color"].default_value = (color[0], color[1], color[2], 1.0)
    em.inputs["Strength"].default_value = strength
    return mat


class Materials:
    """The material for a part's kind in each pass. LED colours are made as they are asked for."""

    def __init__(self):
        g = SHELL_GREY
        self.base = {
            "shell": mat_principled("shell", (g, g, g), 0.30, coat=0.6),
            "graphite": mat_principled("graphite", (0.030, 0.033, 0.040), 0.42),
            "metal": mat_principled("metal", (0.60, 0.62, 0.66), 0.34, metallic=0.85),
            "visor": mat_principled("visor", (0.008, 0.010, 0.016), 0.10, coat=1.0, coat_roughness=0.03),
            # the props (stage 29.8): never tinted, no gloss
            "wood": mat_principled("wood", (0.30, 0.17, 0.08), 0.62),
            "paper": mat_principled("paper", (0.82, 0.82, 0.78), 0.80),
            "ink": mat_principled("ink", (0.03, 0.07, 0.26), 0.55),
            "cover": mat_principled("cover", (0.42, 0.06, 0.05), 0.50),
            "pencil": mat_principled("pencil", (0.85, 0.55, 0.05), 0.45),
            "screen": mat_emission("screen", (0.10, 0.32, 0.62), 1.0),
            "code": mat_emission("code", (0.55, 0.95, 1.0), 1.0),
        }
        self.white = mat_emission("mask_white", (1, 1, 1))
        self.black = mat_emission("mask_black", (0, 0, 0), 0.0)
        self.gloss_shell = mat_principled("gloss_shell", (0, 0, 0), 0.30, coat=0.6)
        self.leds = {}

    def led(self, color, level):
        """An LED at `level` (0..1 of full brightness); it never drops fully dark."""
        level = round(max(0.06, min(1.0, level)), 2)
        key = (tuple(round(c, 3) for c in color), level)
        if key not in self.leds:
            # full brightness pushes the colour towards white, as a bright LED looks
            c = [min(1.0, ch * (0.55 + 1.25 * level)) for ch in key[0]]
            self.leds[key] = mat_emission("led_%d" % len(self.leds), c, 1.0)
        return self.leds[key]

    def get(self, pass_name, kind, state):
        if pass_name == "mask":
            return self.white if kind == "shell" else self.black
        if pass_name == "gloss":
            return self.gloss_shell if kind == "shell" else self.black
        if kind == "led_hi":
            return self.led(state["face"]["color"], state["face"]["hi"])
        if kind == "led_lo":
            return self.led(state["face"]["color"], state["face"]["lo"])
        if kind == "ear_light":
            side = "near" if state["part"].endswith("_n") else "far"
            return self.led(state["lights"]["color"], state["lights"][side])
        return self.base[kind]


# ---- scene ------------------------------------------------------------------------------------

def reset_scene():
    for coll in (bpy.data.objects, bpy.data.meshes, bpy.data.armatures, bpy.data.materials,
                 bpy.data.lights, bpy.data.cameras):
        for item in list(coll):
            coll.remove(item)
    return bpy.context.scene


def set_engine(scene, want, samples):
    """Pick the render engine: Eevee under whichever name this Blender uses, else Cycles."""
    order = {"auto": ["BLENDER_EEVEE_NEXT", "BLENDER_EEVEE", "CYCLES"],
             "eevee": ["BLENDER_EEVEE_NEXT", "BLENDER_EEVEE"], "cycles": ["CYCLES"]}[want]
    chosen = None
    for engine in order:
        try:
            scene.render.engine = engine
            chosen = engine
            break
        except TypeError:
            continue
    if chosen is None:
        raise RuntimeError("no usable render engine among %s" % order)
    if chosen == "CYCLES":
        scene.cycles.samples = max(16, samples)
        try_set(scene.cycles, "use_denoising", False)
        try_set(scene.cycles, "device", "CPU")
    else:
        ee = scene.eevee
        try_set(ee, "taa_render_samples", samples)
        try_set(ee, "use_gtao", True)            # legacy Eevee ambient occlusion
        try_set(ee, "gtao_distance", 1.2)
        try_set(ee, "use_soft_shadows", True)
        try_set(ee, "shadow_cascade_size", "2048")
        try_set(ee, "use_raytracing", True)      # Eevee Next: screen-space reflections
    log("render engine: %s" % chosen)
    return chosen


def setup_render(scene):
    r = scene.render
    r.film_transparent = True
    r.resolution_percentage = 100
    r.image_settings.file_format = "PNG"
    r.image_settings.color_mode = "RGBA"
    r.image_settings.color_depth = "8"
    try_set(r.image_settings, "compression", 15)
    r.use_file_extension = True
    try_set(r, "dither_intensity", 0.0)
    try_set(r, "use_stamp", False)
    scene.display_settings.display_device = "sRGB"
    for name in ("Standard",):
        scene.view_settings.view_transform = name
    try_set(scene.view_settings, "look", "None")
    scene.view_settings.exposure = 0.0
    scene.view_settings.gamma = 1.0


LIGHTS = [  # name, the direction the light travels, strength (relative), angle (softness, degrees)
    ("key", (-0.30, 0.55, -0.78), 2.7, 32.0),
    ("fill", (0.55, 0.80, -0.22), 1.0, 45.0),
    ("rim", (0.25, -0.75, -0.60), 1.6, 12.0),
]
WORLD_STRENGTH = 0.45


def setup_lights(scene):
    world = bpy.data.worlds.new("pet_world")
    nt = _node_tree(world)
    out = nt.nodes.new("ShaderNodeOutputWorld")
    bg = nt.nodes.new("ShaderNodeBackground")
    nt.links.new(bg.outputs["Background"], out.inputs["Surface"])
    bg.inputs["Color"].default_value = (0.92, 0.95, 1.0, 1.0)
    bg.inputs["Strength"].default_value = WORLD_STRENGTH
    scene.world = world
    lamps = []
    for name, travel, strength, angle in LIGHTS:
        data = bpy.data.lights.new(name, "SUN")
        data.energy = strength
        data.angle = math.radians(angle)
        obj = bpy.data.objects.new(name, data)
        obj.rotation_euler = Vector(travel).to_track_quat("-Z", "Y").to_euler()
        try_set(data, "shadow_cascade_max_distance", 40.0)
        scene.collection.objects.link(obj)
        lamps.append(data)
    return lamps, bg


def scale_lights(lamps, bg, factor):
    for lamp in lamps:
        lamp.energy *= factor
    bg.inputs["Strength"].default_value *= factor


def setup_camera(scene):
    data = bpy.data.cameras.new("pet_camera")
    data.type = "ORTHO"
    data.clip_start, data.clip_end = 0.1, 200.0
    cam = bpy.data.objects.new("pet_camera", data)
    scene.collection.objects.link(cam)
    scene.camera = cam
    return cam


def aim_camera(scene, size, yaw=CAM_YAW, pitch=CAM_PITCH, units_w=FRAME_UNITS_W, target=CAM_TARGET):
    """Point the camera and set the picture size; `units_w` world units span the picture's width."""
    cam = scene.camera
    _, _, fwd = camera_axes(yaw, pitch)
    cam.location = Vector(target) - Vector(fwd) * 60.0
    cam.rotation_euler = Vector(fwd).to_track_quat("-Z", "Y").to_euler()
    scene.render.resolution_x, scene.render.resolution_y = size
    # ortho_scale spans the picture's longer side
    cam.data.ortho_scale = units_w if size[0] >= size[1] else units_w * size[1] / size[0]


# ---- rig --------------------------------------------------------------------------------------

def build_armature(scene):
    data = bpy.data.armatures.new("PetRig")
    arm = bpy.data.objects.new("PetRig", data)
    scene.collection.objects.link(arm)
    bpy.context.view_layer.objects.active = arm
    arm.select_set(True)
    bpy.ops.object.mode_set(mode="EDIT")
    for name, (parent, head, tail) in BONES.items():
        eb = data.edit_bones.new(name)
        eb.head, eb.tail = head, tail
        if parent:
            eb.parent = data.edit_bones[parent]
        eb.use_connect = False
    bpy.ops.object.mode_set(mode="OBJECT")
    missing = [b for b in BONES if b not in data.bones]
    if missing:
        raise RuntimeError("bones missing from the armature: %s" % missing)
    for pb in arm.pose.bones:
        pb.rotation_mode = "QUATERNION"
    # each bone's rest orientation, read back from Blender so no roll convention is assumed
    rest = {b.name: np.array(b.matrix_local.to_3x3()) for b in data.bones}
    for name in BONES:
        got = np.array(data.bones[name].head_local)
        if np.linalg.norm(got - head_of(name)) > 1e-4:
            raise RuntimeError("bone %s is not where it was put" % name)
    return arm, rest


def pose_matrices(xf, rest):
    """Blender pose-bone matrices (matrix_basis) for a solved pose.

    With world rotation R and position p per bone (see solve), a bone's change relative to its
    parent is D = Rp^T R with offset t = Rp^T (p - pp) - (head - head_parent); Blender wants that
    in the bone's own rest frame: basis = Rr^T [D | t] Rr."""
    out = {}
    for name, (parent, _, _) in BONES.items():
        R, pos = xf[name]
        if parent:
            Rp, pp = xf[parent]
            D = Rp.T @ R
            t = Rp.T @ (pos - pp) - (head_of(name) - head_of(parent))
        else:
            D, t = R, pos - head_of(name)
        Rr = rest[name]
        M = np.eye(4)
        M[:3, :3] = Rr.T @ D @ Rr
        M[:3, 3] = Rr.T @ t
        out[name] = M
    return out


def apply_pose(arm, rest, pose):
    for name, M in pose_matrices(solve(pose), rest).items():
        arm.pose.bones[name].matrix_basis = Matrix(M.tolist())


def make_object(scene, part, arm, mats):
    mesh = bpy.data.meshes.new(part.name)
    mesh.from_pydata([tuple(v) for v in part.verts.tolist()], [], [tuple(f) for f in part.faces])
    mesh.update()
    if part.closed and len(part.faces):
        bm = bmesh.new()
        bm.from_mesh(mesh)
        bmesh.ops.recalc_face_normals(bm, faces=bm.faces[:])
        bm.to_mesh(mesh)
        bm.free()
    n = len(mesh.polygons)
    if n:
        mesh.polygons.foreach_set("use_smooth", [True] * n)
        mesh.polygons.foreach_set("material_index", [int(s) for s in part.face_slot][:n])
    for kind in part.slots:
        mesh.materials.append(mats.black)
    mesh.update()
    obj = bpy.data.objects.new(part.name, mesh)
    scene.collection.objects.link(obj)
    for bone, w in part.weights.items():
        group = obj.vertex_groups.new(name=bone)
        if np.all(w == 1.0):
            group.add(list(range(len(w))), 1.0, "REPLACE")
        else:
            for i in np.nonzero(w > 1e-4)[0]:
                group.add([int(i)], float(w[i]), "REPLACE")
    mod = obj.modifiers.new("rig", "ARMATURE")
    mod.object = arm
    obj.parent = arm
    return obj


class Pet:
    """The cat in the scene: its objects, and how to put it in a pose for a pass."""

    def __init__(self, scene):
        self.scene = scene
        self.mats = Materials()
        self.arm, self.rest = build_armature(scene)
        self.parts = [(make_object(scene, part, self.arm, self.mats), part) for part in build_parts()]
        self.extras = {part.name for part in icon_parts()}
        self.parts += [(make_object(scene, part, self.arm, self.mats), part) for part in icon_parts()]
        # the props (stage 29.8): shown only for the animations that ask for their set
        self.props = [(make_object(scene, part, self.arm, self.mats), part) for part in build_props()]
        for obj, _ in self.props:
            obj.hide_render = True
        self.faces = {}
        self.show(lambda part: True)
        log("model: %d parts, %d faces, %d bones" % (
            len(self.parts), sum(len(p.faces) for _, p in self.parts), len(BONES)))

    def show(self, wanted, extras=False):
        """Render only the parts `wanted(part)` accepts; the icon-only extras only if asked."""
        for obj, part in self.parts:
            if part.name in self.extras:
                obj.hide_render = not extras
            else:
                obj.hide_render = not wanted(part)

    def face_object(self, spec):
        key = (spec["shape"], spec["lx"], spec["ly"], spec["sweep"], bool(spec.get("big")))
        if key not in self.faces:
            part = face_part(*key)
            self.faces[key] = (make_object(self.scene, part, self.arm, self.mats), part)
        return key

    def set(self, pose, face_spec, light_spec, pass_name, props=()):
        apply_pose(self.arm, self.rest, pose)
        key = self.face_object(face_spec)
        state = {"face": face_spec, "lights": light_spec, "part": ""}
        for k, (obj, part) in self.faces.items():
            obj.hide_render = k != key
        shown = [(obj, part) for obj, part in self.props if any(part.name.startswith("prop_%s_" % s) for s in props)]
        for obj, part in self.props:
            obj.hide_render = (obj, part) not in shown
        for obj, part in self.parts + shown + [self.faces[key]]:
            state["part"] = part.name
            for i, kind in enumerate(part.slots):
                mat = self.mats.get(pass_name, kind, state)
                if obj.data.materials[i] != mat:
                    obj.data.materials[i] = mat
        bpy.context.view_layer.update()


# ---- rendering --------------------------------------------------------------------------------

class Renderer:
    def __init__(self, scene, tmp_dir):
        self.scene = scene
        self.tmp = tmp_dir
        self.count = 0
        os.makedirs(tmp_dir, exist_ok=True)

    def shot(self):
        """Render the scene as it stands; returns float RGBA (h, w, 4), top row first, in display
        (sRGB) values with straight alpha."""
        stem = os.path.join(self.tmp, "frame")
        self.scene.render.filepath = stem
        bpy.ops.render.render(write_still=True)
        path = stem + ".png"
        if not os.path.exists(path):
            raise RuntimeError("the render did not write %s" % path)
        out = self.read(path)
        self.count += 1
        want = (self.scene.render.resolution_x, self.scene.render.resolution_y)
        if (out.shape[1], out.shape[0]) != want:
            raise RuntimeError("the render is %dx%d, expected %dx%d" % (out.shape[1], out.shape[0], want[0], want[1]))
        return out

    @staticmethod
    def read(path):
        """A PNG as float RGBA (h, w, 4), top row first, display values, straight alpha."""
        img = bpy.data.images.load(path, check_existing=False)
        w, h = img.size
        buf = np.empty(w * h * 4, dtype=np.float32)
        img.pixels.foreach_get(buf)
        bpy.data.images.remove(img)
        return buf.reshape(h, w, 4)[::-1].astype(np.float64)


def mask_alpha(mask):
    """The shell's coverage from a mask render (white shell on black, same alpha as the base)."""
    return srgb_to_linear(mask[..., :3].mean(axis=2)) * mask[..., 3]


def calibrate(pet, rnd, lamps, bg, target):
    """Scale the lights so the lit shell lands on `target` (display value of its upper quartile),
    whatever this Blender version makes of light strengths."""
    scene = pet.scene
    aim_camera(scene, (192, 160))
    for attempt in range(5):
        pet.set(STAND, face(), lights(), "mask")
        m = mask_alpha(rnd.shot())
        pet.set(STAND, face(), lights(), "base")
        base = rnd.shot()
        shell = m > 0.95
        if shell.sum() < 200:
            raise RuntimeError("calibration found almost no shell in the picture (%d px): the "
                               "camera, the mask pass or the model is wrong" % shell.sum())
        lum = srgb_to_linear(base[..., :3].mean(axis=2)[shell])
        q25, q50, q75 = np.percentile(lum, [25, 50, 75])
        log("calibration %d: shell linear luminance quartiles %.3f %.3f %.3f (target upper %.3f)"
            % (attempt, q25, q50, q75, srgb_to_linear(target)))
        factor = float(srgb_to_linear(target) / max(q75, 1e-4))
        if abs(factor - 1.0) < 0.04:
            break
        scale_lights(lamps, bg, factor)
    return float(q75)


def render_passes(pet, rnd, pose, face_spec, light_spec, passes, props=()):
    out = {}
    for pass_name in passes:
        pet.set(pose, face_spec, light_spec, pass_name, props)
        out[pass_name] = rnd.shot()
    if "mask" in out:
        out["mask"] = mask_alpha(out["mask"])
    return out


def touches_edge(alpha):
    return bool((alpha[0] > 0.02).any() or (alpha[-1] > 0.02).any()
                or (alpha[:, 0] > 0.02).any() or (alpha[:, -1] > 0.02).any())


def render_full(pet, rnd, cfg, out_dir):
    scale = int(cfg.get("scale", 2))
    fw, fh = FRAME_CSS[0] * scale, FRAME_CSS[1] * scale
    passes = cfg.get("passes", ["base", "mask", "gloss"])
    names = cfg.get("only") or list(ANIMATIONS)
    unknown = [n for n in names if n not in ANIMATIONS]
    if unknown:
        raise ValueError("unknown animations in \"only\": %s" % unknown)
    aim_camera(pet.scene, (fw, fh))
    manifest_anims = {}
    for name in names:
        _, n, fps, loop, nxt = ANIMATIONS[name]
        t_start = time.time()
        sheets = {p: np.zeros((fh, fw * n, 4)) for p in passes}
        edge = []
        for i in range(n):
            pose, face_spec, light_spec = sample(name, frame_time(name, i))
            got = render_passes(pet, rnd, pose, face_spec, light_spec, passes, props_for(name))
            x = i * fw
            for p in passes:
                if p == "mask":   # white, with the shell's coverage as alpha
                    sheets[p][:, x:x + fw, :3] = 1.0
                    sheets[p][:, x:x + fw, 3] = got[p]
                else:
                    sheets[p][:, x:x + fw] = got[p]
            ref = got[passes[0]]
            alpha = ref if ref.ndim == 2 else ref[..., 3]
            if touches_edge(alpha):
                edge.append(i)
        files = {}
        for p in passes:
            files[p] = "%s_%s.png" % (p, name)
            write_png(os.path.join(out_dir, files[p]), to_bytes(sheets[p]))
        if edge:
            log("WARNING: %s touches the edge of its frame in frames %s" % (name, edge))
        manifest_anims[name] = {"frames": n, "fps": fps, "loop": loop, "next": nxt, "sheets": files}
        log("%-9s %2d frames, %d passes, %.1fs%s" % (name, n, len(passes), time.time() - t_start,
                                                   "" if not edge else "  (touches edge!)"))
    return manifest_anims, (fw, fh), scale, passes


def write_manifest(out_dir, anims, frame, scale, passes, engine):
    ppu = FRAME_CSS[0] / FRAME_UNITS_W
    right, up, _ = camera_axes()

    def css_point(world):
        v = np.asarray(world, float) - np.asarray(CAM_TARGET)
        return [round(float(v @ right) * ppu + FRAME_CSS[0] / 2, 2),
                round(FRAME_CSS[1] / 2 - float(v @ up) * ppu, 2)]

    manifest = {
        "version": 1,
        "frame": {"width": frame[0], "height": frame[1], "scale": scale,
                  "css_width": FRAME_CSS[0], "css_height": FRAME_CSS[1]},
        "passes": passes,
        "mask": "white, alpha = how much of the pixel is shell (use as a CSS alpha mask)",
        "ground_css_y": css_point((0, 0, 0))[1],
        "drag_anchor_css": css_point((DRAG_ANCHOR[0], 0, DRAG_ANCHOR[1])),
        "walk": {"stride_css_px": round(WALK_STRIDE * ppu * abs(float(right[0])), 2),
                 "cycle_seconds": ANIMATIONS["walk"][1] / ANIMATIONS["walk"][2]},
        "poses": {pose: {"intro": intro, "animation": anim} for pose, (intro, anim) in POSES.items()},
        "animations": anims,
        "blender": bpy.app.version_string,
        "engine": engine,
    }
    with open(os.path.join(out_dir, "manifest.json"), "w") as fh:
        json.dump(manifest, fh, indent=2)
        fh.write("\n")
    log("wrote manifest.json")


PREVIEW_TINTS = {"purple": (0x8d, 0x69, 0xce), "orange": (0xE7, 0x85, 0x31), "silver": (0xA8, 0xAD, 0xB4)}
PREVIEW_BG = (0.20, 0.21, 0.24)
PREVIEW_BG_LIGHT = (0.91, 0.91, 0.93)


def render_preview(pet, rnd, cfg, out_dir):
    """Stills to judge the design by: three views of the standing cat, one key frame per
    animation, and the key frames again at the real size, tinted as the UI will tint them."""
    scene = pet.scene
    made = []

    def save(name, img, bg=PREVIEW_BG):
        canvas = np.zeros((img.shape[0], img.shape[1], 3)) + np.asarray(bg)
        over(canvas, img, 0, 0)
        write_png(os.path.join(out_dir, name), to_bytes(np.dstack([canvas, np.ones(canvas.shape[:2])])))
        made.append(name)
        return canvas

    # 1. the standing cat from three sides, shell tinted so the materials read as they will
    purple = np.array(PREVIEW_TINTS["purple"]) / 255.0
    views = []
    for vname, (yaw, pitch) in VIEWS.items():
        aim_camera(scene, (512, 512), yaw, pitch, units_w=9.4, target=(-0.4, 0.0, 3.1))
        got = render_passes(pet, rnd, STAND, face(), lights(0.8, 0.8), ["base", "mask", "gloss"])
        img = tinted(got["base"], got["mask"], got["gloss"], purple if vname == "hero" else (1, 1, 1))
        views.append(save("preview_%s.png" % vname, img))
        log("preview: %s view" % vname)

    # 2. one key frame per animation, in the sprite's own framing, five times the CSS size
    big = (FRAME_CSS[0] * 5, FRAME_CSS[1] * 5)
    keys_big = []
    # "only" (stage 29.8): just these animations' key frames
    names = cfg.get("only") or list(ANIMATIONS)
    for name in names:
        aim_camera(scene, big)
        pose, face_spec, light_spec = sample(name, KEY_FRAME[name])
        got = render_passes(pet, rnd, pose, face_spec, light_spec, ["base"], props_for(name))
        if touches_edge(got["base"][..., 3]):
            log("WARNING: the %s key frame touches the edge of its frame" % name)
        keys_big.append(save("preview_%s.png" % name, got["base"]))
        log("preview: %s" % name)

    # 3. the same key frames at the real 2x size, tinted, and at 1x
    small = (FRAME_CSS[0] * 2, FRAME_CSS[1] * 2)
    aim_camera(scene, small)
    strips = {tint: [] for tint in PREVIEW_TINTS}
    for name in names:
        pose, face_spec, light_spec = sample(name, KEY_FRAME[name])
        got = render_passes(pet, rnd, pose, face_spec, light_spec, ["base", "mask", "gloss"], props_for(name))
        for tint, rgb in PREVIEW_TINTS.items():
            strips[tint].append(tinted(got["base"], got["mask"], got["gloss"], np.array(rgb) / 255.0))
    log("preview: tinted real-size frames")

    # 4. the contact sheet
    cols = 4
    rows_big = (len(keys_big) + cols - 1) // cols
    n = len(names)
    sheet_w = max(cols * big[0], n * small[0])
    strip_h = small[1] + small[1] // 2
    sheet_h = 512 + rows_big * big[1] + len(PREVIEW_TINTS) * strip_h
    sheet = np.zeros((sheet_h, sheet_w, 3)) + np.asarray(PREVIEW_BG)
    for k, v in enumerate(views):
        sheet[0:512, k * 512:(k + 1) * 512] = v
    for k, img in enumerate(keys_big):
        x, y = (k % cols) * big[0], 512 + (k // cols) * big[1]
        sheet[y:y + big[1], x:x + big[0]] = img
    y = 512 + rows_big * big[1]
    for row, tint in enumerate(PREVIEW_TINTS):
        if row == len(PREVIEW_TINTS) - 1:   # the last tint on a light background
            sheet[y:y + strip_h, :] = PREVIEW_BG_LIGHT
        for k, img in enumerate(strips[tint]):
            over(sheet, img, k * small[0], y)
            over(sheet, halve(img), k * small[0] + small[0] // 4, y + small[1])
        y += strip_h
    write_png(os.path.join(out_dir, "preview.png"), to_bytes(np.dstack([sheet, np.ones(sheet.shape[:2])])))
    made.append("preview.png")
    with open(os.path.join(out_dir, "preview.json"), "w") as fh:
        json.dump({"files": made, "key_frames": list(names), "tints": list(PREVIEW_TINTS),
                   "layout": "preview.png: row 1 hero/front/side; then key frames, 4 a row, in "
                             "key_frames order; then one strip per tint of the key frames at 2x "
                             "with the 1x size under each (last tint on a light background)",
                   "blender": bpy.app.version_string}, fh, indent=2)
        fh.write("\n")
    log("wrote preview.png and %d stills" % (len(made) - 1))


ICON_SIZES = (1024, 512, 256, 128, 64, 48, 32, 16)
ICON_SMALL = 48                 # this size and below use the simplified head
ICON_TEAL = (0x01, 0xA4, 0x92)  # Catbot's teal, the middle of the old icon's gradient
ICON_HEAD_BONES = {"head", "ear_n", "ear_f"}
ICON_FINE = ("head_seam", "ear_ring_n", "ear_ring_f")   # detail dropped from the small icons


def icon_pose(small):
    """The head for the icon: level, ears tipped outwards so the head fills the square (more so
    at the small sizes, where every pixel of head counts)."""
    out = 26 if small else 14
    return P(ky=0, kp=0, kr=0, en_p=6, ef_p=6, en_r=out, ef_r=out)


def render_icon(pet, rnd, cfg, out_dir):
    """The app icon: the cat's head alone, a slight three-quarter turn, shell in Catbot's teal,
    eyes open and ear lights on, on a transparent background. Every size is its own render.
    Sizes of ICON_SMALL and below use a simplified head (bigger solid eyes, larger ear lights,
    no fine seams, a closer crop and a squarer-on view) and are rendered large and averaged
    down, so they stay crisp."""
    tint = np.array(cfg.get("icon_tint", ICON_TEAL), float) / 255.0
    made = {}
    for size in ICON_SIZES:
        small = size <= ICON_SMALL

        def wanted(part, small=small):
            if not set(part.weights) <= ICON_HEAD_BONES or part.name == "neck_joint":
                return False
            return not (small and part.name in ICON_FINE)

        pet.show(wanted, extras=small)
        over_sample = 8 if small else 1
        yaw, pitch = (76.0, 4.0) if small else (66.0, 5.0)
        pose = icon_pose(small)
        seen = [part for _, part in pet.parts if wanted(part) and (small or part.name not in pet.extras)]
        target, units = fit_square(seen, pose, yaw, pitch, margin=0.03 if small else 0.05)
        aim_camera(pet.scene, (size * over_sample, size * over_sample), yaw, pitch, units_w=units,
                   target=tuple(target))
        got = render_passes(pet, rnd, pose, face("open", big=small), lights(1.0, 1.0),
                            ["base", "mask", "gloss"])
        img = tinted(got["base"], got["mask"], got["gloss"], tint)
        for _ in range(3 if small else 0):
            img = halve(img)
        if touches_edge(img[..., 3]):
            log("WARNING: the %d px icon touches the edge of its square" % size)
        write_png(os.path.join(out_dir, "icon_%d.png" % size), to_bytes(img))
        made[size] = img
        log("icon: %4d px%s" % (size, " (simplified)" if small else ""))
    pet.show(lambda part: True)

    # every size side by side on a dark and a light ground, small ones also blown up 4x
    shown = [256, 128, 64, 48, 32, 16]
    row_h, width = 256 + 16, 16 + sum(s + 16 for s in shown) + sum(s * 4 + 16 for s in (48, 32, 16))
    sheet = np.zeros((row_h * 2, width, 3))
    for row, bg in enumerate((PREVIEW_BG, PREVIEW_BG_LIGHT)):
        sheet[row * row_h:(row + 1) * row_h] = bg
        x = 16
        for s in shown:
            over(sheet, made[s], x, row * row_h + 8)
            x += s + 16
        for s in (48, 32, 16):
            big = np.repeat(np.repeat(made[s], 4, axis=0), 4, axis=1)
            over(sheet, big, x, row * row_h + 8)
            x += s * 4 + 16
    write_png(os.path.join(out_dir, "icon_preview.png"), to_bytes(np.dstack([sheet, np.ones(sheet.shape[:2])])))
    log("wrote icon_%s.png and icon_preview.png" % "/".join(str(s) for s in ICON_SIZES))


MARK_FINE = ICON_FINE + ("neck_seam", "haunch_seam_hn", "haunch_seam_hf") + tuple(
    "body_seam%d" % k for k in range(4))
MARK_VIEWS = {"head": (66.0, 5.0), "full": (38.0, 8.0)}   # camera yaw, pitch
MARK_FULL_HEAD = 0.50           # how much of the full mark's height the head (with ears) takes
MARK_SIZES = (256, 64)          # 64 is the simplified version, for marks shown at 32 px and less


def mark_pose(kind, small):
    """The pose of a bot's mark: the icon's head, or the cat sitting, turned a little towards
    the viewer (the full mark is cropped close: see fit_head_crop)."""
    if kind == "head":
        return icon_pose(small)
    return P(SIT, ky=-34, kr=4, en_r=8, ef_r=8)


def render_marks(pet, rnd, cfg, out_dir):
    """The in-app marks (bot avatars) made from the robot cat: `head` and `full`, each a square
    in the three passes (base, mask, gloss) so the app can tint the shell with the bot's colour.
    256 px is the full model; 64 px is simplified like the small icons (bigger solid eyes,
    larger ear lights, no fine seams) and rendered large then averaged down."""
    listing = {"passes": ["base", "mask", "gloss"],
               "mask": "white, alpha = how much of the pixel is shell (use as a CSS alpha mask)",
               "marks": {}}
    for kind, (yaw, pitch) in MARK_VIEWS.items():
        entry = {"files": {}, "eyes": {}}
        for size in MARK_SIZES:
            small = size < 128

            def wanted(part, small=small, kind=kind):
                if kind == "head" and (not set(part.weights) <= ICON_HEAD_BONES or part.name == "neck_joint"):
                    return False
                return not (small and part.name in MARK_FINE)

            pet.show(wanted, extras=small)
            pose = mark_pose(kind, small)
            seen = [part for _, part in pet.parts if wanted(part) and (small or part.name not in pet.extras)]
            if kind == "full":   # cropped close: head and chest large, haunches and tail cut off
                target, units = fit_head_crop(seen, pose, yaw, pitch, head_share=MARK_FULL_HEAD)
            else:
                target, units = fit_square(seen, pose, yaw, pitch, margin=0.03)
            over_sample = 8 if small else 1
            aim_camera(pet.scene, (size * over_sample, size * over_sample), yaw, pitch, units_w=units,
                       target=tuple(target))
            got = render_passes(pet, rnd, pose, face("open", big=small), lights(1.0, 1.0),
                                ["base", "mask", "gloss"])
            mask = np.dstack([np.ones(got["mask"].shape + (3,)), got["mask"]])
            images = {"base": got["base"], "mask": mask, "gloss": got["gloss"]}
            for _ in range(3 if small else 0):
                images = {p: halve(img) for p, img in images.items()}
                # halve() weights colour by alpha; the mask's colour is white throughout
            if kind == "head" and touches_edge(images["base"][..., 3]):
                log("WARNING: the %s mark at %d px touches the edge of its square" % (kind, size))
            entry["files"][str(size)] = {}
            for p, img in images.items():
                name = "mark_%s_%s_%d.png" % (kind, p, size)
                write_png(os.path.join(out_dir, name), to_bytes(img))
                entry["files"][str(size)][p] = name
            # where the eyes are in the picture, as fractions from the top left
            right, up, _ = camera_axes(yaw, pitch)
            xf = solve(pose)
            eyes = {}
            for label, sign in (("near", -1), ("far", 1)):
                v = point_on(xf, "head", on_head(sign * EYE_LAT, EYE_PHI, 1.056)) - target
                eyes[label] = [round(0.5 + float(v @ right) / units, 4), round(0.5 - float(v @ up) / units, 4)]
            entry["eyes"][str(size)] = eyes
            log("mark: %s %3d px%s" % (kind, size, " (simplified)" if small else ""))
        listing["marks"][kind] = entry
    pet.show(lambda part: True)
    listing["eyes"] = ("eye centres as [x, y] fractions of the square from its top left; near is the "
                       "eye on the viewer's side (the left one in the picture), far the other, "
                       "which the head partly hides in the full mark")
    listing["blender"] = bpy.app.version_string
    with open(os.path.join(out_dir, "marks.json"), "w") as fh:
        json.dump(listing, fh, indent=2)
        fh.write("\n")

    # a look at both marks tinted, at the sizes the app shows them
    files = listing["marks"]
    tints = list(PREVIEW_TINTS.values()) + [ICON_TEAL]
    sheet = np.zeros((2 * 272, 16 + len(tints) * (256 + 64 + 32 + 16 + 48), 3))
    for row, bg in enumerate((PREVIEW_BG, PREVIEW_BG_LIGHT)):
        sheet[row * 272:(row + 1) * 272] = bg

    def load(kind, size):
        out = {}
        for p in ("base", "mask", "gloss"):
            out[p] = rnd.read(os.path.join(out_dir, files[kind]["files"][str(size)][p]))
        return out

    x = 16
    for k, rgb in enumerate(tints):
        kind = "head" if k % 2 == 0 else "full"
        for row in range(2):
            y = row * 272 + 8
            big, small = load(kind, 256), load(kind, 64)
            tint = np.array(rgb, float) / 255.0
            over(sheet, tinted(big["base"], big["mask"][..., 3], big["gloss"], tint), x, y)
            s64 = tinted(small["base"], small["mask"][..., 3], small["gloss"], tint)
            over(sheet, s64, x + 264, y)
            s32 = halve(s64)
            over(sheet, s32, x + 264, y + 72)
            over(sheet, halve(s32), x + 264, y + 112)
            b128 = halve(tinted(big["base"], big["mask"][..., 3], big["gloss"], tint))
            over(sheet, halve(halve(b128)), x + 304, y + 72)   # the 256 one at 32, to compare
        x += 256 + 64 + 32 + 16 + 48
    write_png(os.path.join(out_dir, "marks_preview.png"), to_bytes(np.dstack([sheet, np.ones(sheet.shape[:2])])))
    log("wrote marks.json, marks_preview.png and %d mark images" % (len(MARK_VIEWS) * len(MARK_SIZES) * 3))


# The marks alive (stage 31): each mark again as a strip of eight looks, so the app can make a
# bot's picture blink, glance, prick its ears at work and look a question while it waits.
MARK_LOOKS = ("idle", "idle_blink", "glance_near", "glance_far", "working", "working_blink",
              "waiting", "waiting_blink")


def mark_look(kind, small, look):
    """One look of a mark: (pose, face, ear lights)."""
    base = mark_pose(kind, small)
    out = base["en_r"]
    blink = look.endswith("_blink")
    if look.startswith("working"):     # ears pricked up and forward, eyes narrowed on the job
        pose = P(base, en_p=base["en_p"] + 9, ef_p=base["ef_p"] + 9, en_r=out - 9, ef_r=out - 9)
        return pose, face("blink" if blink else "half", sweep=None if blink else 0, lo=0.5, big=small), lights(1.0, 1.0)
    if look.startswith("waiting"):     # head tipped, one ear cocked, eyes wide and amber
        pose = P(base, kr=base["kr"] + 9, en_p=base["en_p"] + 8, en_r=out - 8, ef_p=base["ef_p"] - 6, ef_r=out + 8)
        return pose, face("blink" if blink else "wide", color=AMBER, big=small), lights(1.0, 1.0, AMBER)
    shift = {"glance_near": -2, "glance_far": 2}.get(look, 0)
    return base, face("blink" if blink else "open", lx=shift, big=small), lights(1.0, 1.0)


def _all_points(parts, poses, pick=lambda part: True):
    return np.vstack([skinned(p, solve(pose)) for pose in poses for p in parts if len(p.verts) and pick(p)])


def render_marklife(pet, rnd, cfg, out_dir):
    """The marks as strips of looks (MARK_LOOKS), left to right, in the three passes:
    `marklife_<head|full>_<pass>_<256|64>.png`. Every look of a mark shares one framing, fitted
    round all of them, so nothing jumps when the app steps from one to the next."""
    passes = ["base", "mask", "gloss"]
    listing = {"passes": passes, "looks": list(MARK_LOOKS), "files": {}}
    for kind, (yaw, pitch) in MARK_VIEWS.items():
        listing["files"][kind] = {}
        for size in MARK_SIZES:
            small = size < 128

            def wanted(part, small=small, kind=kind):
                if kind == "head" and (not set(part.weights) <= ICON_HEAD_BONES or part.name == "neck_joint"):
                    return False
                return not (small and part.name in MARK_FINE)

            pet.show(wanted, extras=small)
            looks = [mark_look(kind, small, look) for look in MARK_LOOKS]
            poses = [pose for pose, _, _ in looks]
            seen = [part for _, part in pet.parts if wanted(part) and (small or part.name not in pet.extras)]
            right, up, _ = camera_axes(yaw, pitch)
            if kind == "full":
                # as fit_head_crop, but round the head in every look
                head = _all_points(seen, poses, lambda p: set(p.weights) <= {"head", "ear_n", "ear_f"})
                body = _all_points(seen, poses, lambda p: not any(b.startswith("tail") for b in p.weights))
                hu, hv = head @ right, head @ up
                units = (hv.max() - hv.min()) / MARK_FULL_HEAD
                top = hv.max() + 0.03 * units
                bu, bv = body @ right, body @ up
                inside = bu[bv >= top - units]
                cu = (inside.min() + inside.max()) / 2
                lo, hi = hu.max() + 0.03 * units - units / 2, hu.min() - 0.03 * units + units / 2
                cu = min(max(cu, lo), hi) if lo <= hi else (hu.min() + hu.max()) / 2
                centre = body.mean(axis=0)
                target = centre + right * (cu - centre @ right) + up * (top - units / 2 - centre @ up)
            else:
                pts = _all_points(seen, poses)
                u, v = pts @ right, pts @ up
                centre = pts.mean(axis=0)
                target = centre + right * ((u.min() + u.max()) / 2 - centre @ right) + up * ((v.min() + v.max()) / 2 - centre @ up)
                units = max(u.max() - u.min(), v.max() - v.min()) * 1.06
            over_sample = 8 if small else 1
            aim_camera(pet.scene, (size * over_sample, size * over_sample), yaw, pitch, units_w=units,
                       target=tuple(target))
            sheets = {p: np.zeros((size, size * len(MARK_LOOKS), 4)) for p in passes}
            for i, (pose, face_spec, light_spec) in enumerate(looks):
                got = render_passes(pet, rnd, pose, face_spec, light_spec, passes)
                mask = np.dstack([np.ones(got["mask"].shape + (3,)), got["mask"]])
                images = {"base": got["base"], "mask": mask, "gloss": got["gloss"]}
                for _ in range(3 if small else 0):
                    images = {p: halve(img) for p, img in images.items()}
                if kind == "head" and touches_edge(images["base"][..., 3]):
                    log("WARNING: the %s mark's %s look at %d px touches the edge" % (kind, MARK_LOOKS[i], size))
                for p in passes:
                    sheets[p][:, i * size:(i + 1) * size] = images[p]
            listing["files"][kind][str(size)] = {}
            for p in passes:
                name = "marklife_%s_%s_%d.png" % (kind, p, size)
                write_png(os.path.join(out_dir, name), to_bytes(sheets[p]))
                listing["files"][kind][str(size)][p] = name
            log("marklife: %s %3d px, %d looks" % (kind, size, len(MARK_LOOKS)))
    pet.show(lambda part: True)
    with open(os.path.join(out_dir, "marklife.json"), "w") as fh:
        json.dump(listing, fh, indent=2)
        fh.write("\n")

    # a look at them tinted: the big strips, and the small ones at twice their size
    tint = np.array(PREVIEW_TINTS["purple"], float) / 255.0
    sheet = np.zeros((2 * (256 + 128 + 24), 256 * len(MARK_LOOKS), 3))
    sheet[:] = PREVIEW_BG
    for row, kind in enumerate(MARK_VIEWS):
        y = row * (256 + 128 + 24)
        for size in MARK_SIZES:
            got = {p: rnd.read(os.path.join(out_dir, listing["files"][kind][str(size)][p])) for p in passes}
            img = tinted(got["base"], got["mask"][..., 3], got["gloss"], tint)
            if size == 64:
                img = np.repeat(np.repeat(img, 2, axis=0), 2, axis=1)
                over(sheet, img, 0, y + 256 + 8)
            else:
                over(sheet, img, 0, y)
    write_png(os.path.join(out_dir, "marklife_preview.png"), to_bytes(np.dstack([sheet, np.ones(sheet.shape[:2])])))
    log("wrote marklife.json, marklife_preview.png and %d strips" % (len(MARK_VIEWS) * len(MARK_SIZES) * 3))


def bake_actions(pet, out_dir):
    """Key every animation into its own Action and save the scene, so the rig can be opened and
    played in Blender. The sprite renders do not depend on this."""
    arm = pet.arm
    arm.animation_data_create()
    for name, (_, n, fps, loop, _) in ANIMATIONS.items():
        action = bpy.data.actions.new("pet_" + name)
        action.use_fake_user = True
        arm.animation_data.action = action
        for i in range(n):
            pose, _, _ = sample(name, frame_time(name, i))
            apply_pose(arm, pet.rest, pose)
            for pb in arm.pose.bones:
                pb.keyframe_insert("location", frame=i + 1)
                pb.keyframe_insert("rotation_quaternion", frame=i + 1)
    arm.animation_data.action = bpy.data.actions.get("pet_idle")
    slot_owner = arm.animation_data
    if hasattr(slot_owner, "action_slot") and slot_owner.action_slot is None:
        slots = getattr(slot_owner.action, "slots", None)
        if slots:
            slot_owner.action_slot = slots[0]
    pet.scene.frame_start, pet.scene.frame_end = 1, ANIMATIONS["idle"][1]
    pet.scene.render.fps = 12
    aim_camera(pet.scene, (FRAME_CSS[0] * 2, FRAME_CSS[1] * 2))
    pet.set(STAND, face(), lights(), "base")
    path = os.path.abspath(os.path.join(out_dir, "pet.blend"))
    bpy.ops.wm.save_as_mainfile(filepath=path, check_existing=False)
    log("wrote pet.blend (one Action per animation, named pet_<animation>)")


def main():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    args = {"--config": None, "--out": "art/pet3d/renders"}
    for i in range(0, len(argv) - 1, 2):
        if argv[i] not in args:
            raise SystemExit("unknown argument %s (use --config FILE --out DIR)" % argv[i])
        args[argv[i]] = argv[i + 1]
    cfg = {}
    if args["--config"]:
        with open(args["--config"]) as fh:
            cfg = json.load(fh)
    mode = cfg.get("mode", "preview")
    if mode not in ("preview", "full", "icon", "marks", "marklife"):
        raise ValueError("\"mode\" must be \"preview\", \"full\", \"icon\", \"marks\" or \"marklife\", not %r" % mode)
    out_dir = os.path.abspath(args["--out"])
    os.makedirs(out_dir, exist_ok=True)
    log("Blender %s, mode %s, output %s" % (bpy.app.version_string, mode, out_dir))
    if bpy.app.version < (4, 0, 0):
        raise RuntimeError("Blender 4.0 or newer is needed (this is %s)" % bpy.app.version_string)

    scene = reset_scene()
    setup_render(scene)
    engine = set_engine(scene, cfg.get("engine", "auto"), int(cfg.get("samples", 24)))
    lamps, bg = setup_lights(scene)
    setup_camera(scene)
    pet = Pet(scene)
    rnd = Renderer(scene, os.path.join(out_dir, "_tmp"))

    try:
        calibrate(pet, rnd, lamps, bg, float(cfg.get("shell_level", 0.80)))
    except RuntimeError as exc:
        if engine == "CYCLES" or cfg.get("engine", "auto") != "auto" or rnd.count > 0:
            raise
        log("Eevee could not render here (%s); falling back to Cycles on the CPU" % exc)
        engine = set_engine(scene, "cycles", int(cfg.get("samples", 24)))
        calibrate(pet, rnd, lamps, bg, float(cfg.get("shell_level", 0.80)))

    if mode == "preview":
        render_preview(pet, rnd, cfg, out_dir)
    elif mode == "icon":
        render_icon(pet, rnd, cfg, out_dir)
    elif mode == "marks":
        render_marks(pet, rnd, cfg, out_dir)
    elif mode == "marklife":
        render_marklife(pet, rnd, cfg, out_dir)
    else:
        anims, frame, scale, passes = render_full(pet, rnd, cfg, out_dir)
        if cfg.get("only"):
            log("\"only\" is set: manifest.json is left as it was (it must list every animation)")
        else:
            write_manifest(out_dir, anims, frame, scale, passes, engine)
    if cfg.get("save_blend", True) and mode in ("preview", "full"):
        try:
            bake_actions(pet, out_dir)
        except Exception as exc:  # the .blend is a convenience; the renders above are done
            log("WARNING: could not save pet.blend: %r" % exc)
    for leftover in ("frame.png",):
        path = os.path.join(rnd.tmp, leftover)
        if os.path.exists(path):
            os.remove(path)
    try:
        os.rmdir(rnd.tmp)
    except OSError:
        pass
    log("done: %d renders" % rnd.count)


if __name__ == "__main__":
    if bpy is None:
        raise SystemExit("run this inside Blender: blender -b --factory-startup -P build_pet.py -- ...")
    main()
