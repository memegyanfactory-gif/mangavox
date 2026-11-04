"""ffmpeg's expression language (libavutil/eval.c), evaluated with numpy over whole arrays.

The renderer writes its camera moves, shakes, fades and slides as ffmpeg expressions. The GPU
engine (gpu_render.py) works the same numbers out ahead of time, for every frame at once, from
the very same strings, so both paths move a picture identically.

    evaluate("1+0.05*(0.5-0.5*cos(PI*on/89))", on=np.arange(90))  -> array of 90 zooms

Variables are keyword arguments (scalars or arrays, broadcast together); names that are not
given raise KeyError. Precedence as in eval.c: unary sign over a whole power, `^` binds tighter
than `*` `/`, then `+` `-`; `;` sequences (the last value is the result)."""
import math
import re

import numpy as np

_NUM = re.compile(r"(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?")
_NAME = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
_CONSTS = {"PI": math.pi, "E": math.e, "PHI": (1 + 5 ** 0.5) / 2, "QP2LAMBDA": 118.0}


def _f(v):
    return np.asarray(v, dtype=np.float64)


def _clip(x, lo, hi):
    return np.minimum(np.maximum(x, lo), hi)


def _mod(a, b):
    # eval.c: a - floor(a / b) * b
    with np.errstate(divide="ignore", invalid="ignore"):
        return a - np.floor(a / b) * b


def _trunc(x):
    return np.trunc(x)


def _round(x):
    # C round(): halves away from zero.
    return np.sign(x) * np.floor(np.abs(x) + 0.5)


_FUNCS = {
    "sin": (1, np.sin), "cos": (1, np.cos), "tan": (1, np.tan), "atan": (1, np.arctan), "asin": (1, np.arcsin),
    "acos": (1, np.arccos), "sinh": (1, np.sinh), "cosh": (1, np.cosh), "tanh": (1, np.tanh),
    "exp": (1, np.exp), "log": (1, np.log), "abs": (1, np.abs), "sqrt": (1, np.sqrt),
    "floor": (1, np.floor), "ceil": (1, np.ceil), "trunc": (1, _trunc), "round": (1, _round),
    "not": (1, lambda x: (x == 0).astype(np.float64)), "squish": (1, lambda x: 1 / (1 + np.exp(4 * x))),
    "gauss": (1, lambda x: np.exp(-x * x / 2) / math.sqrt(2 * math.pi)),
    "pow": (2, lambda a, b: np.power(a, b)), "min": (2, np.minimum), "max": (2, np.maximum),
    "mod": (2, _mod), "hypot": (2, np.hypot), "atan2": (2, np.arctan2),
    "gt": (2, lambda a, b: (a > b).astype(np.float64)), "gte": (2, lambda a, b: (a >= b).astype(np.float64)),
    "lt": (2, lambda a, b: (a < b).astype(np.float64)), "lte": (2, lambda a, b: (a <= b).astype(np.float64)),
    "eq": (2, lambda a, b: (a == b).astype(np.float64)), "bitand": (2, lambda a, b: _f(np.bitwise_and(a.astype(np.int64), b.astype(np.int64)))),
    "bitor": (2, lambda a, b: _f(np.bitwise_or(a.astype(np.int64), b.astype(np.int64)))),
    "clip": (3, _clip), "between": (3, lambda x, a, b: ((x >= a) & (x <= b)).astype(np.float64)),
    "lerp": (3, lambda a, b, x: a + (b - a) * x),
}


