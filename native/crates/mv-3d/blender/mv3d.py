"""MangaVox 3D helpers for Blender (bpy). Imported by asset_runner.py, assemble.py and by the
AI-written asset scripts (`from mv3d import *`).

Conventions every asset follows: metres, ground at z=0, centred on x/y = 0, the front faces -Y.
Characters are built from joint empties (custom property `mv_joint`) so assemble.py can animate
them without an armature: rotating `shoulder.L` swings the left arm, `hip.R` the right leg.
"""
import math

import bpy
import mathutils

_MATS = {}


def rgba(c, alpha=1.0):
    """'#rrggbb', 'rrggbb', (r, g, b) in 0..1 or 0..255 -> (r, g, b, a) linear-ish."""
    if isinstance(c, str):
        s = c.strip().lstrip("#")
        if len(s) == 3:
            s = "".join(ch * 2 for ch in s)
        try:
            r, g, b = (int(s[i:i + 2], 16) / 255.0 for i in (0, 2, 4))
        except ValueError:
            r, g, b = 0.6, 0.6, 0.6
    elif c is None:
        r, g, b = 0.6, 0.6, 0.6
    else:
        v = list(c)[:3]
        if any(x > 1.0 for x in v):
            v = [x / 255.0 for x in v]
        r, g, b = v
    # sRGB -> linear so colours picked from a manga panel look the same in the render.
    lin = lambda x: x / 12.92 if x <= 0.04045 else ((x + 0.055) / 1.055) ** 2.4
    return (lin(r), lin(g), lin(b), alpha)


def mat(name, color="#999999", rough=0.6, metal=0.0, emit=0.0):
    """A cached Principled material; `emit` > 0 makes it glow in its own colour."""
    key = (name, str(color), rough, metal, emit)
    if key in _MATS and _MATS[key].name in bpy.data.materials:
        return _MATS[key]
    m = bpy.data.materials.new(name)
    m.use_nodes = True
    bsdf = m.node_tree.nodes.get("Principled BSDF")
    col = rgba(color)
    m.diffuse_color = col
    if bsdf:
        bsdf.inputs["Base Color"].default_value = col
        bsdf.inputs["Roughness"].default_value = rough
        bsdf.inputs["Metallic"].default_value = metal
        if emit > 0:
            for k in ("Emission Color", "Emission"):
                if k in bsdf.inputs:
                    bsdf.inputs[k].default_value = col
                    break
            if "Emission Strength" in bsdf.inputs:
                bsdf.inputs["Emission Strength"].default_value = emit
    _MATS[key] = m
    return m


def _finish(obj, name, color, material, parent, smooth_shade=False):
    obj.name = name
    if material is None:
        material = mat(name + "_mat", color)
    if material is not None and obj.data is not None and hasattr(obj.data, "materials"):
        obj.data.materials.clear()
        obj.data.materials.append(material)
    if parent is not None:
        set_parent(obj, parent)
    if smooth_shade:
        smooth(obj)
    return obj


def set_parent(obj, parent):
    """Parent keeping the world transform."""
    mw = obj.matrix_world.copy()
    obj.parent = parent
    obj.matrix_parent_inverse = parent.matrix_world.inverted()
    obj.matrix_world = mw


def smooth(obj):
    if obj.type == "MESH":
        for p in obj.data.polygons:
            p.use_smooth = True
    return obj


def box(name, size=(1, 1, 1), loc=(0, 0, 0), rot=(0, 0, 0), color="#999999", material=None, parent=None, bevel=0.0):
    """Box of `size` (x, y, z) centred on `loc`; `rot` in degrees."""
    bpy.ops.mesh.primitive_cube_add(size=1, location=loc, rotation=[math.radians(a) for a in rot])
    o = bpy.context.active_object
    o.scale = size
    bpy.ops.object.transform_apply(location=False, rotation=False, scale=True)
    if bevel > 0:
        m = o.modifiers.new("bevel", "BEVEL")
        m.width = bevel
        m.segments = 2
    return _finish(o, name, color, material, parent)


