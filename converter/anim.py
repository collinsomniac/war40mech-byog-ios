"""Decode Unity's compiled (Mecanim "muscle clip") generic animation and resample it for glTF.

A clip's curves are stored in three blocks, indexed globally in this order:
  streamed: per-key cubic segments, value(t) = ((c0*dt + c1)*dt + c2)*dt + c3 from the key's time
  dense:    frame-major samples at a fixed rate
  constant: one value per curve
Bindings (ClipBindingConstant.genericBindings) consume consecutive curves: position 3, rotation 4 (quaternion),
scale 3, euler 3 (degrees), anything else 1. Each binding names its transform by CRC32 of the path relative to
the Animator's GameObject; the Avatar's TOS table maps those hashes back to paths.
"""
import bisect
import math
import struct

ATTR_POSITION, ATTR_ROTATION, ATTR_SCALE, ATTR_EULER = 1, 2, 3, 4
TRANSFORM_TYPE_ID = 4  # binding.typeID for Transform


def _curve_width(binding) -> int:
    if binding.typeID != TRANSFORM_TYPE_ID:
        return 1
    return {ATTR_POSITION: 3, ATTR_ROTATION: 4, ATTR_SCALE: 3, ATTR_EULER: 3}.get(binding.attribute, 1)


class StreamedCurves:
    """Per-curve sorted key lists: (time, c0, c1, c2, c3)."""

    def __init__(self, words: list[int]):
        raw = struct.pack(f"<{len(words)}I", *words)
        self.keys: dict[int, list[tuple]] = {}
        pos = 0
        while pos + 8 <= len(raw):
            time, count = struct.unpack_from("<fI", raw, pos)
            pos += 8
            for _ in range(count):
                index, c0, c1, c2, c3 = struct.unpack_from("<i4f", raw, pos)
                pos += 20
                self.keys.setdefault(index, []).append((time, c0, c1, c2, c3))
        self.times = {i: [k[0] for k in ks] for i, ks in self.keys.items()}

    def value(self, curve: int, t: float) -> float | None:
        keys = self.keys.get(curve)
        if not keys:
            return None
        i = max(0, bisect.bisect_right(self.times[curve], t) - 1)
        time, c0, c1, c2, c3 = keys[i]
        dt = max(0.0, t - time) if math.isfinite(time) else 0.0
        return ((c0 * dt + c1) * dt + c2) * dt + c3


def _euler_to_quat(x: float, y: float, z: float) -> tuple[float, float, float, float]:
    """Unity euler degrees (applied Z, then X, then Y) to a Unity-space quaternion (x, y, z, w)."""
    hx, hy, hz = (math.radians(a) / 2 for a in (x, y, z))
    cx, sx, cy, sy, cz, sz = math.cos(hx), math.sin(hx), math.cos(hy), math.sin(hy), math.cos(hz), math.sin(hz)
    return (cy * sx * cz + sy * cx * sz,
            sy * cx * cz - cy * sx * sz,
            cy * cx * sz - sy * sx * cz,
            cy * cx * cz + sy * sx * sz)


def _normalize(q):
    n = math.sqrt(sum(c * c for c in q)) or 1.0
    return tuple(c / n for c in q)


def sample_clip(clip, path_of_hash: dict[int, str], fps: float = 30.0):
    """Yield (path, property, times, values) per animated transform property, in glTF space.

    property is 'translation' | 'rotation' | 'scale'; values are tuples per time.
    """
    muscle = clip.m_MuscleClip
    data = muscle.m_Clip.data
    streamed = StreamedCurves(list(data.m_StreamedClip.data))
    streamed_count = data.m_StreamedClip.curveCount
    dense = data.m_DenseClip
    dense_count = dense.m_CurveCount
    constant = list(data.m_ConstantClip.data)

    start, stop = muscle.m_StartTime, muscle.m_StopTime
    frames = max(2, int(round((stop - start) * fps)) + 1)
    times = [start + (stop - start) * i / (frames - 1) for i in range(frames)]

    def curve_value(index: int, t: float) -> float:
        if index < streamed_count:
            v = streamed.value(index, t)
            return 0.0 if v is None else v
        index -= streamed_count
        if index < dense_count:
            frame = min(dense.m_FrameCount - 1, max(0, (t - dense.m_BeginTime) * dense.m_SampleRate))
            f0 = int(frame)
            f1 = min(dense.m_FrameCount - 1, f0 + 1)
            a = dense.m_SampleArray[f0 * dense_count + index]
            b = dense.m_SampleArray[f1 * dense_count + index]
            return a + (b - a) * (frame - f0)
        index -= dense_count
        return constant[index] if index < len(constant) else 0.0

    curve = 0
    for binding in clip.m_ClipBindingConstant.genericBindings:
        width = _curve_width(binding)
        first = curve
        curve += width
        if binding.typeID != TRANSFORM_TYPE_ID or binding.attribute not in (1, 2, 3, 4):
            continue
        path = path_of_hash.get(binding.path)
        if path is None:
            continue
        samples = [tuple(curve_value(first + k, t) for k in range(width)) for t in times]
        if binding.attribute == ATTR_POSITION:
            yield path, "translation", times, [(-x, y, z) for x, y, z in samples]
        elif binding.attribute == ATTR_SCALE:
            yield path, "scale", times, samples
        else:
            quats = samples if binding.attribute == ATTR_ROTATION else [_euler_to_quat(*s) for s in samples]
            out, prev = [], None
            for q in quats:
                x, y, z, w = _normalize(q)
                q = (x, -y, -z, w)
                if prev is not None and sum(a * b for a, b in zip(prev, q)) < 0:
                    q = tuple(-c for c in q)  # keep the shortest arc between samples for linear slerp
                out.append(q)
                prev = q
            yield path, "rotation", times, out