class _Parser:
    def __init__(self, text):
        self.s = text
        self.i = 0

    def peek(self):
        while self.i < len(self.s) and self.s[self.i].isspace():
            self.i += 1
        return self.s[self.i] if self.i < len(self.s) else ""

    def take(self, ch):
        if self.peek() != ch:
            raise ValueError(f"expected {ch!r} at {self.i} in {self.s!r}")
        self.i += 1

    def sequence(self):
        nodes = [self.expr()]
        while self.peek() == ";":
            self.i += 1
            if self.peek() in ("", ")"):
                break
            nodes.append(self.expr())
        if len(nodes) == 1:
            return nodes[0]
        return lambda env: [n(env) for n in nodes][-1]

    def expr(self):
        node = self.term()
        while self.peek() in ("+", "-"):
            op = self.s[self.i]
            self.i += 1
            rhs = self.term()
            node = (lambda a, b: lambda env: a(env) + b(env))(node, rhs) if op == "+" else (lambda a, b: lambda env: a(env) - b(env))(node, rhs)
        return node

    def term(self):
        node = self.factor()
        while self.peek() in ("*", "/"):
            op = self.s[self.i]
            self.i += 1
            rhs = self.factor()
            if op == "*":
                node = (lambda a, b: lambda env: a(env) * b(env))(node, rhs)
            else:
                node = (lambda a, b: lambda env: _div(a(env), b(env)))(node, rhs)
        return node

    def factor(self):
        sign, node = self.signed_primary()
        while self.peek() == "^":
            self.i += 1
            s2, rhs = self.signed_primary()
            base = node
            node = (lambda a, b, s: lambda env: np.power(a(env), s * b(env)))(base, rhs, s2)
        if sign < 0:
            node = (lambda a: lambda env: -a(env))(node)
        return node

    def signed_primary(self):
        sign = 1
        c = self.peek()
        if c in ("+", "-"):
            sign = -1 if c == "-" else 1
            self.i += 1
        return sign, self.primary()

    def primary(self):
        c = self.peek()
        if c == "(":
            self.i += 1
            node = self.sequence()
            self.take(")")
            return node
        m = _NUM.match(self.s, self.i)
        if m:
            self.i = m.end()
            v = float(m.group(0))
            return lambda env: v
        m = _NAME.match(self.s, self.i)
        if not m:
            raise ValueError(f"unexpected {c!r} at {self.i} in {self.s!r}")
        name = m.group(0)
        self.i = m.end()
        if self.peek() == "(":
            self.i += 1
            args = [self.expr()]
            while self.peek() == ",":
                self.i += 1
                args.append(self.expr())
            self.take(")")
            return self.call(name, args)
        if name in _CONSTS:
            v = _CONSTS[name]
            return lambda env: v
        return lambda env: env[name]

    def call(self, name, args):
        if name == "if" or name == "ifnot":
            neg = name == "ifnot"
            c, a = args[0], args[1]
            b = args[2] if len(args) > 2 else (lambda env: 0.0)
            return lambda env: np.where((_f(c(env)) == 0) if neg else (_f(c(env)) != 0), a(env), b(env))
        if name == "st":
            k, v = args
            return lambda env: _store(env, k(env), v(env))
        if name == "ld":
            k = args[0]
            return lambda env: env.setdefault("__st", {}).get(int(_f(k(env))), 0.0)
        if name in ("max", "min") and len(args) > 2:
            f = _FUNCS[name][1]

            def many(env, args=args, f=f):
                out = args[0](env)
                for a in args[1:]:
                    out = f(out, a(env))
                return out
            return many
        if name not in _FUNCS:
            raise ValueError(f"unknown function {name}() in {self.s!r}")
        n, f = _FUNCS[name]
        if len(args) != n:
            raise ValueError(f"{name}() takes {n} arguments in {self.s!r}")
        return lambda env: f(*[_f(a(env)) for a in args])


def _div(a, b):
    with np.errstate(divide="ignore", invalid="ignore"):
        return _f(a) / _f(b)


def _store(env, k, v):
    env.setdefault("__st", {})[int(_f(k))] = v
    return v


_CACHE = {}


def compile_expr(text):
    """The expression `text` as a function of a dict of variables."""
    fn = _CACHE.get(text)
    if fn is None:
        p = _Parser(str(text))
        fn = p.sequence()
        if p.peek():
            raise ValueError(f"trailing {p.s[p.i:]!r} in {text!r}")
        _CACHE[text] = fn
    return fn


def evaluate(text, **variables):
    """`text` evaluated with `variables` (numbers or numpy arrays); a float64 array (0-d for scalars)."""
    env = {k: _f(v) for k, v in variables.items()}
    return _f(compile_expr(text)(env))


def frames(text, count, fps=30, on_base=0, **extra):
    """`text` for every frame 0..count-1 of a filter's clock: n (the frame counter, from 0), on and
    in (from `on_base`: perspective counts them from 1) and t (seconds) set, plus `extra`; an
    array of `count` values."""
    n = np.arange(count, dtype=np.float64)
    out = evaluate(text, n=n, on=n + on_base, **{"in": n + on_base}, t=n / fps, **extra)
    return np.broadcast_to(out, (count,)).astype(np.float64)
