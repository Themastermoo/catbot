# Catbot Bridge: lets Catbot's bots see and work in this Blender, on this computer only.
#
# Catbot installs this file into Blender's add-ons folder, with `catbot_bridge.json` beside it
# (the port and a key). Turned on in Edit > Preferences > Add-ons, it listens on 127.0.0.1 only
# and answers a request only when it carries that key. Catbot decides what a bot may do and asks
# the person before anything that changes the scene, renders or saves; this add-on just does
# what Catbot sends. Everything that touches Blender runs on Blender's own main thread.
#
# One request per connection: a line of JSON `{"key", "op", "args"}` in, a line of JSON
# `{"ok": true, "result": ...}` or `{"ok": false, "error": "..."}` out.
#
# Part of Catbot (Apache-2.0).

bl_info = {
    "name": "Catbot Bridge",
    "author": "Catbot",
    "version": (1, 1, 0),
    "blender": (3, 6, 0),
    "location": "Runs in the background while turned on",
    "description": "Lets Catbot's bots see this Blender and, with your approval in Catbot, change, render and save",
    "category": "System",
}

import base64
import contextlib
import hmac
import io
import json
import math
import os
import queue
import socket
import tempfile
import threading
import time
import traceback

import bpy
import mathutils

BRIDGE_VERSION = 2
DEFAULT_PORT = 9871
# Most objects listed in one answer.
MOST_OBJECTS = 300
# Longest a request may take on the main thread (a render can be slow).
LONGEST = 60 * 60
# Widest picture sent back.
WIDEST = 1280

_jobs = queue.Queue()
_server = None


# ---- the settings Catbot left beside this file -------------------------------------------------

def _config():
    path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "catbot_bridge.json")
    try:
        with open(path, encoding="utf-8") as f:
            found = json.load(f)
    except (OSError, ValueError):
        return None
    key = str(found.get("key") or "")
    if len(key) < 16:
        return None
    return {"port": int(found.get("port") or DEFAULT_PORT), "key": key}


# ---- looking -----------------------------------------------------------------------------------

def _vec(v, digits=4):
    return [round(float(x), digits) for x in v]


def _degrees(euler):
    return [round(math.degrees(a), 3) for a in euler]


def _collection_of(obj):
    return [c.name for c in obj.users_collection]


def op_info(args):
    scene = bpy.context.scene
    return {
        "bridge": BRIDGE_VERSION,
        "blender": bpy.app.version_string,
        "file": bpy.data.filepath or None,
        "unsaved": bool(bpy.data.is_dirty),
        "scene": scene.name,
        "scenes": [s.name for s in bpy.data.scenes],
        "frame": scene.frame_current,
        "frames": [scene.frame_start, scene.frame_end],
        "fps": scene.render.fps,
        "engine": scene.render.engine,
        "resolution": [scene.render.resolution_x, scene.render.resolution_y, scene.render.resolution_percentage],
        "camera": scene.camera.name if scene.camera else None,
        "units": scene.unit_settings.system,
        "objects": len(scene.objects),
        "materials": len(bpy.data.materials),
        "selected": [o.name for o in getattr(bpy.context, "selected_objects", []) or []],
        "active": bpy.context.view_layer.objects.active.name if bpy.context.view_layer.objects.active else None,
    }


def _brief(obj):
    out = {
        "name": obj.name,
        "type": obj.type,
        "location": _vec(obj.location),
        "rotation": _degrees(obj.rotation_euler),
        "scale": _vec(obj.scale),
        "dimensions": _vec(obj.dimensions),
        "parent": obj.parent.name if obj.parent else None,
        "collections": _collection_of(obj),
        "hidden": bool(obj.hide_get()) if hasattr(obj, "hide_get") else bool(obj.hide_viewport),
        "hiddenInRender": bool(obj.hide_render),
    }
    mats = [s.material.name for s in getattr(obj, "material_slots", []) if s.material]
    if mats:
        out["materials"] = mats
    return out


def op_scene(args):
    scene = bpy.context.scene
    objects = list(scene.objects)
    shown = [_brief(o) for o in objects[:MOST_OBJECTS]]
    out = {"info": op_info({}), "objects": shown}
    if len(objects) > MOST_OBJECTS:
        out["more"] = len(objects) - MOST_OBJECTS
    out["collections"] = [c.name for c in bpy.data.collections]
    return out


def _object(name):
    obj = bpy.data.objects.get(str(name or ""))
    if obj is None:
        raise ValueError("there's no object called %r" % name)
    return obj


