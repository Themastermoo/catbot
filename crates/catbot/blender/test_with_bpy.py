"""Tries the Catbot Bridge add-on against real Blender, without its window.

    python -m venv /tmp/bpy && /tmp/bpy/bin/pip install bpy
    /tmp/bpy/bin/python crates/catbot/blender/test_with_bpy.py

bpy is Blender as a Python module (it needs the Python version that Blender release uses). The
viewport picture needs Blender's window, so that one is expected to fail here.
"""
import sys, os, json, socket, threading, time, tempfile
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import bpy
print("blender", bpy.app.version_string)
import catbot_bridge as cb
h = cb.handle
def ok(op, args={}):
    r = h(op, args)
    assert r["ok"], (op, r)
    return r["result"]
info = ok("info"); print("info", info["blender"], info["objects"], info["engine"])
sc = ok("scene"); print("objects", [o["name"] for o in sc["objects"]])
steps = [
 {"action":"add","kind":k,"name":"T_"+k,"location":[i*3,0,0]} for i,k in enumerate(cb.MESHES)
] + [
 {"action":"add","kind":"light","light":"sun","name":"Sun2","power":3},
 {"action":"add","kind":"camera","name":"Cam2","location":[0,-10,3],"rotation":[75,0,0],"active":True},
 {"action":"add","kind":"text","text":"Hi Moo","name":"Hello"},
 {"action":"add","kind":"empty","name":"Pivot"},
 {"action":"material","object":"T_cube","color":[0.1,0.6,0.6],"metallic":0.5,"roughness":0.3,"emission":[1,0,0],"emissionStrength":2},
 {"action":"transform","name":"T_sphere","location":[0,5,0],"rotation":[0,0,45],"scale":2},
 {"action":"duplicate","name":"T_cube","to":"T_cube2","location":[0,0,5]},
 {"action":"parent","name":"T_cube2","parent":"Pivot"},
 {"action":"rename","name":"T_cone","to":"Cone!"},
 {"action":"hide","name":"T_plane","render":True},
 {"action":"frame","start":1,"end":48,"current":10},
 {"action":"keyframe","name":"T_torus","at":10,"location":[1,1,1]},
 {"action":"select","names":["T_cube","T_monkey"]},
 {"action":"delete","name":"T_monkey"},
]
print(ok("change", {"steps": steps})["done"])
o = ok("object", {"name":"T_cube"}); print("cube", o["materialDetails"], o["mesh"], o["boundsWorld"])
for k in cb.MESHES:
    if k in ("monkey","cone"): continue
    print(k, ok("object", {"name":"T_"+k}).get("mesh"))
r = h("change", {"steps":[{"action":"transform","name":"T_cube","location":[9,9,9]},{"action":"delete","name":"nope"}]}); print("bad step ->", r)
print("py", ok("python", {"code":"print(len(D.objects)); result={'n':len(D.objects)}"}))
print("pyerr", h("python", {"code":"1/0"})["error"][-60:])
tmp = tempfile.mkdtemp()
bpy.context.scene.render.engine = "CYCLES"
bpy.context.scene.cycles.samples = 4
bpy.context.scene.render.resolution_x, bpy.context.scene.render.resolution_y = 320, 240
r = ok("render", {"path": os.path.join(tmp,"r","a.png"), "frame": 5}); print("render", r)
pic = ok("look", {"path": r["path"]}); print("pic", pic["mimeType"], pic["size"], len(pic["data"]))
print("look viewport ->", h("look", {}))
for kind in ["blend","glb","gltf","fbx","obj","stl","usd"]:
    ext = {"blend":"blend","gltf":"gltf"}.get(kind, kind)
    p = os.path.join(tmp,"out","x."+ext)
    r = h("save", {"kind":kind,"path":p})
    print(kind, r["ok"], r.get("error"), os.path.exists(p))
print("save no path ->", h("save", {}))
print("unknown ->", h("fly", {}))
# the socket server, with the timer pump done by hand
cfgdir = os.path.dirname(cb.__file__)
key = "k"*32
srv = cb.Server(9871, key)
def ask(obj):
    s = socket.create_connection(("127.0.0.1", 9871)); s.sendall(json.dumps(obj).encode()+b"\n")
    data=b""
    while not data.endswith(b"\n"):
        p=s.recv(65536)
        if not p: break
        data+=p
    return json.loads(data)
res = {}
t = threading.Thread(target=lambda: res.update(a=ask({"key":key,"op":"info"}), b=ask({"key":"wrong","op":"info"})))
t.start()
for _ in range(100):
    cb.pump(); time.sleep(0.02)
    if not t.is_alive(): break
t.join(); srv.stop()
print("socket", res["a"]["ok"], res["a"]["result"]["objects"], res["b"])
print("ALL DONE")
