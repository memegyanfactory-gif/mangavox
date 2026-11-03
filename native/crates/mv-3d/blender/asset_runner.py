"""Builds one library asset headless:  blender -b --factory-startup --python-exit-code 1 -P asset_runner.py -- job.json

job.json: {"name", "category", "out_blend", "out_preview", "outline": bool,
           one of: "code_file" (AI-written script using the mv3d helpers, run sandboxed), "character" (build_character params),
                   "world_map" ({"locations": [...]}), "import_file" (.glb/.gltf/.fbx/.obj/.blend),
           "target_size": metres for the longest side of an imported model}
Prints one line `MV3D_RESULT {...}` (dims, objects) on success, `MV3D_ERROR {...}` on failure.
"""
import json
import math
import os
import sys
import traceback

import bpy
import mathutils

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mv3d  # noqa: E402


import sandbox  # noqa: E402


# ── replaying a sandboxed script ─────────────────────────────────────────────────────────
# The AI's script ran in sandbox.py against a recording API and never touched Blender. Here its
# recorded operations become objects through the real helpers; every value was checked when it
# was recorded and is used only as a number, a colour or a short name.

PRIMITIVES = {
    "box": ("size", "bevel"),
    "cyl": ("r", "depth", "verts"),
    "cone": ("r1", "r2", "depth", "verts"),
    "sphere": ("r", "scale", "segments"),
    "torus": ("r_major", "r_minor"),
    "plane": ("size",),
    "text": ("body", "size"),
}
CALLS = {"build_room": mv3d.build_room, "build_outdoor": mv3d.build_outdoor}


def replay(ops):
    objs, mats = {}, {}
    for op in ops:
        kind = op["op"]
        a = op.get("args", {})
        if kind == "mat":
            mats[op["id"]] = mv3d.mat(a["name"], a["color"], a["rough"], a["metal"], a["emit"])
        elif kind in PRIMITIVES:
            kw = {k: a[k] for k in PRIMITIVES[kind]}
            kw.update(name=a["name"], loc=a["loc"], color=a["color"], parent=objs.get(a.get("parent")))
            if "rot" in a and kind != "sphere":
                kw["rot"] = a["rot"]
            if kind != "text":
                kw["material"] = mats.get(a.get("material"))
            objs[op["id"]] = getattr(mv3d, kind)(**kw)
        elif kind == "empty":
            objs[op["id"]] = mv3d.empty(a["name"], a["loc"], objs.get(a.get("parent")))
        elif kind == "light":
            objs[op["id"]] = mv3d.light(a["name"], a["kind"], a["energy"], a["loc"], a["rot"], a["color"], a["size"])
        elif kind == "smooth" and op["id"] in objs:
            mv3d.smooth(objs[op["id"]])
        elif kind == "call" and op.get("fn") in CALLS:
            CALLS[op["fn"]](**a)
        elif kind == "set" and op["id"] in objs:
            o, v = objs[op["id"]], op["value"]
            if op["field"] == "location":
                o.location = v
            elif op["field"] == "rotation":
                o.rotation_euler = [math.radians(x) for x in v]
            elif op["field"] == "scale":
                o.scale = v
            elif op["field"] == "parent" and v in objs and objs[v] is not o:
                mv3d.set_parent(o, objs[v])


def run_sandboxed(code, name):
    replay(sandbox.record(code, name))


def job_path():
    argv = sys.argv
    return argv[argv.index("--") + 1] if "--" in argv else None


def reset():
    bpy.ops.wm.read_factory_settings(use_empty=True)
    col = bpy.data.collections.new("MV_ASSET")
    bpy.context.scene.collection.children.link(col)
    layer = bpy.context.view_layer.layer_collection.children["MV_ASSET"]
    bpy.context.view_layer.active_layer_collection = layer
    return col


def import_model(path):
    ext = os.path.splitext(path)[1].lower()
    if ext in (".glb", ".gltf"):
        bpy.ops.import_scene.gltf(filepath=path)
    elif ext == ".fbx":
        bpy.ops.import_scene.fbx(filepath=path)
    elif ext == ".obj":
        bpy.ops.wm.obj_import(filepath=path)
    elif ext == ".blend":
        with bpy.data.libraries.load(path, link=False) as (src, dst):
            dst.objects = list(src.objects)
        for o in dst.objects:
            if o is not None and o.type in ("MESH", "EMPTY", "CURVE"):
                bpy.context.collection.objects.link(o)
    else:
        raise RuntimeError("unsupported model format: " + ext)


def world_bbox(objs):
    lo = mathutils.Vector((1e9, 1e9, 1e9))
    hi = mathutils.Vector((-1e9, -1e9, -1e9))
    found = False
    deps = bpy.context.evaluated_depsgraph_get()
    for o in objs:
        if o.type not in ("MESH", "CURVE", "FONT"):
            continue
        ev = o.evaluated_get(deps)
        for c in ev.bound_box:
            w = ev.matrix_world @ mathutils.Vector(c)
            lo = mathutils.Vector(map(min, lo, w))
            hi = mathutils.Vector(map(max, hi, w))
            found = True
    if not found:
        return mathutils.Vector((0, 0, 0)), mathutils.Vector((0, 0, 0))
    return lo, hi