def op_object(args):
    obj = _object(args.get("name"))
    out = _brief(obj)
    out["children"] = [c.name for c in obj.children]
    out["modifiers"] = [{"name": m.name, "type": m.type, "shown": bool(m.show_viewport)} for m in getattr(obj, "modifiers", [])]
    out["customProperties"] = {k: str(obj[k])[:200] for k in obj.keys() if not k.startswith("_")}
    corners = [obj.matrix_world @ mathutils.Vector(c) for c in obj.bound_box]
    out["boundsWorld"] = {
        "min": _vec([min(c[i] for c in corners) for i in range(3)]),
        "max": _vec([max(c[i] for c in corners) for i in range(3)]),
    }
    data = obj.data
    if obj.type == "MESH" and data is not None:
        out["mesh"] = {"vertices": len(data.vertices), "edges": len(data.edges), "faces": len(data.polygons)}
    elif obj.type == "LIGHT":
        out["light"] = {"kind": data.type, "power": round(float(data.energy), 3), "color": _vec(data.color)}
    elif obj.type == "CAMERA":
        out["camera"] = {"lens": round(float(data.lens), 3), "kind": data.type}
    elif obj.type == "FONT":
        out["text"] = data.body[:500]
    slots = []
    for s in getattr(obj, "material_slots", []):
        m = s.material
        if m is None:
            continue
        slot = {"name": m.name, "viewportColor": _vec(m.diffuse_color, 3)}
        bsdf = _principled(m, make=False)
        if bsdf is not None:
            slot["baseColor"] = _vec(bsdf.inputs["Base Color"].default_value, 3)
            slot["metallic"] = round(float(bsdf.inputs["Metallic"].default_value), 3)
            slot["roughness"] = round(float(bsdf.inputs["Roughness"].default_value), 3)
        slots.append(slot)
    out["materialDetails"] = slots
    return out


def _view3d():
    """A window, area and region showing the 3D viewport, or None."""
    wm = bpy.context.window_manager
    for window in getattr(wm, "windows", []):
        for area in window.screen.areas:
            if area.type == "VIEW_3D":
                region = next((r for r in area.regions if r.type == "WINDOW"), None)
                if region is not None:
                    return window, area, region
    return None


def _picture(path):
    """A file's picture as a JPEG no wider than WIDEST, base64."""
    img = bpy.data.images.load(path, check_existing=False)
    try:
        w, h = img.size
        if w == 0 or h == 0:
            raise ValueError("that picture is empty")
        if w > WIDEST:
            img.scale(WIDEST, max(1, round(h * WIDEST / w)))
        fd, out = tempfile.mkstemp(suffix=".jpg")
        os.close(fd)
        try:
            img.filepath_raw = out
            img.file_format = "JPEG"
            img.save()
            with open(out, "rb") as f:
                data = f.read()
        finally:
            with contextlib.suppress(OSError):
                os.remove(out)
    finally:
        bpy.data.images.remove(img)
    return {"mimeType": "image/jpeg", "data": base64.b64encode(data).decode("ascii"), "size": [w, h]}


def op_look(args):
    if args.get("path"):
        path = str(args["path"])
        if not os.path.isfile(path):
            raise ValueError("there's no picture at %s" % path)
        return _picture(path)
    found = _view3d()
    if found is None:
        raise ValueError("no 3D viewport is open in Blender")
    window, area, region = found
    fd, shot = tempfile.mkstemp(suffix=".png")
    os.close(fd)
    try:
        with bpy.context.temp_override(window=window, area=area, region=region):
            bpy.ops.screen.screenshot_area(filepath=shot)
        return _picture(shot)
    finally:
        with contextlib.suppress(OSError):
            os.remove(shot)


# ---- changing ----------------------------------------------------------------------------------

def _principled(mat, make=True):
    if make and hasattr(mat, "use_nodes") and not mat.use_nodes:
        with contextlib.suppress(Exception):
            mat.use_nodes = True
    tree = getattr(mat, "node_tree", None)
    if tree is None:
        return None
    for node in tree.nodes:
        if node.type == "BSDF_PRINCIPLED":
            return node
    if not make:
        return None
    node = tree.nodes.new("ShaderNodeBsdfPrincipled")
    output = next((n for n in tree.nodes if n.type == "OUTPUT_MATERIAL"), None) or tree.nodes.new("ShaderNodeOutputMaterial")
    tree.links.new(node.outputs["BSDF"], output.inputs["Surface"])
    return node


