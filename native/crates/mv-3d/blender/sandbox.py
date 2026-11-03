"""Sandbox for AI-written asset scripts. Pure Python: this module never imports bpy.

The script comes from an AI model that read untrusted manga pages, so it never touches Blender.
It runs against a recording API: every helper call (box, cyl, mat, ...) is checked and stored as
a plain operation, and helpers return opaque parts that only allow `location`, `rotation`,
`scale`, `name` and `parent`. asset_runner.py then replays the operations with the real mv3d
helpers. With no bpy object in reach there is no RNA to abuse (compositor, drivers, sequencer,
file paths).

On top of that the code is checked before it runs: attribute access is an allowlist, names and
arguments may not start with "_", strings may not contain "__", and imports, classes, match
statements, global/nonlocal and async code are rejected. Builtins are a small allowlist and
print is capped and prefixed, so a script cannot flood the log or forge the runner's result line.
"""
import ast
import builtins
import math
import random

MAX_OPS = 20000
MAX_SCATTER = 500
MAX_PRINT = 4000
LIMIT = 1000.0  # metres; anything larger is clamped

# Every attribute a script may read or write. Parts: location/rotation/scale/name/parent.
# The rest are value methods of str/list/dict, the math and random modules and Vector.
ALLOWED_ATTRS = {
    "location", "rotation", "scale", "name", "parent",
    "append", "extend", "insert", "pop", "index", "count", "sort", "reverse", "remove", "clear", "copy",
    "items", "keys", "values", "get", "update", "setdefault",
    "lower", "upper", "strip", "lstrip", "rstrip", "split", "join", "replace", "startswith", "endswith",
    "title", "capitalize", "zfill",
    "pi", "tau", "e", "inf", "sin", "cos", "tan", "asin", "acos", "atan", "atan2", "sqrt", "radians", "degrees",
    "floor", "ceil", "hypot", "exp", "log", "fabs", "copysign", "fmod", "isfinite", "dist", "trunc",
    "random", "uniform", "randint", "choice", "gauss", "shuffle", "sample", "seed", "Random", "randrange",
    "triangular",
    "x", "y", "z", "length", "normalized", "dot", "cross", "lerp", "to_tuple",
}
SAFE_BUILTINS = ("abs", "all", "any", "bool", "dict", "divmod", "enumerate", "filter", "float", "int", "isinstance",
                 "len", "list", "map", "max", "min", "pow", "range", "reversed", "round", "set", "sorted", "str",
                 "sum", "tuple", "zip", "Exception", "ValueError", "TypeError", "IndexError", "KeyError",
                 "ZeroDivisionError", "RuntimeError")
REJECTED_NODES = (ast.Import, ast.ImportFrom, ast.ClassDef, ast.Global, ast.Nonlocal, ast.Match, ast.AsyncFunctionDef,
                  ast.AsyncFor, ast.AsyncWith, ast.Await, ast.Yield, ast.YieldFrom)


class SandboxError(Exception):
    pass


def _bad_name(n):
    return n is not None and n.startswith("_")


def check_code(code):
    """Parses and checks the script; raises SandboxError naming the line and the problem."""
    try:
        tree = ast.parse(code)
    except SyntaxError as e:
        raise SandboxError("syntax error on line %s: %s" % (e.lineno, e.msg))
    for node in ast.walk(tree):
        line = getattr(node, "lineno", "?")
        if isinstance(node, REJECTED_NODES):
            raise SandboxError("line %s: %s is not allowed" % (line, type(node).__name__))
        if isinstance(node, ast.Attribute) and node.attr not in ALLOWED_ATTRS:
            raise SandboxError("line %s: '.%s' is not available; only the helpers and plain values can be used" % (line, node.attr))
        if isinstance(node, ast.Name) and _bad_name(node.id):
            raise SandboxError("line %s: names may not start with '_'" % line)
        if isinstance(node, ast.arg) and _bad_name(node.arg):
            raise SandboxError("line %s: argument names may not start with '_'" % line)
        if isinstance(node, ast.keyword) and _bad_name(node.arg):
            raise SandboxError("line %s: keyword names may not start with '_'" % line)
        if isinstance(node, ast.FunctionDef) and _bad_name(node.name):
            raise SandboxError("line %s: function names may not start with '_'" % line)
        if isinstance(node, ast.Constant) and isinstance(node.value, (str, bytes)) and ("__" in str(node.value)):
            raise SandboxError("line %s: strings may not contain '__'" % line)
    return tree


