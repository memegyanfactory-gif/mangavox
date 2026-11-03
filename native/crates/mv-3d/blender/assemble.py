"""Assembles, animates and renders one scene:  blender -b --factory-startup --python-exit-code 1 -P assemble.py -- scene.json

scene.json (written by mv-3d, frames already converted from seconds):
  fps, width, height, samples, mode ("previz" | "final" | "both"), time_of_day, mood,
  sky {"hdri": path | null, "color": "#rrggbb"}, environment {"blend": path} | null,
  actors [{id, blend, kind, pos [x, y], facing (deg | [x, y] | actor id), scale}],
  actions [{actor, type, frames [a, b], target ([x, y] | actor id | null)}],
  shots [{id, frames [a, b], framing, subject, secondary, angle, move}],
  previz_dir, frames_dir, blend_out, previz_width, previz_height
Prints `MV3D_RESULT {...}` on success, `MV3D_ERROR {...}` on failure.
"""
import json
import math
import os
import random
import sys
import traceback

import bpy
import mathutils
from mathutils import Vector

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mv3d  # noqa: E402

V = Vector


def job_path():
    return sys.argv[sys.argv.index("--") + 1]


# ── loading ─────────────────────────────────────────────────────────────────────────────────

class Actor:
    def __init__(self, aid, kind, col):
        self.id = aid
        self.kind = kind
        self.col = col
        objs = list(col.all_objects)
        self.root = next((o for o in objs if o.get("mv_joint") == "root" and o.parent is None), None)
        if self.root is None:
            self.root = next(o for o in objs if o.parent is None)
        self.joints = {o["mv_joint"]: o for o in objs if o.get("mv_joint")}
        dims = self.root.get("mv_dims")
        self.height = float(dims[2]) if dims else 1.7
        self.rest = {n: o.rotation_euler.copy() for n, o in self.joints.items()}
        self.rest_loc = {n: o.location.copy() for n, o in self.joints.items()}

    def j(self, name):
        return self.joints.get(name)


def append(path, name):
    with bpy.data.libraries.load(path, link=False) as (src, dst):
        dst.collections = [c for c in src.collections if c == "MV_ASSET"] or list(src.collections)[:1]
    col = dst.collections[0]
    col.name = name
    bpy.context.scene.collection.children.link(col)
    return col


def heading_to(d):
    """rot_z that turns a -Y-facing asset toward direction d."""
    return math.atan2(d.x, -d.y)


def unwrap(prev, new):
    while new - prev > math.pi:
        new -= math.tau
    while new - prev < -math.pi:
        new += math.tau
    return new


# ── keyframing helpers ──────────────────────────────────────────────────────────────────────

def key_loc(o, f, loc=None):
    if loc is not None:
        o.location = loc
    o.keyframe_insert("location", frame=f)


def key_rot(o, f, rot=None):
    if rot is not None:
        o.rotation_euler = rot
    o.keyframe_insert("rotation_euler", frame=f)


def joint_rot(a, name, f, x=0.0, y=0.0, z=0.0):
    o = a.j(name)
    if o is None:
        return
    r = a.rest[name]
    key_rot(o, f, mathutils.Euler((r.x + math.radians(x), r.y + math.radians(y), r.z + math.radians(z))))


def joint_rest(a, names, f):
    for n in names:
        joint_rot(a, n, f)


LIMBS = ["shoulder.L", "shoulder.R", "elbow.L", "elbow.R", "hip.L", "hip.R", "knee.L", "knee.R", "head"]