def _target_collection(step):
    name = step.get("collection")
    if name:
        col = bpy.data.collections.get(name)
        if col is None:
            col = bpy.data.collections.new(name)
            bpy.context.scene.collection.children.link(col)
        return col
    return bpy.context.view_layer.active_layer_collection.collection


def _torus(bm, major=1.0, minor=0.25, rings=48, sides=12):
    verts = []
    for i in range(rings):
        a = 2 * math.pi * i / rings
        for j in range(sides):
            b = 2 * math.pi * j / sides
            r = major + minor * math.cos(b)
            verts.append(bm.verts.new((r * math.cos(a), r * math.sin(a), minor * math.sin(b))))
    for i in range(rings):
        for j in range(sides):
            a = verts[i * sides + j]
            b = verts[((i + 1) % rings) * sides + j]
            c = verts[((i + 1) % rings) * sides + (j + 1) % sides]
            d = verts[i * sides + (j + 1) % sides]
            bm.faces.new((a, b, c, d))


MESHES = ("cube", "sphere", "ico_sphere", "cylinder", "cone", "plane", "torus", "monkey")
OTHERS = ("empty", "camera", "light", "text", "image")


def _picture_file(path):
    """A picture on this computer, loaded into Blender (once)."""
    path = str(path or "")
    if not os.path.isabs(path) or not os.path.isfile(path):
        raise ValueError("there's no picture at %r (give its full path)" % path)
    return bpy.data.images.load(path, check_existing=True)


def _texture(mat, bsdf, image):
    """Puts a picture into a material's colour."""
    tree = mat.node_tree
    node = next((n for n in tree.nodes if n.type == "TEX_IMAGE" and n.label == "Catbot picture"), None) or tree.nodes.new("ShaderNodeTexImage")
    node.label = "Catbot picture"
    node.image = image
    node.location = (bsdf.location.x - 320, bsdf.location.y)
    tree.links.new(node.outputs["Color"], bsdf.inputs["Base Color"])
    if "Alpha" in node.outputs:
        tree.links.new(node.outputs["Alpha"], bsdf.inputs["Alpha"])


def _mesh(kind, size):
    import bmesh

    bm = bmesh.new()
    try:
        r = size / 2.0
        if kind == "cube":
            bmesh.ops.create_cube(bm, size=size)
        elif kind == "sphere":
            bmesh.ops.create_uvsphere(bm, u_segments=32, v_segments=16, radius=r)
        elif kind == "ico_sphere":
            bmesh.ops.create_icosphere(bm, subdivisions=2, radius=r)
        elif kind in ("cylinder", "cone"):
            bmesh.ops.create_cone(bm, cap_ends=True, cap_tris=False, segments=32, radius1=r, radius2=r if kind == "cylinder" else 0.0, depth=size)
        elif kind == "plane":
            bmesh.ops.create_grid(bm, x_segments=1, y_segments=1, size=r)
        elif kind == "torus":
            _torus(bm, major=r * 0.75, minor=r * 0.25)
        elif kind == "monkey":
            bmesh.ops.create_monkey(bm)
            if size != 2.0:
                bmesh.ops.scale(bm, vec=(r, r, r), verts=bm.verts)
        mesh = bpy.data.meshes.new(kind)
        bm.to_mesh(mesh)
    finally:
        bm.free()
    return mesh


def _place(obj, step):
    if "location" in step:
        obj.location = [float(x) for x in step["location"]]
    if "rotation" in step:
        obj.rotation_euler = [math.radians(float(x)) for x in step["rotation"]]
    if "scale" in step:
        s = step["scale"]
        obj.scale = [float(s)] * 3 if isinstance(s, (int, float)) else [float(x) for x in s]