# ── values ─────────────────────────────────────────────────────────────────────────────────

def num(v, lo=-LIMIT, hi=LIMIT, default=0.0):
    try:
        f = float(v)
    except (TypeError, ValueError):
        return default
    if not math.isfinite(f):
        return default
    return min(max(f, lo), hi)


def vec3(v, lo=-LIMIT, hi=LIMIT, default=(0.0, 0.0, 0.0)):
    try:
        items = list(v)[:3]
    except TypeError:
        return tuple(default)
    while len(items) < 3:
        items.append(default[len(items)])
    return tuple(num(x, lo, hi, d) for x, d in zip(items, default))


def vec2(v, lo=0.01, hi=LIMIT, default=(1.0, 1.0)):
    try:
        items = list(v)[:2]
    except TypeError:
        items = []
    while len(items) < 2:
        items.append(default[len(items)])
    return tuple(num(x, lo, hi, d) for x, d in zip(items, default))


def text(v, n=63):
    s = "".join(c for c in str(v) if c.isprintable())
    return s[:n] or "part"


def color(v):
    if isinstance(v, str):
        return text(v, 16)
    if isinstance(v, (list, tuple)):
        return tuple(num(x, 0.0, 255.0) for x in list(v)[:3])
    return "#999999"


# ── recording API ──────────────────────────────────────────────────────────────────────────

class Recorder:
    def __init__(self):
        self.ops = []
        self.next_id = 0
        self.printed = 0

    def add(self, op):
        if len(self.ops) >= MAX_OPS:
            raise SandboxError("the script made more than %d parts" % MAX_OPS)
        self.ops.append(op)

    def new_id(self):
        self.next_id += 1
        return self.next_id


class Part:
    """An object the script made. Only location, rotation (degrees), scale, name and parent."""
    __slots__ = ("_id", "_rec", "_vals")

    def __init__(self, rec, pid, name, loc=(0, 0, 0), rot=(0, 0, 0), scale=(1, 1, 1)):
        object.__setattr__(self, "_id", pid)
        object.__setattr__(self, "_rec", rec)
        object.__setattr__(self, "_vals", {"name": name, "location": tuple(loc), "rotation": tuple(rot), "scale": tuple(scale), "parent": None})

    def _set(self, field, value):
        self._vals[field] = value
        self._rec.add({"op": "set", "id": self._id, "field": field, "value": value})

    @property
    def name(self):
        return self._vals["name"]

    @property
    def location(self):
        return self._vals["location"]

    @location.setter
    def location(self, v):
        self._set("location", vec3(v))

    @property
    def rotation(self):
        return self._vals["rotation"]

    @rotation.setter
    def rotation(self, v):
        self._set("rotation", vec3(v, -36000.0, 36000.0))

    @property
    def scale(self):
        return self._vals["scale"]

    @scale.setter
    def scale(self, v):
        if isinstance(v, (int, float)):
            v = (v, v, v)
        self._set("scale", vec3(v, -100.0, 100.0, (1.0, 1.0, 1.0)))

    @property
    def parent(self):
        return self._vals["parent"]

    @parent.setter
    def parent(self, p):
        if not isinstance(p, Part):
            raise SandboxError("parent must be a part the script made")
        self._vals["parent"] = p
        self._rec.add({"op": "set", "id": self._id, "field": "parent", "value": p._id})


