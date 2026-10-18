# Cogitator

A bring-your-own-copy engine for **Warhammer 40,000: Mechanicus** on iPhone and iPad, written in Rust with Bevy.

This repository contains **no game content and no code from the game**. You need your own copy of
Mechanicus (Steam). A converter on your PC reads that installation and produces a content pack; your PC serves
the pack to your phone over Tailscale; the app downloads it once and plays from it.

```
your Steam install ──► converter (PC) ──► content/pack ──► serve.py (Tailscale only) ──► Cogitator.app (iOS)
```

## Status

Playable loop: main menu → New Game → the tutorial tomb (generated, events, rewards, awakening) → battles
(the game's grids, spawns, movement, skills and Necron behaviour trees) → the Ark (missions on offer,
armoury, deployment) → the next tomb. See [docs/STATUS.md](docs/STATUS.md) for what is faithful,
approximate or still a placeholder, and [docs/METHOD.md](docs/METHOD.md) for how the port is built.

## Use

On the PC (Python 3.12, run once per game update; `<Data>` is `<Steam>\steamapps\common\Mechanicus\Mechanicus_Data`):

```
cd converter
python -m venv .venv && .venv\Scripts\pip install UnityPy TypeTreeGeneratorAPI numpy dnfile pillow
.venv\Scripts\python export_data.py <Data> --pack ..\content\pack --scenes     # every data record
.venv\Scripts\python export_enums.py <Data> --pack ..\content\pack             # enum tables from the DLL
.venv\Scripts\python export_ui.py <Data> --pack ..\content\pack --loc --fonts --scene level5=MainMenu --scene level11=DungeonUI
.venv\Scripts\python export_prefab.py <Data> --pack ..\content\pack --scene level5=MainMenuScene --scene level1=ExplorationScene
.venv\Scripts\python export_camera.py <Data> --pack ..\content\pack --scene level1=Exploration
.venv\Scripts\python export_battle.py <Data> --pack ..\content\pack                # every battle room
.venv\Scripts\python export_units.py <Data> --pack ..\content\pack                 # unit models
.venv\Scripts\python fit_textures.py ..\content\pack\models	extures --max 1024   # mobile texture budget
.venv\Scripts\python build_manifest.py ..\content\pack
python ..\server\serve.py --pack ..\content\pack --bind <this PC's Tailscale IP> --port 8790
```

The phone build comes from the `build-ipa` workflow as an unsigned IPA. Sign and install it wirelessly with
[appman](https://github.com/collinsomniac/appman-ios). Set the repository variable `COGITATOR_SERVER` to your
pack server's URL, e.g. `http://<pc-name>:8790` using Tailscale MagicDNS.

## Rules for contributors

- Never commit converted output, game files, or decompiled game code. `.gitignore` blocks the usual suspects.
- Engine code here is original. Behaviour is reimplemented from observation and from the data in the user's
  copy, not copied from the game's assemblies.

Warhammer 40,000 and Mechanicus are trademarks of their respective owners. This project is unofficial and not
affiliated with Games Workshop, Bulwark Studios or Kasedo Games.