def cyl(name, r=0.5, depth=1.0, loc=(0, 0, 0), rot=(0, 0, 0), color="#999999", material=None, parent=None, verts=24):
    """Cylinder along local Z centred on `loc`."""
    bpy.ops.mesh.primitive_cylinder_add(vertices=verts, radius=r, depth=depth, location=loc, rotation=[math.radians(a) for a in rot])
    return _finish(bpy.context.active_object, name, color, material, parent, smooth_shade=True)


def cone(name, r1=0.5, r2=0.0, depth=1.0, loc=(0, 0, 0), rot=(0, 0, 0), color="#999999", material=None, parent=None, verts=24):
    bpy.ops.mesh.primitive_cone_add(vertices=verts, radius1=r1, radius2=r2, depth=depth, location=loc, rotation=[math.radians(a) for a in rot])
    return _finish(bpy.context.active_object, name, color, material, parent, smooth_shade=True)


def sphere(name, r=0.5, loc=(0, 0, 0), scale=(1, 1, 1), color="#999999", material=None, parent=None, segments=24):
    bpy.ops.mesh.primitive_uv_sphere_add(segments=segments, ring_count=max(8, segments // 2), radius=r, location=loc)
    o = bpy.context.active_object
    o.scale = scale
    bpy.ops.object.transform_apply(location=False, rotation=False, scale=True)
    return _finish(o, name, color, material, parent, smooth_shade=True)


def torus(name, r_major=0.5, r_minor=0.1, loc=(0, 0, 0), rot=(0, 0, 0), color="#999999", material=None, parent=None):
    bpy.ops.mesh.primitive_torus_add(major_radius=r_major, minor_radius=r_minor, location=loc, rotation=[math.radians(a) for a in rot])
    return _finish(bpy.context.active_object, name, color, material, parent, smooth_shade=True)


def plane(name, size=(10, 10), loc=(0, 0, 0), rot=(0, 0, 0), color="#777777", material=None, parent=None):
    bpy.ops.mesh.primitive_plane_add(size=1, location=loc, rotation=[math.radians(a) for a in rot])
    o = bpy.context.active_object
    o.scale = (size[0], size[1], 1)
    bpy.ops.object.transform_apply(location=False, rotation=False, scale=True)
    return _finish(o, name, color, material, parent)


def empty(name, loc=(0, 0, 0), parent=None, joint=None):
    o = bpy.data.objects.new(name, None)
    o.empty_display_size = 0.1
    o.location = loc
    bpy.context.collection.objects.link(o)
    bpy.context.view_layer.update()
    if parent is not None:
        set_parent(o, parent)
    if joint:
        o["mv_joint"] = joint
    bpy.context.view_layer.update()
    return o


def text(name, body, loc=(0, 0, 0), size=0.5, color="#ffffff", rot=(90, 0, 0), parent=None):
    bpy.ops.object.text_add(location=loc, rotation=[math.radians(a) for a in rot])
    o = bpy.context.active_object
    o.data.body = str(body)
    o.data.size = size
    o.data.align_x = "CENTER"
    return _finish(o, name, color, None, parent)


def light(name, kind="POINT", energy=500.0, loc=(0, 0, 3), rot=(0, 0, 0), color="#ffffff", size=1.0):
    """A lamp: kind POINT | SUN | SPOT | AREA; energy in watts (SUN: strength ~1-5); rot in degrees."""
    kind = str(kind).upper()
    if kind not in ("POINT", "SUN", "SPOT", "AREA"):
        kind = "POINT"
    d = bpy.data.lights.new(name, kind)
    d.energy = float(energy)
    d.color = rgba(color)[:3]
    if kind == "AREA":
        d.size = float(size)
    elif kind != "SUN":
        d.shadow_soft_size = float(size) * 0.25
    o = bpy.data.objects.new(name, d)
    o.location = loc
    o.rotation_euler = [math.radians(a) for a in rot]
    bpy.context.collection.objects.link(o)
    return o


def sanitize():
    """Removes everything in the file that could run code or write files: drivers (their
    expressions are Python), text blocks (scripts), the compositor and the sequencer. Run before
    every render and save, so no .blend this app writes or renders carries any of them, even
    one built from a downloaded model."""
    for coll in (bpy.data.objects, bpy.data.meshes, bpy.data.materials, bpy.data.lights, bpy.data.cameras,
                 bpy.data.worlds, bpy.data.node_groups, bpy.data.scenes, bpy.data.curves, bpy.data.textures,
                 bpy.data.shape_keys, bpy.data.armatures):
        for idb in coll:
            for holder in (idb, getattr(idb, "node_tree", None)):
                ad = getattr(holder, "animation_data", None) if holder is not None else None
                if ad is not None:
                    for d in list(ad.drivers):
                        ad.drivers.remove(d)
    for t in list(bpy.data.texts):
        bpy.data.texts.remove(t)
    for sc in bpy.data.scenes:
        r = sc.render
        for attr, val in (("use_compositing", False), ("use_sequencer", False), ("use_file_extension", True)):
            try:
                setattr(r, attr, val)
            except (AttributeError, TypeError):
                pass
        try:
            sc.use_nodes = False
        except (AttributeError, TypeError):
            pass
        if getattr(sc, "compositing_node_group", None) is not None:
            try:
                sc.compositing_node_group = None
            except (AttributeError, TypeError):
                pass
        if sc.sequence_editor is not None:
            sc.sequence_editor_clear()


def set_engine(scene, kind):
    """EEVEE is BLENDER_EEVEE_NEXT on Blender 4.2-4.5 and BLENDER_EEVEE again from 5.0.
    Workbench falls back to EEVEE where it is missing."""
    names = ("BLENDER_WORKBENCH",) if kind == "workbench" else ("BLENDER_EEVEE", "BLENDER_EEVEE_NEXT")
    for n in names:
        try:
            scene.render.engine = n
            return n
        except TypeError:
            continue
    if kind == "workbench":
        return set_engine(scene, "eevee")
    raise RuntimeError("this Blender has no EEVEE renderer")


def scatter(fn, count, area=(10, 10), seed=1, z=0.0, avoid_center=0.0):
    """Calls fn(i, (x, y, z), rng) `count` times at random spots in `area`, keeping
    `avoid_center` metres around the origin free (the actors' stage)."""
    import random
    rng = random.Random(seed)
    placed = 0
    tries = 0
    while placed < count and tries < count * 20:
        tries += 1
        x = rng.uniform(-area[0] / 2, area[0] / 2)
        y = rng.uniform(-area[1] / 2, area[1] / 2)
        if math.hypot(x, y) < avoid_center:
            continue
        fn(placed, (x, y, z), rng)
        placed += 1


# ── characters ──────────────────────────────────────────────────────────────────────────────

BUILDS = {"slim": 0.88, "average": 1.0, "athletic": 1.08, "muscular": 1.2, "heavy": 1.3, "child": 0.95}


def build_character(p):
    """A stylized, animatable figure from parameters (all optional):
    height (m), build, skin, hair_color, hair_style (short|spiky|long|ponytail|swept|bald|bun),
    eye_color, top_kind (tshirt|tank|shirt|jacket|robe|shirtless|armor|dress), top_color,
    bottom_kind (pants|shorts|skirt), bottom_color, shoe_color, belt_color,
    accessories ([sword|headband|glasses|cape|scarf|bag|hat|mask|gloves|earrings]),
    accent_color. Returns the root empty."""
    h = float(p.get("height") or 1.75)
    k = BUILDS.get(str(p.get("build", "average")).lower(), 1.0)
    skin = p.get("skin") or "#f1c9a5"
    top_kind = str(p.get("top_kind") or "tshirt").lower()
    top = p.get("top_color") or "#3b4a6b"
    bottom_kind = str(p.get("bottom_kind") or "pants").lower()
    bottom = p.get("bottom_color") or "#2a2a2a"
    shoes = p.get("shoe_color") or "#1b1b1b"
    hair = p.get("hair_color") or "#2b1d14"
    eyes = p.get("eye_color") or "#3a2a1a"
    accent = p.get("accent_color") or "#b02020"
    acc = [str(a).lower() for a in (p.get("accessories") or [])]

    m_skin, m_top, m_bot = mat("skin", skin, 0.55), mat("top", top, 0.7), mat("bottom", bottom, 0.8)
    m_shoe, m_hair, m_eye = mat("shoes", shoes, 0.5), mat("hair", hair, 0.45), mat("eye", eyes, 0.2)
    m_white, m_acc = mat("eye_white", "#f4f4f4", 0.3), mat("accent", accent, 0.5)
    bare_torso = top_kind == "shirtless"
    bare_arms = top_kind in ("tank", "shirtless", "dress")

    root = empty("root", (0, 0, 0), joint="root")
    hip_z = 0.5 * h
    pelvis = empty("pelvis", (0, 0, hip_z), root, joint="pelvis")
    sw = 0.13 * h * k  # half shoulder width
    # torso: waist up to the shoulders
    torso_h = 0.3 * h
    tz = hip_z + torso_h / 2 + 0.02 * h
    t = cyl("torso", r=0.1 * h * k, depth=torso_h, loc=(0, 0, tz), material=m_skin if bare_torso else m_top, parent=pelvis)
    t.scale = (1.0, 0.62, 1.0)
    chest = sphere("chest", r=0.105 * h * k, loc=(0, 0, tz + torso_h * 0.28), scale=(1.15, 0.68, 0.7), material=m_skin if bare_torso else m_top, parent=pelvis)
    if bare_torso and k >= 1.1:
        for i, x in enumerate((-0.045, 0.045)):
            sphere("pec_%d" % i, r=0.05 * h, loc=(x * h / 1.75, -0.055 * h * k, tz + torso_h * 0.3), scale=(1.1, 0.5, 0.8), material=m_skin, parent=pelvis)
    hips = sphere("hips", r=0.095 * h * k, loc=(0, 0, hip_z), scale=(1.05, 0.7, 0.55), material=m_bot, parent=pelvis)
    if p.get("belt_color") or top_kind == "robe":
        torus("belt", r_major=0.095 * h * k, r_minor=0.012 * h, loc=(0, 0, hip_z + 0.04 * h), material=mat("belt", p.get("belt_color") or accent), parent=pelvis).scale = (1.0, 0.66, 1.0)

    # head
    neck_z = hip_z + torso_h + 0.03 * h
    cyl("neck", r=0.035 * h * k, depth=0.06 * h, loc=(0, 0, neck_z), material=m_skin, parent=pelvis)
    head = empty("head", (0, 0, neck_z + 0.03 * h), pelvis, joint="head")
    hr = 0.078 * h
    hc = neck_z + 0.03 * h + hr
    sphere("skull", r=hr, loc=(0, 0, hc), scale=(0.9, 0.95, 1.08), material=m_skin, parent=head)
    for i, x in enumerate((-0.026 * h, 0.026 * h)):
        sphere("eye_white_%d" % i, r=0.016 * h, loc=(x, -hr * 0.82, hc + 0.004 * h), scale=(1.0, 0.4, 0.75), material=m_white, parent=head)
        sphere("iris_%d" % i, r=0.01 * h, loc=(x, -hr * 0.9, hc + 0.003 * h), scale=(1.0, 0.4, 1.0), material=m_eye, parent=head)
        box("brow_%d" % i, (0.03 * h, 0.006 * h, 0.006 * h), (x, -hr * 0.86, hc + 0.024 * h), rot=(0, (-8 if i == 0 else 8), 0), material=m_hair, parent=head)
    box("mouth", (0.024 * h, 0.004 * h, 0.004 * h), (0, -hr * 0.9, hc - 0.032 * h), material=mat("mouth", "#7a3b32"), parent=head)
    sphere("nose", r=0.007 * h, loc=(0, -hr * 0.96, hc - 0.01 * h), material=m_skin, parent=head)
    _hair(str(p.get("hair_style") or "short").lower(), head, hc, hr, h, m_hair)

    # arms: shoulder empties at the top of the torso, upper arm + elbow + forearm + hand
    sh_z = hip_z + torso_h - 0.01 * h
    arm_r = 0.026 * h * k
    up_len, fo_len = 0.17 * h, 0.16 * h
    sleeve = m_skin if bare_arms else m_top
    fore = m_top if top_kind in ("jacket", "robe", "armor", "shirt") else m_skin
    for side, sx in (("L", 1), ("R", -1)):
        sh = empty("shoulder." + side, (sx * sw, 0, sh_z), pelvis, joint="shoulder." + side)
        sphere("deltoid." + side, r=arm_r * 1.25, loc=(sx * sw, 0, sh_z), material=sleeve, parent=sh)
        cyl("upper_arm." + side, r=arm_r, depth=up_len, loc=(sx * sw, 0, sh_z - up_len / 2), material=sleeve, parent=sh)
        el = empty("elbow." + side, (sx * sw, 0, sh_z - up_len), sh, joint="elbow." + side)
        cyl("forearm." + side, r=arm_r * 0.85, depth=fo_len, loc=(sx * sw, 0, sh_z - up_len - fo_len / 2), material=fore, parent=el)
        sphere("hand." + side, r=arm_r * 1.05, loc=(sx * sw, 0, sh_z - up_len - fo_len - arm_r * 0.6), scale=(0.8, 0.9, 1.1), material=m_acc if "gloves" in acc else m_skin, parent=el)

    # legs
    th_len, sh_len = 0.24 * h, 0.23 * h
    leg_r = 0.045 * h * k
    shorts = bottom_kind == "shorts"
    for side, sx in (("L", 1), ("R", -1)):
        hx = sx * 0.055 * h * k
        hp = empty("hip." + side, (hx, 0, hip_z), pelvis, joint="hip." + side)
        cyl("thigh." + side, r=leg_r, depth=th_len, loc=(hx, 0, hip_z - th_len / 2), material=m_bot, parent=hp)
        kn = empty("knee." + side, (hx, 0, hip_z - th_len), hp, joint="knee." + side)
        cyl("shin." + side, r=leg_r * 0.8, depth=sh_len, loc=(hx, 0, hip_z - th_len - sh_len / 2), material=m_skin if shorts else m_bot, parent=kn)
        box("foot." + side, (leg_r * 1.6, leg_r * 3.0, 0.035 * h), (hx, -leg_r * 0.6, 0.0175 * h), material=m_shoe, parent=kn, bevel=0.01)

    if top_kind in ("robe", "dress") or bottom_kind == "skirt":
        length = 0.32 * h if top_kind == "robe" else 0.2 * h
        cone("skirt", r1=0.15 * h * k, r2=0.1 * h * k, depth=length, loc=(0, 0, hip_z - length / 2 + 0.03 * h), material=m_top if top_kind != "pants" and bottom_kind != "skirt" else m_bot, parent=pelvis).scale = (1.0, 0.8, 1.0)
    if top_kind == "jacket":
        box("jacket_open", (0.02 * h, 0.01 * h, torso_h * 0.9), (0, -0.065 * h * k, tz), material=mat("shirt_under", "#e8e8e8"), parent=pelvis)
    if top_kind == "armor":
        sphere("pauldron.L", r=arm_r * 1.8, loc=(sw, 0, sh_z + 0.01 * h), scale=(1, 1, 0.6), material=mat("armor", accent, 0.3, 0.8), parent=pelvis)
        sphere("pauldron.R", r=arm_r * 1.8, loc=(-sw, 0, sh_z + 0.01 * h), scale=(1, 1, 0.6), material=mat("armor", accent, 0.3, 0.8), parent=pelvis)

    for a in acc:
        if a == "headband":
            torus("headband", r_major=hr * 0.95, r_minor=0.008 * h, loc=(0, 0, hc + hr * 0.35), material=m_acc, parent=head)
        elif a == "glasses":
            for i, x in enumerate((-0.026 * h, 0.026 * h)):
                torus("lens_%d" % i, r_major=0.016 * h, r_minor=0.002 * h, loc=(x, -hr * 0.95, hc + 0.004 * h), rot=(90, 0, 0), material=mat("frame", "#111111"), parent=head)
        elif a == "sword":
            s = box("sword_blade", (0.012 * h, 0.004 * h, 0.5 * h), (0, 0.08 * h * k, tz + 0.05 * h), rot=(0, 35, 0), material=mat("steel", "#c8ccd4", 0.25, 0.9), parent=pelvis)
            box("sword_guard", (0.06 * h, 0.012 * h, 0.012 * h), (0.11 * h, 0.08 * h * k, tz + 0.2 * h), rot=(0, 35, 0), material=m_acc, parent=pelvis)
        elif a == "cape":
            c = box("cape", (sw * 2.1, 0.01 * h, 0.45 * h), (0, 0.075 * h * k, sh_z - 0.22 * h), rot=(-6, 0, 0), material=m_acc, parent=pelvis)
        elif a == "scarf":
            torus("scarf", r_major=0.05 * h * k, r_minor=0.018 * h, loc=(0, 0, neck_z), material=m_acc, parent=pelvis)
        elif a == "bag":
            box("bag", (0.12 * h, 0.06 * h, 0.14 * h), (0, 0.09 * h * k, tz), material=mat("bag", "#5a4030"), parent=pelvis, bevel=0.01)
        elif a == "hat":
            cyl("hat_brim", r=hr * 1.6, depth=0.008 * h, loc=(0, 0, hc + hr * 0.7), material=m_acc, parent=head)
            cyl("hat_top", r=hr * 0.95, depth=0.06 * h, loc=(0, 0, hc + hr * 0.95), material=m_acc, parent=head)
        elif a == "mask":
            sphere("mask", r=hr * 0.8, loc=(0, -hr * 0.35, hc - 0.02 * h), scale=(1.0, 0.6, 0.55), material=m_acc, parent=head)
        elif a == "earrings":
            for i, x in enumerate((-hr * 0.9, hr * 0.9)):
                sphere("earring_%d" % i, r=0.006 * h, loc=(x, 0, hc - 0.02 * h), material=mat("gold", "#d4af37", 0.2, 1.0), parent=head)
    return root


def _hair(style, head, hc, hr, h, m):
    if style == "bald":
        return
    cap = sphere("hair_cap", r=hr * 1.07, loc=(0, hr * 0.06, hc + hr * 0.12), scale=(0.95, 1.0, 0.9), material=m, parent=head)
    if style == "spiky":
        import random
        rng = random.Random(7)
        for i in range(14):
            a = (i / 14.0) * math.tau
            el = rng.uniform(0.15, 0.9)
            d = mathutils.Vector((math.cos(a) * math.cos(el), math.sin(a) * math.cos(el) + 0.15, math.sin(el))).normalized()
            base = mathutils.Vector((0, 0, hc)) + d * hr * 0.9
            o = cone("spike_%d" % i, r1=hr * 0.3, r2=0, depth=hr * 0.9, loc=base + d * hr * 0.35, material=m, parent=head)
            o.rotation_mode = "QUATERNION"
            o.rotation_quaternion = d.to_track_quat("Z", "Y")
    elif style in ("long", "ponytail", "bun"):
        if style == "long":
            box("hair_back", (hr * 1.9, hr * 0.5, hr * 2.6), (0, hr * 0.55, hc - hr * 0.9), material=m, parent=head, bevel=0.02)
        elif style == "ponytail":
            cyl("ponytail", r=hr * 0.25, depth=hr * 2.0, loc=(0, hr * 1.15, hc - hr * 0.5), rot=(-15, 0, 0), material=m, parent=head)
        else:
            sphere("bun", r=hr * 0.4, loc=(0, hr * 0.8, hc + hr * 0.6), material=m, parent=head)
    elif style == "swept":
        sphere("hair_swept", r=hr * 0.9, loc=(0, hr * 0.35, hc + hr * 0.45), scale=(1.0, 1.25, 0.55), material=m, parent=head)
    else:  # short
        box("fringe", (hr * 1.6, hr * 0.3, hr * 0.35), (0, -hr * 0.72, hc + hr * 0.62), rot=(-20, 0, 0), material=m, parent=head, bevel=0.02)


# ── maps and fallback environments ─────────────────────────────────────────────────────────

def build_world_map(locations, size=60.0, seed=3, land="#6f8f4e", water="#2f5f8a"):
    """A table-top world map: rolling terrain, a sea, and a labelled pin for each location
    ({"name": str, "pos": [x, y] in -1..1, "kind": str})."""
    bpy.ops.mesh.primitive_grid_add(x_subdivisions=120, y_subdivisions=120, size=size, location=(0, 0, 0))
    g = bpy.context.active_object
    g.name = "terrain"
    tex = bpy.data.textures.new("terrain_noise", "CLOUDS")
    tex.noise_scale = size * 0.22
    disp = g.modifiers.new("relief", "DISPLACE")
    disp.texture = tex
    disp.strength = size * 0.06
    disp.mid_level = 0.45
    g.data.materials.append(mat("land", land, 0.9))
    smooth(g)
    plane("sea", (size * 1.05, size * 1.05), (0, 0, size * 0.004), material=mat("sea", water, 0.15))
    for i, loc in enumerate(locations or []):
        pos = loc.get("pos") or [math.cos(i * 2.1) * 0.6, math.sin(i * 2.1) * 0.6]
        x, y = float(pos[0]) * size * 0.42, float(pos[1]) * size * 0.42
        top = size * 0.08
        cyl("pin_%d" % i, r=size * 0.004, depth=top, loc=(x, y, top / 2), material=mat("pin", "#222222"))
        sphere("pin_head_%d" % i, r=size * 0.012, loc=(x, y, top), material=mat("pin_head_%d" % i, "#d23c3c", 0.4, 0.0, 0.5))
        text("label_%d" % i, loc.get("name", "?"), loc=(x, y - size * 0.01, top + size * 0.025), size=size * 0.022, color="#ffffff")


def build_room(width=14.0, depth=12.0, height=4.0, floor="#8a7560", wall="#c9c2b6", trim="#4a3a2a", open_front=True):
    """Fallback interior: floor, back and side walls (front left open for the camera)."""
    plane("floor", (width, depth), (0, 0, 0), material=mat("floor", floor, 0.8))
    box("wall_back", (width, 0.2, height), (0, depth / 2, height / 2), material=mat("wall", wall, 0.9))
    box("wall_left", (0.2, depth, height), (-width / 2, 0, height / 2), material=mat("wall", wall, 0.9))
    box("wall_right", (0.2, depth, height), (width / 2, 0, height / 2), material=mat("wall", wall, 0.9))
    box("skirting", (width, 0.25, 0.15), (0, depth / 2 - 0.1, 0.075), material=mat("trim", trim))
    if not open_front:
        box("wall_front", (width, 0.2, height), (0, -depth / 2, height / 2), material=mat("wall", wall, 0.9))


def build_outdoor(size=40.0, ground="#6d7a4a", features=12, seed=2, feature_color="#5a4a3a"):
    """Fallback exterior: a large ground and scattered rocks / trees out of the stage area."""
    plane("ground", (size, size), (0, 0, 0), material=mat("ground", ground, 0.95))

    def put(i, at, rng):
        if rng.random() < 0.5:
            cyl("trunk_%d" % i, r=0.18, depth=2.2, loc=(at[0], at[1], 1.1), material=mat("bark", "#4b3621"))
            cone("crown_%d" % i, r1=1.3, r2=0, depth=3.2, loc=(at[0], at[1], 3.4), material=mat("leaves", "#2f5a2a"))
        else:
            sphere("rock_%d" % i, r=rng.uniform(0.4, 1.2), loc=at, scale=(1.3, 1.0, 0.7), material=mat("rock", feature_color, 0.9))

    scatter(put, features, (size * 0.9, size * 0.9), seed, 0.0, avoid_center=6.0)


# ── outline (anime look) ───────────────────────────────────────────────────────────────────

def toon_outline(objs, thickness=0.012):
    """Inverted-hull outline: a flipped solidify shell with a black back-face-culled material."""
    line = bpy.data.materials.get("mv_outline")
    if line is None:
        line = bpy.data.materials.new("mv_outline")
        line.use_nodes = True
        b = line.node_tree.nodes.get("Principled BSDF")
        if b:
            b.inputs["Base Color"].default_value = (0, 0, 0, 1)
            b.inputs["Roughness"].default_value = 1.0
        line.diffuse_color = (0, 0, 0, 1)
        line.use_backface_culling = True
    for o in objs:
        if o.type != "MESH" or len(o.data.vertices) < 8 or "mv_outline" in o.modifiers:
            continue
        o.data.materials.append(line)
        s = o.modifiers.new("mv_outline", "SOLIDIFY")
        s.thickness = thickness
        s.offset = 1.0
        s.use_flip_normals = True
        s.use_rim = False
        s.material_offset = len(o.data.materials) - 1