class Material:
    """A material the script made; only usable as material=..."""
    __slots__ = ("_id",)

    def __init__(self, mid):
        object.__setattr__(self, "_id", mid)


def make_api(rec):
    def ref(p, kind):
        if p is None:
            return None
        if not isinstance(p, kind):
            raise SandboxError("expected a %s made by the script" % ("part" if kind is Part else "material"))
        return p._id

    def prim(op, name, loc, args, rot=(0, 0, 0), material=None, parent=None, color_=None):
        pid = rec.new_id()
        a = dict(args)
        a["name"] = text(name)
        a["loc"] = vec3(loc)
        if rot is not None:
            a["rot"] = vec3(rot, -36000.0, 36000.0)
        a["color"] = color(color_ if color_ is not None else "#999999")
        a["material"] = ref(material, Material)
        a["parent"] = ref(parent, Part)
        rec.add({"op": op, "id": pid, "args": a})
        return Part(rec, pid, a["name"], a["loc"], a.get("rot", (0, 0, 0)))

    def mat(name, color="#999999", rough=0.6, metal=0.0, emit=0.0):
        mid = rec.new_id()
        rec.add({"op": "mat", "id": mid, "args": {"name": text(name), "color": globals()["color"](color), "rough": num(rough, 0, 1, 0.6), "metal": num(metal, 0, 1), "emit": num(emit, 0, 50)}})
        return Material(mid)

    def box(name, size=(1, 1, 1), loc=(0, 0, 0), rot=(0, 0, 0), color="#999999", material=None, parent=None, bevel=0.0):
        return prim("box", name, loc, {"size": vec3(size, 0.001, LIMIT, (1, 1, 1)), "bevel": num(bevel, 0, 1)}, rot, material, parent, color)

    def cyl(name, r=0.5, depth=1.0, loc=(0, 0, 0), rot=(0, 0, 0), color="#999999", material=None, parent=None, verts=24):
        return prim("cyl", name, loc, {"r": num(r, 0.001, LIMIT, 0.5), "depth": num(depth, 0.001, LIMIT, 1), "verts": int(num(verts, 3, 128, 24))}, rot, material, parent, color)

    def cone(name, r1=0.5, r2=0.0, depth=1.0, loc=(0, 0, 0), rot=(0, 0, 0), color="#999999", material=None, parent=None, verts=24):
        return prim("cone", name, loc, {"r1": num(r1, 0, LIMIT, 0.5), "r2": num(r2, 0, LIMIT), "depth": num(depth, 0.001, LIMIT, 1), "verts": int(num(verts, 3, 128, 24))}, rot, material, parent, color)

    def sphere(name, r=0.5, loc=(0, 0, 0), scale=(1, 1, 1), color="#999999", material=None, parent=None, segments=24):
        return prim("sphere", name, loc, {"r": num(r, 0.001, LIMIT, 0.5), "scale": vec3(scale, 0.001, 100, (1, 1, 1)), "segments": int(num(segments, 6, 96, 24))}, None, material, parent, color)

    def torus(name, r_major=0.5, r_minor=0.1, loc=(0, 0, 0), rot=(0, 0, 0), color="#999999", material=None, parent=None):
        return prim("torus", name, loc, {"r_major": num(r_major, 0.001, LIMIT, 0.5), "r_minor": num(r_minor, 0.001, LIMIT, 0.1)}, rot, material, parent, color)

    def plane(name, size=(10, 10), loc=(0, 0, 0), rot=(0, 0, 0), color="#777777", material=None, parent=None):
        return prim("plane", name, loc, {"size": vec2(size)}, rot, material, parent, color)

    def text_(name, body, loc=(0, 0, 0), size=0.5, color="#ffffff", rot=(90, 0, 0), parent=None):
        return prim("text", name, loc, {"body": text(body, 120), "size": num(size, 0.01, 100, 0.5)}, rot, None, parent, color)

    def empty(name, loc=(0, 0, 0), parent=None):
        pid = rec.new_id()
        rec.add({"op": "empty", "id": pid, "args": {"name": text(name), "loc": vec3(loc), "parent": ref(parent, Part)}})
        return Part(rec, pid, text(name), vec3(loc))

    def light(name, kind="POINT", energy=500.0, loc=(0, 0, 3), rot=(0, 0, 0), color="#ffffff", size=1.0):
        pid = rec.new_id()
        k = str(kind).upper()
        rec.add({"op": "light", "id": pid, "args": {"name": text(name), "kind": k if k in ("POINT", "SUN", "SPOT", "AREA") else "POINT",
                                                    "energy": num(energy, 0, 100000, 500), "loc": vec3(loc), "rot": vec3(rot, -36000.0, 36000.0),
                                                    "color": globals()["color"](color), "size": num(size, 0.01, 100, 1)}})
        return Part(rec, pid, text(name), vec3(loc))

    def smooth(part):
        rec.add({"op": "smooth", "id": ref(part, Part)})
        return part

    def set_parent(child, parent):
        child.parent = parent
        return child

    def scatter(fn, count, area=(10, 10), seed=1, z=0.0, avoid_center=0.0):
        rng = random.Random(int(num(seed, -1e9, 1e9, 1)))
        count = int(num(count, 0, MAX_SCATTER))
        ax, ay = vec2(area, 0.0)
        z = num(z)
        avoid = num(avoid_center, 0, LIMIT)
        placed = tries = 0
        while placed < count and tries < count * 20:
            tries += 1
            x, y = rng.uniform(-ax / 2, ax / 2), rng.uniform(-ay / 2, ay / 2)
            if math.hypot(x, y) < avoid:
                continue
            fn(placed, (x, y, z), rng)
            placed += 1

    def build_room(width=14.0, depth=12.0, height=4.0, floor="#8a7560", wall="#c9c2b6", trim="#4a3a2a", open_front=True):
        rec.add({"op": "call", "fn": "build_room", "args": {"width": num(width, 1, 200, 14), "depth": num(depth, 1, 200, 12), "height": num(height, 0.5, 50, 4),
                                                              "floor": color(floor), "wall": color(wall), "trim": color(trim), "open_front": bool(open_front)}})

    def build_outdoor(size=40.0, ground="#6d7a4a", features=12, seed=2, feature_color="#5a4a3a"):
        rec.add({"op": "call", "fn": "build_outdoor", "args": {"size": num(size, 1, 500, 40), "ground": color(ground), "features": int(num(features, 0, 200, 12)),
                                                                 "seed": int(num(seed, -1e9, 1e9, 2)), "feature_color": color(feature_color)}})

    def show(*args):
        # Capped and prefixed, so it can neither flood the log nor forge an MV3D_ line.
        s = " ".join(str(a) for a in args)
        left = MAX_PRINT - rec.printed
        if left > 0:
            s = s[:left].replace("\n", " ")
            rec.printed += len(s)
            builtins.print("[script] " + s)

    return {"mat": mat, "box": box, "cyl": cyl, "cone": cone, "sphere": sphere, "torus": torus, "plane": plane,
            "text": text_, "empty": empty, "light": light, "smooth": smooth, "set_parent": set_parent, "scatter": scatter,
            "build_room": build_room, "build_outdoor": build_outdoor, "print": show}


def record(code, name="asset_script"):
    """Checks and runs the script against the recording API; returns its operations."""
    tree = check_code(code)
    rec = Recorder()
    api = make_api(rec)
    safe = {n: getattr(builtins, n) for n in SAFE_BUILTINS}
    safe["print"] = api.pop("print")
    env = {"__builtins__": safe, "__name__": "mv_asset", "math": math, "random": random}
    try:
        from mathutils import Vector  # value type only; available inside Blender
        env["Vector"] = Vector
    except ImportError:
        pass
    env.update(api)
    exec(compile(tree, name, "exec"), env)
    return rec.ops
