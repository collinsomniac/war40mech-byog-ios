# How this port is built

## The split

- **The player's PC** only converts and serves. The converter reads the player's own install and writes a
  content pack (glTF scenes and models, textures, JSON data, localisation, fonts, battle grids). `serve.py`
  serves it over Tailscale. No game logic or rendering runs on the PC.
- **The phone** does everything at play time: rules, AI, rendering, UI, saves. It syncs the pack once
  (battle rooms on demand) and plays offline after that.
- **This repository** holds only original code: the Rust app and the Python converter. No game files,
  converted output or decompiled code is ever committed.

What goes in the pack versus what runs on the device:

| Pack (precomputed on the PC, as data) | Device (computed at play time) |
|---|---|
| Geometry, materials, textures (capped at 1024 px), animations | Rendering, animation playback, camera |
| Every data record (missions, events, skills, units, rooms…) as JSON | All rules: tomb generation, events, rewards, battles, AI, Ark |
| Battle grids (the game ray-casts them at load; we bake the same scan) | Pathfinding, targeting, line of fire |
| Behaviour trees (the game's own JSON) and enum tables (from the player's DLL) | The tree interpreter |
| UI layouts (RectTransforms, sprites, text terms) | Layout, input, localisation |

The rule of thumb: anything the game derives from scene data at load time and never changes again (grids,
spawn markers, static batching) is baked in the converter; anything that depends on the campaign or the
player runs on the device.

## Reading the game

The decompiled assemblies stay git-ignored on the PC and are read as a specification. Each ported rule
names the class it follows (for example `TaskExploSetupEvents`, `RandomSpawnDeployment.GetEntity`) so it can
be checked again. The aim is the game's behaviour, not its structure:

- The game's coroutine "tasks" become plain functions over state (`rules::*`); its timeline of
  presentation "steps" becomes Bevy systems that animate what the rules report.
- Data-driven systems are run from data: event dialog graphs, skill target infos, spawn deployments and
  Behavior Designer trees are interpreted, not re-authored.
- Where a system isn't ported yet, the game says so in-game ("Not yet ported: …") and the gap is counted,
  rather than being faked.

## Checking

- `app/tests/game.rs` is the one test file. It runs against the real converted pack: New Game into the
  tutorial tomb, a tutorial battle with the trees, a campaign room's spawns, the menus, the manifest.
- `COGITATOR_AUTOPLAY=1` plays the game through its own HUD actions; with `COGITATOR_SNAPSHOT` it captures
  later screens on the desktop. Run desktop checks through `tools/snapshot.sh` (800x600, vsync, memory
  watchdog): the GPU is shared with the player's desktop.
- Visuals are compared with the game itself at the same resolution.

## Mobile choices

- Point and spot lights don't cast shadows (battle rooms hold dozens); only the sun does.
- Textures are capped at 1024 px in the pack.
- Unity effect shaders (portal cards, light shafts) are hidden or drawn additive until they're ported.