def gather(col):
    """Everything the script made goes into MV_ASSET under one root empty."""
    for o in list(bpy.context.scene.collection.objects):
        col.objects.link(o)
        bpy.context.scene.collection.objects.unlink(o)
    for c in list(bpy.data.collections):
        if c is col:
            continue
        for o in list(c.objects):
            if col not in o.users_collection:
                col.objects.link(o)
    objs = list(col.all_objects)
    root = next((o for o in objs if o.get("mv_joint") == "root"), None)
    if root is None:
        root = bpy.data.objects.new("MV_ROOT", None)
        col.objects.link(root)
        root["mv_joint"] = "root"
        bpy.context.view_layer.update()
        for o in objs:
            if o.parent is None and o is not root:
                mv3d.set_parent(o, root)
    return root, list(col.all_objects)


def preview(objs, out, category):
    lo, hi = world_bbox(objs)
    centre = (lo + hi) / 2
    size = max((hi - lo).length, 0.5)
    scene = bpy.context.scene
    cam_data = bpy.data.cameras.new("preview_cam")
    cam_data.lens = 50
    cam = bpy.data.objects.new("preview_cam", cam_data)
    scene.collection.objects.link(cam)
    view = mathutils.Vector((0.55, -1.0, 0.45)) if category != "maps" else mathutils.Vector((0.0, -0.7, 1.0))
    if category == "environments":
        view = mathutils.Vector((0.25, -1.0, 0.35))
    cam.location = centre + view.normalized() * size * 1.45
    cam.rotation_mode = "QUATERNION"
    cam.rotation_quaternion = (centre - cam.location).to_track_quat("-Z", "Y")
    cam_data.clip_end = size * 20
    scene.camera = cam
    sun_data = bpy.data.lights.new("preview_sun", "SUN")
    sun_data.energy = 3.0
    sun = bpy.data.objects.new("preview_sun", sun_data)
    sun.rotation_euler = (math.radians(50), 0, math.radians(30))
    scene.collection.objects.link(sun)
    world = bpy.data.worlds.new("preview_world")
    world.use_nodes = True
    bg = world.node_tree.nodes.get("Background")
    if bg:
        bg.inputs[0].default_value = (0.32, 0.34, 0.38, 1)
        bg.inputs[1].default_value = 0.8
    scene.world = world
    mv3d.set_engine(scene, "eevee")
    try:
        scene.eevee.taa_render_samples = 16
    except AttributeError:
        pass
    scene.view_settings.view_transform = "Standard"
    scene.render.resolution_x = 512
    scene.render.resolution_y = 512
    scene.render.image_settings.file_format = "PNG"
    scene.render.filepath = out
    bpy.ops.render.render(write_still=True)
    for o in (cam, sun):
        bpy.data.objects.remove(o)


def main():
    job = json.load(open(job_path(), encoding="utf-8"))
    col = reset()
    category = job.get("category", "props")
    if job.get("character") is not None:
        mv3d.build_character(job["character"])
    elif job.get("world_map") is not None:
        mv3d.build_world_map(job["world_map"].get("locations", []))
    elif job.get("import_file"):
        import_model(job["import_file"])
    elif job.get("code_file"):
        code = open(job["code_file"], encoding="utf-8").read()
        run_sandboxed(code, job["code_file"])
    else:
        raise RuntimeError("job has nothing to build")

    root, objs = gather(col)
    meshes = [o for o in objs if o.type == "MESH"]
    if not meshes and category != "maps":
        raise RuntimeError("the script made no mesh objects")
    lo, hi = world_bbox(objs)
    if job.get("import_file"):
        # Imported models come in any unit and offset: stand them on the ground, centred,
        # with the longest side at the requested size.
        dims = hi - lo
        longest = max(dims.x, dims.y, dims.z, 1e-6)
        target = float(job.get("target_size") or longest)
        s = target / longest
        root.scale = (s, s, s)
        root.location = (-(lo.x + hi.x) / 2 * s, -(lo.y + hi.y) / 2 * s, -lo.z * s)
        bpy.context.view_layer.update()
        lo, hi = world_bbox(objs)
    if job.get("outline") and category in ("characters", "props", "vehicles"):
        mv3d.toon_outline(meshes)
    dims = hi - lo
    root["mv_category"] = category
    root["mv_name"] = job.get("name", "")
    root["mv_dims"] = [dims.x, dims.y, dims.z]
    mv3d.sanitize()
    if job.get("out_preview"):
        preview(objs, job["out_preview"], category)
    mv3d.sanitize()
    os.makedirs(os.path.dirname(job["out_blend"]), exist_ok=True)
    bpy.ops.wm.save_as_mainfile(filepath=job["out_blend"], compress=True)
    print("MV3D_RESULT " + json.dumps({"dims": [round(dims.x, 3), round(dims.y, 3), round(dims.z, 3)], "min": [round(v, 3) for v in lo], "objects": len(objs), "meshes": len(meshes)}))


try:
    main()
except Exception as e:  # noqa: BLE001 - every failure is reported back to the app
    print("MV3D_ERROR " + json.dumps({"error": str(e), "trace": traceback.format_exc()[-3000:]}))
    sys.exit(1)
