"""Probe: for a prefab root, follow Animator -> controller -> clips and report how each clip's data is stored."""
import pathlib
import sys

import UnityPy

data_dir = pathlib.Path(sys.argv[1])
root_name = sys.argv[2]

env = UnityPy.load(str(data_dir / "resources.assets"))


def walk(transform):
    yield transform
    for child in transform.m_Children:
        yield from walk(child.read())


for obj in env.objects:
    if obj.type.name != "Transform":
        continue
    t = obj.read()
    if t.m_Father.path_id or t.m_GameObject.read().m_Name != root_name:
        continue
    for node in walk(t):
        go = node.m_GameObject.read()
        for c in go.m_Component:
            if c.component.type.name != "Animator":
                continue
            animator = c.component.read()
            print(f"Animator on {go.m_Name}: avatar={animator.m_Avatar.path_id} controller={animator.m_Controller.path_id}")
            controller = animator.m_Controller.read()
            print(f"  controller {controller.type.name if hasattr(controller, 'type') else type(controller).__name__}: "
                  f"{getattr(controller, 'm_Name', '?')}")
            clips = getattr(controller, "m_AnimationClips", None)
            if clips is None and hasattr(controller, "m_Controller"):
                clips = getattr(controller.m_Controller.read(), "m_AnimationClips", [])
            for ptr in (clips or [])[:40]:
                clip = ptr.read()
                muscle = getattr(clip, "m_MuscleClip", None)
                inner = muscle.m_Clip.data if muscle else None
                streamed = len(inner.m_StreamedClip.data) if inner else 0
                dense = (inner.m_DenseClip.m_FrameCount, inner.m_DenseClip.m_CurveCount) if inner else None
                const = len(inner.m_ConstantClip.data) if inner else 0
                bindings = len(clip.m_ClipBindingConstant.genericBindings)
                print(f"    {clip.m_Name:40} legacy={clip.m_Legacy} len={muscle.m_StopTime - muscle.m_StartTime if muscle else '?':.2f}s "
                      f"rotCurves={len(clip.m_RotationCurves)} streamed={streamed} dense={dense} const={const} bindings={bindings}")
    break