def _add(step):
    kind = str(step.get("kind") or "").lower()
    name = str(step.get("name") or kind.replace("_", " ").title() or "Object")
    size = float(step.get("size") or 2.0)
    if kind in MESHES:
        data = _mesh(kind, size)
    elif kind == "empty":
        data = None
    elif kind == "camera":
        data = bpy.data.cameras.new(name)
        if "lens" in step:
            data.lens = float(step["lens"])
    elif kind == "light":
        light_kind = str(step.get("light") or "POINT").upper()
        if light_kind not in ("POINT", "SUN", "SPOT", "AREA"):
            raise ValueError("a light is point, sun, spot or area")
        data = bpy.data.lights.new(name, light_kind)
        if "power" in step:
            data.energy = float(step["power"])
        if "color" in step:
            data.color = [float(x) for x in step["color"]][:3]
    elif kind == "text":
        data = bpy.data.curves.new(name, "FONT")
        data.body = str(step.get("text") or name)
    elif kind == "image":
        # A flat card showing a picture, its shape the picture's.
        image = _picture_file(step.get("path"))
        w, h = image.size
        if not step.get("name"):
            name = os.path.splitext(os.path.basename(image.filepath))[0]
        data = _mesh("plane", 2.0)
        aspect = (w / h) if h else 1.0
        for v in data.vertices:
            v.co.x *= size / 2.0 * (aspect if aspect >= 1 else 1)
            v.co.y *= size / 2.0 * (1 if aspect >= 1 else 1 / aspect)
        mat = bpy.data.materials.new(name + " picture")
        bsdf = _principled(mat)
        if bsdf is not None:
            _texture(mat, bsdf, image)
        data.materials.append(mat)
    else:
        raise ValueError("can't add a %r; kinds are %s" % (kind, ", ".join(MESHES + OTHERS)))
    obj = bpy.data.objects.new(name, data)
    _target_collection(step).objects.link(obj)
    _place(obj, step)
    if kind == "camera" and step.get("active"):
        bpy.context.scene.camera = obj
    return "added %s %s" % (kind, obj.name)


def _material(step):
    obj = _object(step.get("object"))
    if not hasattr(obj.data, "materials"):
        raise ValueError("%s can't have a material" % obj.name)
    name = str(step.get("name") or (obj.name + " material"))
    mat = bpy.data.materials.get(name) or bpy.data.materials.new(name)
    bsdf = _principled(mat)
    color = step.get("color")
    if color is not None:
        rgba = [float(x) for x in color] + [1.0] * (4 - len(color))
        mat.diffuse_color = rgba[:4]
        if bsdf is not None:
            bsdf.inputs["Base Color"].default_value = rgba[:4]
    if step.get("texture") and bsdf is not None:
        _texture(mat, bsdf, _picture_file(step["texture"]))
    if bsdf is not None:
        if "metallic" in step:
            bsdf.inputs["Metallic"].default_value = float(step["metallic"])
            mat.metallic = float(step["metallic"])
        if "roughness" in step:
            bsdf.inputs["Roughness"].default_value = float(step["roughness"])
            mat.roughness = float(step["roughness"])
        if "emission" in step:
            em = [float(x) for x in step["emission"]] + [1.0] * (4 - len(step["emission"]))
            socket_name = "Emission Color" if "Emission Color" in bsdf.inputs else "Emission"
            bsdf.inputs[socket_name].default_value = em[:4]
            bsdf.inputs["Emission Strength"].default_value = float(step.get("emissionStrength", 1.0))
    if step.get("replace", True):
        obj.data.materials.clear()
    obj.data.materials.append(mat)
    return "gave %s the material %s" % (obj.name, mat.name)


def _do(step):
    action = str(step.get("action") or "")
    if action == "add":
        return _add(step)
    if action == "transform":
        obj = _object(step.get("name"))
        _place(obj, step)
        return "moved %s" % obj.name
    if action == "delete":
        obj = _object(step.get("name"))
        name = obj.name
        bpy.data.objects.remove(obj, do_unlink=True)
        return "deleted %s" % name
    if action == "rename":
        obj = _object(step.get("name"))
        old = obj.name
        obj.name = str(step.get("to") or old)
        return "renamed %s to %s" % (old, obj.name)
    if action == "duplicate":
        obj = _object(step.get("name"))
        twin = obj.copy()
        if obj.data is not None and not step.get("linked"):
            twin.data = obj.data.copy()
        if step.get("to"):
            twin.name = str(step["to"])
        for col in obj.users_collection:
            col.objects.link(twin)
        _place(twin, step)
        return "duplicated %s as %s" % (obj.name, twin.name)
    if action == "material":
        return _material(step)
    if action == "parent":
        obj = _object(step.get("name"))
        parent = _object(step["parent"]) if step.get("parent") else None
        world = obj.matrix_world.copy()
        obj.parent = parent
        obj.matrix_world = world
        return "parented %s to %s" % (obj.name, parent.name if parent else "nothing")
    if action == "hide":
        obj = _object(step.get("name"))
        hidden = bool(step.get("hidden", True))
        obj.hide_set(hidden)
        if "render" in step:
            obj.hide_render = bool(step["render"])
        return "%s %s" % ("hid" if hidden else "showed", obj.name)
    if action == "frame":
        scene = bpy.context.scene
        if "start" in step:
            scene.frame_start = int(step["start"])
        if "end" in step:
            scene.frame_end = int(step["end"])
        if "current" in step:
            scene.frame_set(int(step["current"]))
        return "frames %d-%d, at %d" % (scene.frame_start, scene.frame_end, scene.frame_current)
    if action == "keyframe":
        obj = _object(step.get("name"))
        frame = int(step.get("at", bpy.context.scene.frame_current))
        _place(obj, step)
        what = step.get("what") or ["location", "rotation_euler", "scale"]
        for path in what:
            obj.keyframe_insert(data_path=path, frame=frame)
        return "keyed %s at frame %d" % (obj.name, frame)
    if action == "select":
        for o in bpy.context.view_layer.objects:
            o.select_set(False)
        names = step.get("names") or []
        for n in names:
            _object(n).select_set(True)
        if names:
            bpy.context.view_layer.objects.active = _object(names[0])
        return "selected %s" % (", ".join(names) or "nothing")
    raise ValueError("there's no change called %r" % action)


