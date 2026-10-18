"""Export game-rules data from the user's own install into a local content pack (never committed).

For each wanted MonoBehaviour class, writes pack/data/<Class>.json: a list of objects holding the plain serialized
fields plus the decoded Odin payload. Asset references stay as {file, path_id} for later passes to resolve.
"""
import argparse
import collections
import json
import pathlib
import sys
import time

import UnityPy

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))  # sibling modules under python -I
from UnityPy.helpers.TypeTreeGenerator import TypeTreeGenerator

import odin

# Every data-layer ScriptableObject/MonoBehaviour class (Mechanicus.Data.*, Mechanicus.Dungeon.Data.*).
DEFAULT_CLASSES = [
    "AreaSpawnDataMb", "AreaTriggerAllDataMb", "BarrierDataBase", "BarrierDataMb", "BarrierWallDataBase",
    "BarrierWallDataMb", "Blueprint", "BuffIconData", "BuffLibrary", "Canticle", "CanticleConfig",
    "ConfigLibrary", "CustomizationTechpriest", "CustomizationTechpriestModel", "DungeonEvent",
    "DungeonEventConfig", "DungeonLibrary", "DungeonModifier", "DungeonModifierConfig", "DungeonTemplate",
    "EmergePlatformDataMb", "EntityLibrary", "Equipment", "ExplorationLibrary", "FlayedOneSpawnDataBase",
    "FlayedOneSpawnDataMb", "FlyingPlatformDataMb", "GameplayConfig", "GeneratorDataBase", "GeneratorDataMb",
    "Glyph", "HUDLibrary", "KastelanRobotDataBase", "KataphronDataBase", "MapLibrary", "MechanicusLibrary",
    "Mission", "MissionConfig", "MissionLibrary", "NecronArchiveDataBase", "NecronCanoptekAcanthriteDataBase",
    "NecronCanoptekSpiderDataBase", "NecronCanoptekWraithDataBase", "NecronComputerDataBase",
    "NecronCryptekDataBase", "NecronCtanDataBase", "NecronCtanDataMb", "NecronCtanFireballMoveableDataBase",
    "NecronCtanFireballStaticDataBase", "NecronDeathMarkDataBase", "NecronDestroyerDataBase",
    "NecronDestroyerDataMb", "NecronDestroyerHeavyDataBase", "NecronFlayedOneDataBase",
    "NecronFlayedOneDataMb", "NecronImmortalGaussDataBase", "NecronImmortalTeslaDataBase",
    "NecronLordAgrolekhDataBase", "NecronLordAgrolekhDataMb", "NecronLordDataBase",
    "NecronLordEkropisDataBase", "NecronLordEkropisDataMb", "NecronLordMhelobDataBase",
    "NecronLordMhelobDataMb", "NecronLordNeftuskDataBase", "NecronLordNeftuskDataMb",
    "NecronLordSzaregonDataBase", "NecronLordUbjaoDataBase", "NecronLordUbjaoDataMb",
    "NecronLychguardShieldDataBase", "NecronLychguardSpearDataBase", "NecronPraetorianPcDataBase",
    "NecronPraetorianRoCDataBase", "NecronPropsDataBase", "NecronPropsDataMb", "NecronScarabDataBase",
    "NecronScarabDataMb", "NecronTacticalPointCollectableArchiveDataBase",
    "NecronTacticalPointCollectableArchiveDataMb", "NecronTurretCrossDataBase", "NecronTurretCrossDataMb",
    "NecronTurretDataBase", "NecronTurretDataMb", "NecronVargardDataBase", "NecronWarriorDataBase",
    "NecronWarriorDataMb", "NeftuskPillarDataBase", "NeftuskPillarDataMb", "RandomSpawnConfig",
    "RandomSpawnDeployment", "RandomSpawnDeploymentEntityData", "RandomSpawnDeploymentInteractive", "Room",
    "RoomAutoSelectTemplate", "RoomData", "RoomLibrary", "RoomTemplate", "SarcophagusDataBase",
    "SarcophagusDataMb", "ScarabsTrapDataBase", "ScarabsTrapDataMb", "ServitorDataBase", "ServoskullDataBase",
    "SicarianInfiltratorDataBase", "SicarianRustalkerDataBase", "Skill", "SkillTree",
    "SkitariiRangerDataBase", "SkitariiRangerMeleeDataBase", "SkitariiVanguardDataBase",
    "SkitariiVanguardMeleeDataBase", "StoryDialog", "StoryLibrary", "TechpriestDataBase", "TechpriestXP",
    "ThroneDataBase", "ThroneDataMb", "TroopBlueprint", "UIDungeonLibrary", "XenariteBossDataBase",
    "XenariteBossDataMb", "XenariteServitorDataBase", "XenariteServitorDataMb", "XenariteTechpriestDataBase",
    "XenariteTechpriestDataMb",
    # Configs outside those namespaces.
    "Character", "StoryMessageConfig", "NecronUnitOrderConfig", "NecronsConfig", "SectorConfig", "EntityConfig",
    "ShipSkillAddDeploymentPoints", "ShipSkillAddTechpriest", "ShipSkillAugmentCognitionGauge",
    "ShipSkillReduceTroopDeploymentCost", "ExternalBehaviorTree", "BehaviorTree",
    "IntroDialogFaustiniusReditus", "IntroDialogIntroduiceCharacters",
]


