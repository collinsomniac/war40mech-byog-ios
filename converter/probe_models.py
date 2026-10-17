"""Probe: list renderable objects matching a name, with mesh size, material, shader and main texture."""
import pathlib
import sys

import UnityPy

data_dir = pathlib.Path(sys.argv[1])
needle = sys.argv[2].lower()
limit = int(sys.argv[3]) if len(sys.argv) > 3 else 15

env = UnityPy.load(str(data_dir / "resources.assets"))
shown = 0
for obj in env.objects:
    if obj.type.name not in ("SkinnedMeshRenderer", "MeshRenderer"):
        continue
    renderer = obj.read()
    go = renderer.m_GameObject.read()
    if needle not in go.m_Name.lower():
        continue
    if obj.type.name == "SkinnedMeshRenderer":
        mesh_ptr = renderer.m_Mesh
    else:
        filters = [c for c in go.m_Component if c.component.type.name == "MeshFilter"]
        mesh_ptr = filters[0].component.read().m_Mesh if filters else None
    mesh = mesh_ptr.read() if mesh_ptr else None
    desc = f"{obj.type.name:20} {go.m_Name:40} mesh={mesh.m_Name if mesh else None}"
    if mesh:
        desc += f" verts={mesh.m_VertexCount if hasattr(mesh, 'm_VertexCount') else '?'}"
    for mat_ptr in renderer.m_Materials[:2]:
        try:
            mat = mat_ptr.read()
            shader = mat.m_Shader.read()
            tex_envs = dict((k, v) for k, v in mat.m_SavedProperties.m_TexEnvs)
            main = tex_envs.get("_MainTex") or tex_envs.get("_Albedo")
            tex = main.m_Texture.read().m_Name if main and main.m_Texture.path_id else None
            desc += f"\n      mat={mat.m_Name} shader={shader.m_ParsedForm.m_Name if hasattr(shader, 'm_ParsedForm') else shader.m_Name} tex={tex} props={list(tex_envs)[:8]}"
        except Exception as e:
            desc += f"\n      mat error {type(e).__name__}: {e}"
    print(desc)
    shown += 1
    if shown >= limit:
        break