def _undo_point(message):
    with contextlib.suppress(Exception):
        bpy.ops.ed.undo_push(message=message)


def op_change(args):
    steps = args.get("steps") or []
    if not isinstance(steps, list) or not steps:
        raise ValueError("give steps: a list of changes")
    done = []
    _undo_point("Before Catbot's change")
    for i, step in enumerate(steps):
        try:
            done.append(_do(step))
        except Exception as e:
            raise ValueError("step %d (%s) failed: %s; the %d before it were done" % (i + 1, step.get("action"), e, i))
    _undo_point("Catbot's change")
    return {"done": done}


def op_python(args):
    code = str(args.get("code") or "")
    if not code.strip():
        raise ValueError("there's no code to run")
    space = {"bpy": bpy, "mathutils": mathutils, "C": bpy.context, "D": bpy.data, "__name__": "__catbot__"}
    printed = io.StringIO()
    _undo_point("Before Catbot's script")
    try:
        with contextlib.redirect_stdout(printed), contextlib.redirect_stderr(printed):
            exec(compile(code, "<catbot>", "exec"), space)
    except Exception:
        raise ValueError((printed.getvalue()[-4000:] + "\n" + traceback.format_exc(limit=6)).strip())
    _undo_point("Catbot's script")
    out = {"printed": printed.getvalue()[-20000:]}
    if "result" in space:
        try:
            json.dumps(space["result"])
            out["result"] = space["result"]
        except (TypeError, ValueError):
            out["result"] = repr(space["result"])[:4000]
    return out


def _with_window(fn):
    """Runs an operator with a window in context when there is one (some need it)."""
    found = _view3d()
    if found is None:
        return fn()
    window, area, region = found
    with bpy.context.temp_override(window=window, area=area, region=region):
        return fn()


def op_render(args):
    scene = bpy.context.scene
    path = str(args.get("path") or "")
    if not path:
        raise ValueError("Catbot gives a path for the picture")
    os.makedirs(os.path.dirname(path), exist_ok=True)
    keep = (scene.render.filepath, scene.render.resolution_percentage, scene.frame_current, scene.render.image_settings.file_format)
    try:
        scene.render.filepath = path
        scene.render.image_settings.file_format = "PNG"
        if args.get("percent"):
            scene.render.resolution_percentage = max(1, min(100, int(args["percent"])))
        if args.get("frame") is not None:
            scene.frame_set(int(args["frame"]))
        if scene.camera is None:
            raise ValueError("the scene has no camera to render from")
        started = time.time()
        _with_window(lambda: bpy.ops.render.render(write_still=True))
        took = round(time.time() - started, 1)
    finally:
        scene.render.filepath, scene.render.resolution_percentage = keep[0], keep[1]
        scene.render.image_settings.file_format = keep[3]
        if scene.frame_current != keep[2]:
            scene.frame_set(keep[2])
    if not os.path.isfile(path):
        raise ValueError("Blender rendered but didn't write %s" % path)
    return {"path": path, "seconds": took, "engine": scene.render.engine}


EXPORTS = ("glb", "gltf", "fbx", "obj", "stl", "usd")