def class_names(path: pathlib.Path) -> dict[int, str]:
    """path_id -> script class name, read without generated type trees (cheap and never fails on layout)."""
    names = {}
    for obj in UnityPy.load(str(path)).objects:
        if obj.type.name == "MonoBehaviour":
            try:
                names[obj.path_id] = obj.read(check_read=False).m_Script.read().m_ClassName
            except Exception:
                pass
    return names


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("data_dir", type=pathlib.Path, help="<install>/Mechanicus_Data")
    ap.add_argument("--pack", type=pathlib.Path, required=True, help="content pack output directory")
    ap.add_argument("--classes", default=",".join(DEFAULT_CLASSES))
    ap.add_argument("--scenes", action="store_true", help="also scan scene files (level*), for scene-placed data")
    args = ap.parse_args()

    wanted = set(args.classes.split(","))
    gen = TypeTreeGenerator("2017.4.19f1")
    gen.load_local_dll_folder(str(args.data_dir / "Managed"))

    out = collections.defaultdict(list)
    externals = {}
    first_errors = {}
    failures = collections.Counter()
    started = time.time()
    files = sorted(args.data_dir.glob("*.assets"))
    if args.scenes:
        files += sorted(p for p in args.data_dir.glob("level*") if p.suffix == "")
    for path in files:
        names = class_names(path)
        if not wanted & set(names.values()):
            continue
        env = UnityPy.load(str(path))
        env.typetree_generator = gen
        for f in env.files.values():
            # PPtr m_FileID n > 0 means externals[n-1]; keep just the file name so references resolve by name.
            externals[path.name] = [pathlib.PurePosixPath(e.path.replace("\\", "/")).name for e in f.externals]
            break
        for obj in env.objects:
            cls = names.get(obj.path_id)
            if cls not in wanted:
                continue
            try:
                partial = False
                try:
                    tree = obj.read_typetree()
                except ValueError as e:
                    # The generated type tree can be a few bytes short of the stored object; the fields it does
                    # describe still read correctly, so keep them and flag the record.
                    if "Expected to read" not in str(e):
                        raise
                    tree = obj.read_typetree(check_read=False)
                    partial = True
                serialization = tree.pop("serializationData", None) or {}
                blob = serialization.get("SerializedBytes")
                record = {"file": path.name, "path_id": obj.path_id, "fields": tree}
                if partial:
                    record["partial"] = True
                if blob:
                    record["odin"] = odin.decode(blob)
                    record["odin_unity_refs"] = serialization.get("ReferencedUnityObjects", [])
                out[cls].append(record)
            except Exception as e:
                failures[f"{cls}: {type(e).__name__}"] += 1
                first_errors.setdefault(cls, f"{type(e).__name__}: {e}"[:300])
        print(f"{path.name}: done", file=sys.stderr)

    data_dir = args.pack / "data"
    data_dir.mkdir(parents=True, exist_ok=True)
    for cls, records in out.items():
        records.sort(key=lambda r: r["fields"].get("m_Name", ""))
        (data_dir / f"{cls}.json").write_text(json.dumps(records, default=str))
    (data_dir / "_files.json").write_text(json.dumps(externals, indent=0))
    summary = {
        "seconds": round(time.time() - started, 1),
        "counts": {cls: len(r) for cls, r in sorted(out.items())},
        "failures": dict(failures),
        "first_errors": first_errors,
    }
    (data_dir / "_summary.json").write_text(json.dumps(summary, indent=1))
    print(json.dumps(summary, indent=1))
    return 0


if __name__ == "__main__":
    sys.exit(main())