class Animator:
    def __init__(self, actors, fps):
        self.actors = actors
        self.fps = fps
        self.pos = {}
        self.head = {}

    def place(self, a, pos, facing, scale):
        a.root.location = V((pos[0], pos[1], 0.0))
        a.root.scale = (scale, scale, scale)
        self.pos[a.id] = V((pos[0], pos[1], 0.0))
        self.head[a.id] = 0.0
        a.root.rotation_euler = (0, 0, 0)

    def face_initial(self, a, facing):
        h = self.resolve_heading(a, facing)
        if h is not None:
            self.head[a.id] = h
            a.root.rotation_euler = (0, 0, h)

    def resolve_point(self, target):
        if isinstance(target, str) and target in self.pos:
            return self.pos[target]
        if isinstance(target, (list, tuple)) and len(target) >= 2:
            return V((float(target[0]), float(target[1]), 0.0))
        return None

    def resolve_heading(self, a, facing):
        if isinstance(facing, (int, float)):
            return math.radians(float(facing))
        p = self.resolve_point(facing)
        if p is None:
            return None
        d = p - self.pos[a.id]
        if d.length < 1e-3:
            return None
        return heading_to(d)

    def key_root(self, a, f):
        key_loc(a.root, f, self.pos[a.id].copy())
        key_rot(a.root, f, mathutils.Euler((a.root.rotation_euler.x, a.root.rotation_euler.y, self.head[a.id])))

    def turn(self, a, f0, f1, target):
        self.key_root(a, f0)
        h = self.resolve_heading(a, target)
        if h is not None:
            self.head[a.id] = unwrap(self.head[a.id], h)
        self.key_root(a, max(f0 + 1, min(f1, f0 + 8)))

    def move(self, a, f0, f1, target, run=False):
        p = self.resolve_point(target)
        if p is None:
            return
        if isinstance(target, str) and target in self.pos:
            # Stop in front of another actor, not inside them.
            d = p - self.pos[a.id]
            if d.length > 1.0:
                p = p - d.normalized() * 0.9
            else:
                p = self.pos[a.id]
        d = p - self.pos[a.id]
        self.key_root(a, f0)
        if d.length > 0.05:
            self.head[a.id] = unwrap(self.head[a.id], heading_to(d))
            self.key_root(a, f0 + 3)
        self.pos[a.id] = p
        self.key_root(a, f1)
        if a.kind == "character":
            self.gait(a, f0, f1, run)

    def gait(self, a, f0, f1, run):
        period = 8 if run else 12
        leg, arm, knee = (45, 40, 60) if run else (25, 18, 30)
        joint_rest(a, LIMBS, f0)
        pel = a.j("pelvis")
        base = a.rest_loc.get("pelvis")
        f, side = f0 + 2, 1
        while f < f1 - 2:
            joint_rot(a, "hip.L", f, x=-leg * side)
            joint_rot(a, "hip.R", f, x=leg * side)
            joint_rot(a, "knee.L", f, x=knee if side < 0 else 5)
            joint_rot(a, "knee.R", f, x=knee if side > 0 else 5)
            joint_rot(a, "shoulder.L", f, x=arm * side)
            joint_rot(a, "shoulder.R", f, x=-arm * side)
            joint_rot(a, "elbow.L", f, x=-20 if run else -8)
            joint_rot(a, "elbow.R", f, x=-20 if run else -8)
            if pel is not None and base is not None:
                key_loc(pel, f, base + V((0, 0, 0.03 if run else 0.015)))
                key_loc(pel, f + period // 4, base)
            f += period // 2
            side = -side
        joint_rest(a, LIMBS, f1)
        if pel is not None and base is not None:
            key_loc(pel, f1, base)

    def strike(self, a, f0, f1, target, limb):
        if target is not None:
            self.turn(a, f0, f0 + 2, target)
        joint_rest(a, LIMBS, f0)
        mid = f0 + max(2, (f1 - f0) // 3)
        if limb == "arm":
            joint_rot(a, "shoulder.R", mid - 2, x=20)
            joint_rot(a, "elbow.R", mid - 2, x=-100)
            joint_rot(a, "shoulder.R", mid, x=-88)
            joint_rot(a, "elbow.R", mid, x=0)
            joint_rot(a, "shoulder.L", mid, x=25)
            joint_rot(a, "elbow.L", mid, x=-90)
        else:
            joint_rot(a, "hip.R", mid, x=-85)
            joint_rot(a, "knee.R", mid, x=10)
            joint_rot(a, "hip.L", mid, x=10)
        # lunge forward a little
        h = self.head[a.id]
        fwd = V((math.sin(h), -math.cos(h), 0))
        self.key_root(a, f0)
        start = self.pos[a.id].copy()
        key_loc(a.root, mid, start + fwd * 0.25)
        key_loc(a.root, f1, start)
        joint_rest(a, LIMBS, f1)

    def gesture(self, a, f0, f1, kind):
        joint_rest(a, LIMBS, f0)
        if kind == "talk":
            f, s = f0 + 3, 1
            while f < f1 - 3:
                joint_rot(a, "head", f, x=4 * s, z=5 * s)
                f += 7
                s = -s
            joint_rot(a, "shoulder.R", f0 + 6, x=-30, y=0, z=-10)
            joint_rot(a, "elbow.R", f0 + 6, x=-55)
            joint_rot(a, "shoulder.R", max(f0 + 7, f1 - 6), x=-25, z=-10)
            joint_rot(a, "elbow.R", max(f0 + 7, f1 - 6), x=-50)
        elif kind == "nod":
            for i, f in enumerate(range(f0 + 3, f1 - 2, 6)):
                joint_rot(a, "head", f, x=15 if i % 2 == 0 else 0)
        elif kind == "shake_head":
            for i, f in enumerate(range(f0 + 3, f1 - 2, 5)):
                joint_rot(a, "head", f, z=18 if i % 2 == 0 else -18)
        elif kind in ("point", "raise_arm", "wave"):
            up = -90 if kind == "point" else -165
            joint_rot(a, "shoulder.R", f0 + 5, x=up)
            if kind == "wave":
                for i, f in enumerate(range(f0 + 7, f1 - 4, 5)):
                    joint_rot(a, "shoulder.R", f, x=up, y=0, z=18 if i % 2 == 0 else -18)
            joint_rot(a, "shoulder.R", max(f0 + 6, f1 - 5), x=up)
        elif kind == "guard":
            for side in ("L", "R"):
                joint_rot(a, "shoulder." + side, f0 + 4, x=-60)
                joint_rot(a, "elbow." + side, f0 + 4, x=-110)
                joint_rot(a, "shoulder." + side, max(f0 + 5, f1 - 4), x=-60)
                joint_rot(a, "elbow." + side, max(f0 + 5, f1 - 4), x=-110)
        elif kind in ("lift", "exercise"):
            for i, f in enumerate(range(f0 + 4, f1 - 3, 10)):
                for side in ("L", "R"):
                    joint_rot(a, "shoulder." + side, f, x=-40 if i % 2 == 0 else -10)
                    joint_rot(a, "elbow." + side, f, x=-130 if i % 2 == 0 else -30)
        elif kind == "bow":
            pel = a.j("pelvis")
            if pel is not None:
                key_rot(pel, f0, a.rest["pelvis"].copy())
                key_rot(pel, f0 + 8, mathutils.Euler((a.rest["pelvis"].x + math.radians(35), 0, 0)))
                key_rot(pel, max(f0 + 9, f1 - 4), mathutils.Euler((a.rest["pelvis"].x + math.radians(35), 0, 0)))
                key_rot(pel, f1, a.rest["pelvis"].copy())
        joint_rest(a, LIMBS, f1)

    def jump(self, a, f0, f1):
        self.key_root(a, f0)
        joint_rest(a, LIMBS, f0)
        mid = (f0 + f1) // 2
        p = self.pos[a.id]
        for side in ("L", "R"):
            joint_rot(a, "knee." + side, f0 + 3, x=50)
            joint_rot(a, "hip." + side, f0 + 3, x=-40)
            joint_rot(a, "knee." + side, mid, x=20)
            joint_rot(a, "hip." + side, mid, x=-20)
            joint_rot(a, "shoulder." + side, mid, x=-150)
        key_loc(a.root, f0 + 3, p.copy())
        key_loc(a.root, mid, p + V((0, 0, 0.6 * a.height)))
        key_loc(a.root, f1 - 2, p.copy())
        joint_rest(a, LIMBS, f1)

    def fall(self, a, f0, f1, backward=True):
        self.key_root(a, f0)
        rx = math.radians(-88 if backward else 88)
        mid = min(f1, f0 + max(6, (f1 - f0) // 3))
        key_rot(a.root, mid, mathutils.Euler((rx, 0, self.head[a.id])))
        key_loc(a.root, mid, self.pos[a.id] + V((0, 0, 0.12 * a.height)))
        a.root.rotation_euler.x = rx  # stays down
        joint_rot(a, "shoulder.L", mid, x=-40, y=0, z=40)
        joint_rot(a, "shoulder.R", mid, x=-40, y=0, z=-40)
        key_rot(a.root, f1, mathutils.Euler((rx, 0, self.head[a.id])))
        key_loc(a.root, f1, self.pos[a.id] + V((0, 0, 0.12 * a.height)))

    def get_up(self, a, f0, f1):
        a.root.rotation_euler.x = a.root.rotation_euler.x
        key_rot(a.root, f0)
        key_loc(a.root, f0)
        a.root.rotation_euler.x = 0.0
        self.key_root(a, f1)
        joint_rest(a, LIMBS, f1)

    def sit(self, a, f0, f1):
        joint_rest(a, LIMBS, f0)
        pel = a.j("pelvis")
        base = a.rest_loc.get("pelvis")
        if pel is not None and base is not None:
            key_loc(pel, f0, base)
            key_loc(pel, f0 + 10, base - V((0, 0, 0.24 * a.height)))
            key_loc(pel, f1, base - V((0, 0, 0.24 * a.height)))
        for side in ("L", "R"):
            for f in (f0 + 10, f1):
                joint_rot(a, "hip." + side, f, x=-90)
                joint_rot(a, "knee." + side, f, x=90)

    def idle(self, a, f0, f1):
        pel = a.j("pelvis")
        base = a.rest_loc.get("pelvis")
        if pel is None or base is None or f1 - f0 < 12:
            return
        f, up = f0, True
        while f <= f1:
            key_loc(pel, f, base + V((0, 0, 0.006 if up else 0.0)))
            f += 24
            up = not up
        key_loc(pel, f1, base)


def run_actions(anim, actors, actions, last_frame):
    busy = {a: [] for a in actors}
    for act in sorted(actions, key=lambda x: x.get("frames", [0, 0])[0]):
        a = actors.get(act.get("actor"))
        if a is None:
            continue
        f0, f1 = [int(v) for v in act.get("frames", [1, 24])]
        f1 = max(f1, f0 + 4)
        t = str(act.get("type", "idle")).lower()
        tgt = act.get("target")
        busy[a.id].append((f0, f1))
        is_char = a.kind == "character"
        if t in ("walk_to", "walk", "move_to", "approach", "enter"):
            anim.move(a, f0, f1, tgt, run=False)
        elif t in ("run_to", "run", "charge", "dash", "chase", "flee"):
            anim.move(a, f0, f1, tgt, run=True)
        elif t in ("turn_to", "turn", "look_at", "face"):
            anim.turn(a, f0, f1, tgt)
        elif not is_char:
            continue
        elif t in ("punch", "hit", "strike", "attack", "slash", "slap"):
            anim.strike(a, f0, f1, tgt, "arm")
        elif t in ("kick",):
            anim.strike(a, f0, f1, tgt, "leg")
        elif t in ("talk", "speak", "shout", "explain", "argue", "laugh"):
            if tgt is not None:
                anim.turn(a, f0, f0 + 4, tgt)
            anim.gesture(a, f0, f1, "talk")
        elif t in ("nod", "agree"):
            anim.gesture(a, f0, f1, "nod")
        elif t in ("shake_head", "refuse", "disagree"):
            anim.gesture(a, f0, f1, "shake_head")
        elif t in ("point", "raise_arm", "wave"):
            anim.gesture(a, f0, f1, t)
        elif t in ("guard", "block", "defend", "stance"):
            anim.gesture(a, f0, f1, "guard")
        elif t in ("lift", "exercise", "train", "workout"):
            anim.gesture(a, f0, f1, "lift")
        elif t in ("bow", "kneel"):
            anim.gesture(a, f0, f1, "bow")
        elif t in ("jump", "leap"):
            anim.jump(a, f0, f1)
        elif t in ("fall", "collapse", "knocked_down", "die", "faint"):
            anim.fall(a, f0, f1, backward=t != "collapse")
        elif t in ("get_up", "stand_up", "stand"):
            anim.get_up(a, f0, f1)
        elif t in ("sit", "sit_down", "rest"):
            anim.sit(a, f0, f1)
    for aid, a in actors.items():
        if a.kind != "character":
            continue
        spans = sorted(busy[aid])
        cur = 1
        for f0, f1 in spans:
            if f0 - cur > 12:
                anim.idle(a, cur, f0 - 1)
            cur = max(cur, f1 + 1)
        if last_frame - cur > 12:
            anim.idle(a, cur, last_frame)


# ── cameras ─────────────────────────────────────────────────────────────────────────────────

FRAMING = {
    # framing: (visible height in metres or None for "relative to subject", focus as share of subject height)
    "extreme_close": (0.32, 0.92),
    "close": (0.6, 0.88),
    "medium_close": (0.85, 0.82),
    "medium": (1.15, 0.72),
    "full": (None, 0.55),
    "wide": ("wide", 0.5),
    "establishing": ("establishing", 0.3),
}
LENS = 35.0


def vfov(width, height):
    hf = 2 * math.atan(36.0 / (2 * LENS))
    return 2 * math.atan(math.tan(hf / 2) * height / width)


def actor_focus(a, share):
    """A point on the actor at `share` of its height, following the head when it lies, sits or jumps."""
    base = a.root.matrix_world.translation.copy()
    head = a.j("head")
    if head is None:
        return base + V((0, 0, a.height * share))
    top = head.matrix_world.translation + V((0, 0, 0.08 * a.height))
    return base.lerp(top, min(1.0, share / 0.92))


def actor_state(a, f):
    bpy.context.scene.frame_set(f)
    m = a.root.matrix_world
    return m.translation.copy(), m.to_euler().z


def blocked(cam, focus, others, f):
    """Another actor stands between the camera and the subject."""
    seg = (focus - cam).to_2d()
    L = seg.length
    if L < 1e-3:
        return False
    for o in others:
        p, _ = actor_state(o, f)
        rel = (p - cam).to_2d()
        t = rel.dot(seg) / (L * L)
        if 0.05 < t < 0.95 and (rel - seg * t).length < 0.45:
            return True
    return False


def clear_azimuth(az, focus, front, dist, others, f):
    for d in (0, -45, 45, -80, 80, -120, 120, 180):
        a = az + math.radians(d)
        cam = focus + (mathutils.Matrix.Rotation(a, 3, "Z") @ front) * dist
        if not blocked(cam, focus, others, f):
            return a
    return az


def make_cameras(spec, actors, env_box):
    scene = bpy.context.scene
    fov = vfov(spec["width"], spec["height"])
    out = []
    first_actor = next(iter(actors.values()), None)
    for i, s in enumerate(spec.get("shots", [])):
        f0, f1 = [int(v) for v in s["frames"]]
        subj = actors.get(s.get("subject")) or first_actor
        sec = actors.get(s.get("secondary"))
        framing = str(s.get("framing") or "medium").lower().replace("-", "_").replace(" ", "_")
        if framing in ("two_shot", "over_shoulder") and sec is None:
            framing = "medium"
        angle = str(s.get("angle") or "front").lower()
        move = str(s.get("move") or "static").lower()
        cam_data = bpy.data.cameras.new("cam_%02d" % i)
        cam_data.lens = LENS
        cam_data.clip_end = 500
        cam = bpy.data.objects.new("cam_%02d" % i, cam_data)
        scene.collection.objects.link(cam)
        tgt = bpy.data.objects.new("cam_%02d_target" % i, None)
        scene.collection.objects.link(tgt)
        con = cam.constraints.new("TRACK_TO")
        con.target = tgt
        con.track_axis = "TRACK_NEGATIVE_Z"
        con.up_axis = "UP_Y"
        keys = list(range(f0, f1 + 1, 6))
        if keys[-1] != f1:
            keys.append(f1)
        seed = random.Random(i * 7 + 1)
        shot_az = None
        others = [o for o in actors.values() if o is not subj and o is not sec]
        for n, f in enumerate(keys):
            t = (f - f0) / max(1, f1 - f0)
            if subj is not None:
                p, h = actor_state(subj, f)
                H = subj.height
            else:
                p, h, H = V((0, 0, 0)), 0.0, 1.7
            if framing == "two_shot" and sec is not None:
                q, _ = actor_state(sec, f)
                centre = (p + q) / 2
                span = (p - q).length
                d = (q - p)
                facing = heading_to(V((-d.y, d.x, 0))) if d.length > 1e-3 else h
                vis = max(H * 1.3, span * 0.8 + H * 0.6)
                focus = centre + V((0, 0, H * 0.65))
            elif framing == "over_shoulder" and sec is not None:
                q, _ = actor_state(sec, f)
                d = p - q
                dd = d.normalized() if d.length > 1e-3 else V((0, -1, 0))
                side = V((dd.y, -dd.x, 0))
                focus = actor_focus(subj, 0.85)
                pos = q - dd * 1.7 + side * 0.8 + V((0, 0, sec.height * 1.02))
                out_pos = pos
                key_loc(tgt, f, focus)
                key_loc(cam, f, out_pos)
                continue
            else:
                vis_spec, share = FRAMING.get(framing, FRAMING["medium"])
                if vis_spec is None:
                    vis = H * 1.3
                elif vis_spec == "wide":
                    vis = max(H * 3.2, 5.0)
                elif vis_spec == "establishing":
                    vis = max(env_box[1].x - env_box[0].x, 12.0) * 0.55 if env_box else 14.0
                else:
                    vis = vis_spec * H / 1.75
                focus = actor_focus(subj, share) if subj is not None else p + V((0, 0, H * share))
                facing = h
            dist = vis / (2 * math.tan(fov / 2))
            az = shot_az if shot_az is not None else {"front": math.radians(22), "side": math.radians(90), "back": math.radians(170), "left": math.radians(-60), "right": math.radians(60)}.get(angle, math.radians(22))
            elev = {"low": math.radians(-14), "high": math.radians(32), "top": math.radians(70)}.get(angle, math.radians(6))
            if framing == "establishing":
                elev = max(elev, math.radians(24))
            if framing == "two_shot":
                # Side-on puts one actor behind the other: frame both from the front.
                az = math.radians(8)
            if move == "push_in":
                dist *= 1.0 - 0.3 * t
            elif move == "pull_out":
                dist *= 0.7 + 0.3 * t
            elif move in ("pan_left", "pan_right"):
                az += math.radians(25) * t * (1 if move == "pan_left" else -1)
            elif move == "orbit":
                az += math.radians(70) * t
            # The camera looks at the subject's front: facing h means the front points (sin h, -cos h).
            front = V((math.sin(facing), -math.cos(facing), 0))
            if shot_az is None:
                az = clear_azimuth(az, focus, front, dist, others, f)
                shot_az = az
            rot = mathutils.Matrix.Rotation(az, 3, "Z")
            horiz = rot @ front
            # Sets are open on -Y (the camera side): keep the camera there unless the shot is from behind.
            if angle != "back" and horiz.y > 0.15:
                horiz = V((horiz.x, -horiz.y, 0))
            pos = focus + horiz * dist * math.cos(elev) + V((0, 0, dist * math.sin(elev)))
            if angle == "low":
                pos.z = max(0.25, min(pos.z, focus.z - 0.2))
            pos.z = max(pos.z, 0.2)
            if env_box is not None:
                lo, hi = env_box
                pos.x = min(max(pos.x, lo.x + 0.4), hi.x - 0.4)
                pos.y = min(max(pos.y, lo.y - 6.0), hi.y - 0.4)
                if hi.z > 2.5:
                    pos.z = min(pos.z, hi.z - 0.3)
            if move == "shake" or (move == "handheld"):
                pos += V((seed.uniform(-0.04, 0.04), seed.uniform(-0.04, 0.04), seed.uniform(-0.03, 0.03)))
            key_loc(tgt, f, focus)
            key_loc(cam, f, pos)
        m = scene.timeline_markers.new("shot_%02d" % i, frame=f0)
        m.camera = cam
        if i == 0:
            scene.camera = cam
        out.append({"id": s.get("id", "shot_%02d" % i), "camera": cam, "frames": [f0, f1]})
    return out


# ── world and lights ────────────────────────────────────────────────────────────────────────

SKIES = {
    "day": ("#9fc3e8", 0.55, 52, "#fff4e0", 2.2),
    "morning": ("#c3d6ea", 0.5, 25, "#ffe6c4", 2.0),
    "dawn": ("#d9b9a8", 0.7, 10, "#ffc896", 2.2),
    "dusk": ("#d08a5c", 0.6, 8, "#ff9a5a", 2.4),
    "evening": ("#7a6a8a", 0.5, 12, "#ffb07a", 1.8),
    "night": ("#121a30", 0.25, 40, "#9db4ff", 0.6),
    "interior": ("#3a3a40", 0.5, 60, "#fff0dc", 1.6),
}


def setup_world(spec, env_box):
    scene = bpy.context.scene
    tod = str(spec.get("time_of_day") or "day").lower()
    sky_col, strength, sun_el, sun_col, sun_e = SKIES.get(tod, SKIES["day"])
    sky = spec.get("sky") or {}
    if sky.get("color"):
        sky_col = sky["color"]
    world = bpy.data.worlds.new("mv_world")
    world.use_nodes = True
    nt = world.node_tree
    bg = nt.nodes.get("Background")
    hdri = sky.get("hdri")
    if hdri and os.path.isfile(hdri):
        env = nt.nodes.new("ShaderNodeTexEnvironment")
        env.image = bpy.data.images.load(hdri)
        nt.links.new(env.outputs["Color"], bg.inputs["Color"])
        bg.inputs["Strength"].default_value = strength
    else:
        bg.inputs["Color"].default_value = mv3d.rgba(sky_col)
        bg.inputs["Strength"].default_value = strength
    scene.world = world
    sun_d = bpy.data.lights.new("sun", "SUN")
    sun_d.energy = sun_e
    sun_d.color = mv3d.rgba(sun_col)[:3]
    sun_d.angle = math.radians(3)
    sun = bpy.data.objects.new("sun", sun_d)
    sun.rotation_euler = (math.radians(90 - sun_el), 0, math.radians(35))
    scene.collection.objects.link(sun)
    # Soft fill above the stage so faces never go black indoors or at night.
    top = (env_box[1].z - 0.3) if env_box and env_box[1].z > 2.5 else 5.0
    fill_d = bpy.data.lights.new("fill", "AREA")
    walled = env_box is not None and env_box[1].z > 2.5
    fill_d.energy = 1500 if walled else (400 if tod in ("interior", "night") else 60)
    fill_d.size = 10 if walled else 6
    if walled:
        bg.inputs["Strength"].default_value = max(bg.inputs["Strength"].default_value, 0.7)
    fill_d.color = mv3d.rgba("#fff2e0" if tod != "night" else "#b8c8ff")[:3]
    fill = bpy.data.objects.new("fill", fill_d)
    fill.location = (0, -1.5, min(top, 5.0))
    scene.collection.objects.link(fill)


def render_settings(spec, engine, w, h, samples):
    scene = bpy.context.scene
    scene.render.resolution_x = w
    scene.render.resolution_y = h
    scene.render.resolution_percentage = 100
    scene.render.image_settings.file_format = "PNG"
    # AgX keeps sunlit ground from burning out to white.
    scene.view_settings.view_transform = "AgX" if engine != "workbench" else "Standard"
    if engine == "workbench":
        mv3d.set_engine(scene, "workbench")
        sh = scene.display.shading
        sh.light = "STUDIO"
        sh.color_type = "MATERIAL"
        sh.show_object_outline = True
        sh.show_shadows = True
        sh.show_cavity = True
        sh.show_backface_culling = True
    else:
        mv3d.set_engine(scene, "eevee")
        try:
            scene.eevee.taa_render_samples = samples
            scene.eevee.use_shadows = True
        except AttributeError:
            pass


def main():
    spec = json.load(open(job_path(), encoding="utf-8"))
    bpy.ops.wm.read_factory_settings(use_empty=True)
    scene = bpy.context.scene
    fps = int(spec.get("fps", 24))
    scene.render.fps = fps
    shots = spec.get("shots", [])
    last = max([int(s["frames"][1]) for s in shots] + [fps])
    scene.frame_start = 1
    scene.frame_end = last

    env_box = None
    env = spec.get("environment") or {}
    if env.get("blend") and os.path.isfile(env["blend"]):
        col = append(env["blend"], "environment")
        root = next((o for o in col.all_objects if o.get("mv_joint") == "root"), None)
        bpy.context.view_layer.update()
        lo = V((1e9, 1e9, 1e9))
        hi = V((-1e9, -1e9, -1e9))
        for o in col.all_objects:
            if o.type == "MESH":
                for c in o.bound_box:
                    w = o.matrix_world @ V(c)
                    lo = V(map(min, lo, w))
                    hi = V(map(max, hi, w))
        if lo.x < hi.x:
            env_box = (lo, hi)
    else:
        mv3d.plane("ground", (60, 60), material=mv3d.mat("ground", "#6b6b6b", 0.9))

    actors = {}
    anim = Animator(actors, fps)
    for spec_a in spec.get("actors", []):
        if not spec_a.get("blend") or not os.path.isfile(spec_a["blend"]):
            continue
        col = append(spec_a["blend"], "actor_" + spec_a["id"])
        a = Actor(spec_a["id"], spec_a.get("kind", "prop"), col)
        actors[a.id] = a
        pos = spec_a.get("pos") or [0, 0]
        anim.place(a, pos, spec_a.get("facing"), float(spec_a.get("scale") or 1.0))
    for spec_a in spec.get("actors", []):
        a = actors.get(spec_a["id"])
        if a is not None:
            anim.face_initial(a, spec_a.get("facing"))
            anim.key_root(a, 1)
    # Appended assets (and downloaded models) keep no drivers, scripts or compositor.
    mv3d.sanitize()
    run_actions(anim, actors, spec.get("actions", []), last)
    setup_world(spec, env_box)
    cams = make_cameras(spec, actors, env_box)
    if not cams:
        raise RuntimeError("the scene has no shots")

    if spec.get("blend_out"):
        os.makedirs(os.path.dirname(spec["blend_out"]), exist_ok=True)
        bpy.ops.wm.save_as_mainfile(filepath=spec["blend_out"], compress=True)

    mode = spec.get("mode", "both")
    stills = []
    if mode in ("previz", "both"):
        os.makedirs(spec["previz_dir"], exist_ok=True)
        render_settings(spec, "workbench", int(spec.get("previz_width", 640)), int(spec.get("previz_height", 360)), 1)
        for i, c in enumerate(cams):
            f0, f1 = c["frames"]
            scene.frame_set((f0 + f1) // 2)
            scene.camera = c["camera"]
            path = os.path.join(spec["previz_dir"], "shot_%02d.png" % i)
            scene.render.filepath = path
            bpy.ops.render.render(write_still=True)
            stills.append(path)
        scene.camera = cams[0]["camera"]
    if mode in ("final", "both"):
        os.makedirs(spec["frames_dir"], exist_ok=True)
        render_settings(spec, "eevee", int(spec["width"]), int(spec["height"]), int(spec.get("samples", 16)))
        scene.render.filepath = os.path.join(spec["frames_dir"], "f_")
        bpy.ops.render.render(animation=True)
    print("MV3D_RESULT " + json.dumps({"frames": last, "fps": fps, "shots": [{"id": c["id"], "frames": c["frames"]} for c in cams], "previz": stills}))


try:
    main()
except Exception as e:  # noqa: BLE001
    print("MV3D_ERROR " + json.dumps({"error": str(e), "trace": traceback.format_exc()[-3000:]}))
    sys.exit(1)
