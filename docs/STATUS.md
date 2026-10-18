# Port status

What runs, what is partial, and what is a stand-in. "Faithful" means it follows the game's own logic
(checked against the player's copy); "approximate" means it plays the same role but differs in detail;
"placeholder" means a stand-in that only keeps the flow moving.

Updated 2026-10-18.

## Game flow

| Piece | State | Notes |
|---|---|---|
| Boot, pack sync over Tailscale | done | Incremental (manifest + SHA-256, local checksum index); battle rooms fetched on demand; offline play from the last pack. |
| Main menu (converted Unity UI) | done | Layout, fonts, localisation, windows. Continue appears when a save exists. Settings/Load/Credits windows open but their contents are unported. |
| New Game | faithful | `GameNewSaveState.NewGame`: default settings, starting tech-priests, canticle, blueprints, troops, ship skills from MechanicusLibrary. Difficulty/extra settings windows not yet editable. |
| Intro cutscene | placeholder | Skipped with a parity report; its end (TutoLauncher.LaunchDeploy) is ported, so the tutorial starts as in the game. |
| Save / Continue | done | `saves/campaign.json` beside the pack; written after every meaningful step. Mid-battle saves resume at the tomb. |

## Exploration (tomb)

| Piece | State | Notes |
|---|---|---|
| Tomb generation | faithful | Cell scatter and push-apart, Delaunay + Prim's, one corridor per room side, n/2 loops, straight-line missions, room counts, Unity-style seeded RNG (tutorial seed 452). Exact Unity float sequences not guaranteed. |
| Event placement | faithful | Start room (farthest from centroid), objective events by depth range, custom events, auto fights, glyph events with glyph rolls, opened-door events. |
| Event dialogs | faithful rules / own UI | Dialog graph, choices, ChangeDialog/End/Cancel/Fight/Door, every reward type with GameplayConfig ranges. Drawn by our HUD, not the converted `UIExploEvent` panel (illustrations and icons not shown yet). |
| Awakening gauge | faithful | Per-room cost from settings, levels every 5; level-up popup and dungeon modifiers not yet applied. |
| Map visuals | approximate | Rooms, corridors and markers are our own holo tiles on the converted holo-table, not the game's room-shape meshes (MapLibrary decode fails). |
| Canticles in exploration | missing | |

## Battle

| Piece | State | Notes |
|---|---|---|
| Battle rooms | done | All 84 battle scenes converted: static 3D, template props, entities, deploy areas, enemy spawn markers. |
| Walkable grid | faithful | GridManager's scan reproduced against the scene colliders (Ground/Hole/NotWalkable layers), heights, 0.6 climb, no corner cutting. |
| Enemy spawning | faithful | TaskBattleSpawnEnemies + RandomSpawnDeployment.GetEntity + deployment capacity (story/random, template bonus, awakening, extra-Necron setting). |
| Turn order | faithful (core) | Initiative timeline; Necron activity / reinforcement / delay entries not yet. |
| Movement | faithful | Move skill: range per use, extra range bought with cognition per the extra-movement setting, 3D step distances. Opportunity attacks missing. |
| Skills (damage) | approximate | Range/targets/damage/cost/cooldown/uses-per-turn from data; hit = dodge, critical = max, armour pools. Buffs, machine spirit, per-skill task behaviours (pushback, overload, heal specials) not yet. |
| Necron AI | faithful framework | The game's Behavior Designer trees run by our interpreter; ~25 leaf types ported (all the common ones). Lord/boss-specific leaves fail cleanly and are reported. |
| Reanimation protocols | approximate | Knock-out with countdown, body destroyed if hit; countdown fixed at 2 (data-driven value pending). |
| Archives / cognition | approximate | Collected by walking adjacent; real collection uses a skill/interaction. |
| Sarcophagi, traps, generators, turrets, barriers, platforms | placeholder | Present as scenery; their spawns/behaviour not yet. |
| Troop deployment during battle | missing | Troops start deployed. |
| Unit models | done | Skinned/rigid unit prefabs with idle animation; tech-priests use the customisable body with default parts. Attack/move/death clips not played yet. |
| Battle camera | done | Drag to pan, pinch/wheel to zoom. |

## Ark

| Piece | State | Notes |
|---|---|---|
| Missions on offer | faithful | IsMissionAvailable (done, countdown, required missions, sector boss after 3). |
| Deployment | approximate | Tech-priests + troops by capacity; canticle choice and per-troop costs per data pending. |
| Armoury / equip | approximate | Buy a blueprint's equipment into its first free slot; no skill tree, augments, ship skills. |
| Story and dialogues on the Ark | missing | |
| Ark UI | own UI | The converted ArkUI isn't used yet; the Ark 3D interior is. |

## Presentation

| Piece | State |
|---|---|
| Unity scenes → glTF (meshes, materials, lights, cameras, static batching, skinning, animation) | done |
| Unity shader look (fog, post-processing, custom terrain/FX shaders) | approximate: PBR + emissive; FX additive or hidden |
| Audio (FMOD banks) | missing |
| VFX (particles) | missing |

## Known issues

- Device loss on the desktop GPU while C: was full (fixed by freeing disk). Desktop checks now run under
  `tools/snapshot.sh`, which kills the app above 5 GB VRAM / 6 GB RAM.
- Mission giver portraits, event illustrations and skill icons aren't drawn.
