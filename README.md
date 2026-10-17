# Cogitator

A bring-your-own-copy engine for **Warhammer 40,000: Mechanicus** on iPhone and iPad, written in Rust with Bevy.

This repository contains **no game content and no code from the game**. You need your own copy of
Mechanicus (Steam). A converter on your PC reads that installation and produces a content pack; your PC serves
the pack to your phone over Tailscale; the app downloads it once and plays from it.

```
your Steam install ──► converter (PC) ──► content/pack ──► serve.py (Tailscale only) ──► Cogitator.app (iOS)
```

## Status

Early bring-up. Working today:

- `converter/inventory.py`: scans every serialized file in the install (395k GameObjects, 1,040 script classes).
- `converter/odin.py`: a reader for the Odin Serializer binary format (OdinSerializer is Apache-2.0), used by
  the game's rules data. Decodes 1,701 of 1,701 blobs in the main data files.
- `converter/export_data.py`: exports skills, equipment, Tech-Priests, dungeon events, rooms, blueprints and
  dialogue to JSON.
- `app/`: syncs the pack over Tailscale (manifest + SHA-256, incremental) and reports what arrived.

## Use

On the PC (Python 3.12, run once per game update):

```
cd converter
python -m venv .venv && .venv\Scripts\pip install UnityPy TypeTreeGeneratorAPI
.venv\Scripts\python export_data.py "<Steam>\steamapps\common\Mechanicus\Mechanicus_Data" --pack ..\content\pack
.venv\Scripts\python build_manifest.py ..\content\pack
python ..\server\serve.py --pack ..\content\pack --bind <this PC's Tailscale IP> --port 8790
```

The phone build comes from the `build-ipa` workflow as an unsigned IPA. Sign it with a free Apple ID (keep the
`increased-memory-limit` entitlement) and install it. Set the repository variable `COGITATOR_SERVER` to your
pack server's URL, e.g. `http://<pc-name>:8790` using Tailscale MagicDNS.

## Rules for contributors

- Never commit converted output, game files, or decompiled game code. `.gitignore` blocks the usual suspects.
- Engine code here is original. Behaviour is reimplemented from observation and from the data in the user's
  copy, not copied from the game's assemblies.

Warhammer 40,000 and Mechanicus are trademarks of their respective owners. This project is unofficial and not
affiliated with Games Workshop, Bulwark Studios or Kasedo Games.