def op_save(args):
    kind = str(args.get("kind") or "blend").lower()
    path = str(args.get("path") or "")
    if kind == "blend":
        if path:
            os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
            _with_window(lambda: bpy.ops.wm.save_as_mainfile(filepath=path, copy=bool(args.get("copy"))))
        elif bpy.data.filepath:
            path = bpy.data.filepath
            _with_window(lambda: bpy.ops.wm.save_mainfile())
        else:
            raise ValueError("this file was never saved; give a path")
        return {"saved": path}
    if kind not in EXPORTS:
        raise ValueError("can save blend, or export %s" % ", ".join(EXPORTS))
    if not path:
        raise ValueError("Catbot gives a path to export to")
    os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
    selected = bool(args.get("selectedOnly"))
    if kind in ("glb", "gltf"):
        fmt = "GLB" if kind == "glb" else "GLTF_SEPARATE"
        _with_window(lambda: bpy.ops.export_scene.gltf(filepath=path, export_format=fmt, use_selection=selected))
    elif kind == "fbx":
        _with_window(lambda: bpy.ops.export_scene.fbx(filepath=path, use_selection=selected))
    elif kind == "obj":
        _with_window(lambda: bpy.ops.wm.obj_export(filepath=path, export_selected_objects=selected))
    elif kind == "stl":
        _with_window(lambda: bpy.ops.wm.stl_export(filepath=path, export_selected_objects=selected))
    elif kind == "usd":
        _with_window(lambda: bpy.ops.wm.usd_export(filepath=path, selected_objects_only=selected))
    return {"exported": path, "kind": kind}


OPS = {
    "info": op_info,
    "scene": op_scene,
    "object": op_object,
    "look": op_look,
    "change": op_change,
    "python": op_python,
    "render": op_render,
    "save": op_save,
}


def handle(op, args):
    """Does one request; runs on Blender's main thread."""
    fn = OPS.get(op)
    if fn is None:
        return {"ok": False, "error": "there's no request called %r" % op}
    try:
        return {"ok": True, "result": fn(args if isinstance(args, dict) else {})}
    except Exception as e:
        return {"ok": False, "error": str(e) or e.__class__.__name__}


# ---- listening ---------------------------------------------------------------------------------

def pump():
    """Blender's main thread: does waiting requests. Called by a timer."""
    for _ in range(8):
        try:
            op, args, box, done = _jobs.get_nowait()
        except queue.Empty:
            break
        box["reply"] = handle(op, args)
        done.set()
    return 0.05


class Server:
    def __init__(self, port, key):
        self.port = port
        self.key = key.encode("utf-8")
        self.stopping = threading.Event()
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.sock.bind(("127.0.0.1", port))
        self.sock.listen(8)
        self.sock.settimeout(0.5)
        self.thread = threading.Thread(target=self._serve, name="catbot-bridge", daemon=True)
        self.thread.start()

    def stop(self):
        self.stopping.set()
        with contextlib.suppress(OSError):
            self.sock.close()

    def _serve(self):
        while not self.stopping.is_set():
            try:
                conn, _ = self.sock.accept()
            except socket.timeout:
                continue
            except OSError:
                break
            threading.Thread(target=self._one, args=(conn,), daemon=True).start()

    def _one(self, conn):
        with conn:
            conn.settimeout(30)
            try:
                line = b""
                while not line.endswith(b"\n") and len(line) < 4_000_000:
                    part = conn.recv(65536)
                    if not part:
                        break
                    line += part
                ask = json.loads(line.decode("utf-8"))
            except (OSError, ValueError):
                return
            given = str(ask.get("key") or "").encode("utf-8")
            if not hmac.compare_digest(given, self.key):
                reply = {"ok": False, "error": "wrong key"}
            else:
                box, done = {}, threading.Event()
                _jobs.put((str(ask.get("op") or ""), ask.get("args") or {}, box, done))
                if done.wait(LONGEST):
                    reply = box["reply"]
                else:
                    reply = {"ok": False, "error": "Blender took too long"}
            with contextlib.suppress(OSError):
                conn.settimeout(None)
                conn.sendall(json.dumps(reply).encode("utf-8") + b"\n")


def register():
    global _server
    config = _config()
    if config is None:
        print("Catbot Bridge: catbot_bridge.json is missing; install the add-on again from Catbot (Settings > Apps > Blender)")
        return
    try:
        _server = Server(config["port"], config["key"])
    except OSError as e:
        print("Catbot Bridge: couldn't listen on port %d: %s" % (config["port"], e))
        _server = None
        return
    if not bpy.app.timers.is_registered(pump):
        bpy.app.timers.register(pump, persistent=True)
    print("Catbot Bridge: listening on 127.0.0.1:%d" % config["port"])


def unregister():
    global _server
    if bpy.app.timers.is_registered(pump):
        bpy.app.timers.unregister(pump)
    if _server is not None:
        _server.stop()
        _server = None
