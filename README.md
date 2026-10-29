# Mush8
A native Mach-O iOS/macOS port of Mario Kart 8 Deluxe, with LAN, online and wireless multiplayer

## How does this work ?
the Switch and Apple Silicon both run arm64, so the game's code can almost run as is. a few instructions can't run outside a Switch (the x18 register, system calls, system registers), so this project rewrites them with a tool called  `nxpatch`
the rest, most importance peace is `nxhost` : it plays the role of the Switch's OS (Horizon) for the game, its services (files, saves, controllers, audio, network...) and its graphics API (NVN), which is turned into Metal. The game's shaders are translated to Metal ones and compiled ahead of time too

On macOS, the patched game code is loaded when you launch it. On iOS, and because we don't have JIT, we can't afforc to do that : game code is linked into the app when you build it

On top of that, Mush8 adds LAN Play over the internet ([lan-play.com](https://www.lan-play.com) servers) and a wireless mode for nearby devices

## Game files
Because Nintendo ninjas are always watching, you will need your own dump :
- `prod.keys`
- the base game (`.nsp`)
- update 4.0.1 or 3.0.5 (`.nsp`). To play LAN with a Switch, use the same update as the Switch
- a `Firmware` folder dumped from your Switch (fonts, Miis)
- optionally, the Booster Course Pass, named `dlc.nsp`

Mush8 will then asks for them : on macOS pick the folder holding them in the launcher, on iOS import them from the app (or copy them in Files → On My iPhone → Mush8)

## Requirements
- Apple Silicon Mac, macOS 14 or later
- Rust toolchain
- Xcode 26 or later
- recommended : the Metal Toolchain, which Xcode 26 downloads separately (`xcodebuild -downloadComponent MetalToolchain`). Without it, Prepare can't compile the game's shaders ahead of time : the game still runs, but compiles them while you play, so new scenes (a race start for example) show black or missing graphics for a moment

For iOS, also :
- `rustup` with `rustup toolchain install 1.98.1` and `rustup target add aarch64-apple-ios --toolchain 1.98.1`
- `xcodegen` (`brew install xcodegen`)
- an Apple ID to sign the app on iOS (a free one works), or AltStore / SideStore to install the `.ipa`
- a device on iOS/macOS 26 or later, with 6 GB of RAM or more (8 GB is comfy)

## Building
the easy way :
```
./build.sh
```
The script (should) installs what's missing (Homebrew, Rust...), then asks whether you want the Mac or the iOS version and walks you through the rest. The Mac `.app` ends up in `~/Downloads/Mush8Output`, the iOS one goes straight to your plugged-in iPhone / iPad

By hand, macOS :
```
./make-app.sh
```
gives `build/Mush8.app`. Open it, choose your game files folder and hit Prepare (the first time, it extracts the game and compiles its shaders, a few minutes)

iOS :
```
./make-ios.sh --install
```
The signing team is taken from the account signed into Xcode (Settings > Accounts), `TEAM=<your team id>` overrides it, `BUNDLE_ID` sets your own app id, `FREE=1` signs with the entitlements a free account can get. The first build compiles every shader for iOS, so it takes a while

Or make an `.ipa` to install with AltStore / SideStore :
```
./make-ios.sh --ipa
```
it ends up in `build/Mush8.ipa`

Tests :
```
cargo test --workspace
```
They need no game files and no controller

## Multiplayer
Pick a mode in the launcher, under Multiplayer :
- LAN : players on the same network, a Switch can join too
- Online : any lan-play.com server (`host:port`), Switches on that server included
- Wireless : nearby Mush8 players

## Controls
Any controller normally works (Switch Pro, Joy-Con, DualSense, Xbox...). On macOS the keyboard works too (WASD stick, arrows d-pad, X/Return A, Z/Backspace B, Q/E L/R, 1/3 ZL/ZR, Tab +), on iOS there are touch controls when no controller is connected

An iPhone can be a controller too : open Mush8 on it and tap "Use as a controller". It finds the other Mush8 instance on the same Wi-Fi and joins as the next player

Any browser can be a controller too : turn "Remote controller" and "Browser controllers" on in the launcher and the address shown on the other device

On macOS, if your controller or a part of it (for example rumble) does not work, set "Gamepad driver" to SDL in the launcher

## Mods
Mush8 loads mods from `mods` inside the selected game files folder on macOS, or the app's Documents folder on iOS. Open Game data → Mods → Customize on macOS, or the launcher's Mods section on iOS. Mods load in name order; later mods win file conflicts.

Use Browse GameBanana to search Mario Kart 8 Deluxe mods by name, section, URL or ID, then choose a file to download and install. The app supports ZIP, 7z and RAR, displays download progress, and supports cancellation. Existing mods are not overwritten. Downloads are limited to 512 MiB and extraction to 2 GiB / 20,000 entries. The flow was informed by [SimpleModDownloader](https://github.com/PoloNX/SimpleModDownloader).

Each mod can contain `romfs` / `exefs` directly or use `atmosphere/contents/0100152000022000/`, `contents/0100152000022000/`, or `0100152000022000/`. IPS/pchtxt patches in `atmosphere/exefs_patches/<patchset>/` or `exefs_patches/<patchset>/` share that mod's toggle. A top-level `atmosphere` folder is loaded last with its own toggle.

On iOS, swipe a mod for file inspection or deletion; long-press also opens its context menu. On macOS, use the context menu. Deletion requires confirmation. The shared top-level `atmosphere` folder is read-only here because it may contain other games' files.

iOS supports RomFS-only mods; code patches require rebuilding on a Mac. On macOS, run Prepare after changing code patches, which must match the game's NSO build ID. ARM32 code, replacement ExeFS modules, Skyline NRO plugins, packed `exefs.nsp` / `romfs.bin`, and `.stub` removal are unsupported. Use mods compatible with your game update; a supported folder layout alone does not establish gameplay compatibility.

```
mods/
  Peter Griffin/
    romfs/
      Driver/Ludwig.szs
      UI/cmn/menu.sarc/mn_L_CharaIcon_00.szs/timg/__Combined.bntx/tc_Chara_Ludwig.png
      UI/EUen/message.sarc/Common.msbt/1029.txt
  Faster Items/
    exefs/
      158C0C7399AABD7BF83795C2EF3484C7.ips
  Course Mod/
    atmosphere/contents/0100152000022000/romfs/
      Course/Gu_FirstCircuit/course_model.szs
```

## Limitations
- free Apple IDs and the `.ipa` (AltStore / SideStore) don't get the extended virtual addressing entitlement, so the game runs with a smaller 5 GB memory layout
- Nintendo's online play isnt available
- 5 to 8 players split screen is Switch 2 only
- no amiibo (yet)

## Credits
Thanks to everyone who contributed :
- [codedpalette](https://github.com/codedpalette) : a bunch of fixes and improvements
- [hchzr](https://github.com/hchzr) : 4K and 60 fps split screen
- [lyccccy](https://github.com/lyccccy) : GPU heap split, so iPhones with less memory can render
- [FritteHD](https://github.com/FritteHD) : browser phone pad
- [justr4mzi](https://github.com/justr4mzi) : kernel signal, Mii and video decoder fixes
- [184904](https://github.com/184904) : null index buffer fix

Contributions are welcome !

## Third party
- [libvpx](https://chromium.googlesource.com/webm/libvpx) v1.17.0 VP8 decoder (BSD 3-Clause, see `crates/vpx/libvpx/LICENSE` and `PATENTS`) : WebM movies
