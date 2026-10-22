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

Mush8 will then asks for them : on macOS pick the folder holding them in the launcher, on iOS import them from the app (or copy them in Files → On My iPhone → Mush8). File names don't matter, except for the Booster Course Pass : the macOS launcher remembers the last names it found, so name it `dlc.nsp` to be sure it is found

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
The easy way :
```
./build.sh
```
It installs what's missing (Homebrew, Rust...), then asks whether you want the Mac or the iOS version and walks you through the rest. The Mac `.app` ends up in `~/Downloads/Mush8Output`, the iOS one goes straight to your plugged-in iPhone / iPad

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

Or make an `.ipa` to install with AltStore / SideStore (no signing needed on the Mac) :
```
./make-ios.sh --ipa
```
it ends up in `build/Mush8.ipa`

Tests :
```
cargo test --workspace
```
They need no game files and no controller

## Graphics
On a Mac, the launcher's Settings go past what the Switch draws :
- Console mode : Docked renders at 1080p, 1440p or 4K (MK8D stops at 1080p, Mush8 patches its output size and memory budgets)
- Supersampling : when the console mode is bigger than the screen (4K on a 1440p display), the image is filtered down to the screen for smooth edges
- Full resolution 3D : the game draws its 3D scene at 75% of the output and scales it up, this draws it at 100%
- High quality shadows : shadow maps at twice the size
- 120 fps : MetalFX makes an in-between image for every frame (macOS 26, set the display to 120 Hz). The game still runs at 60, so controls don't get faster. Cmd+I switches it off and on while playing
- Full frame rate split screen : with 3 or 4 players the game draws each screen every other frame, this draws them all every frame
- Performance HUD : Metal's frame rate and GPU time over the game

`tools/race-bench.sh` boots straight into a race and prints how fast it renders

## Multiplayer
Pick a mode in the launcher, under Multiplayer :
- LAN : players on the same network, a Switch can join too
- Online : any lan-play.com server (`host:port`), Switches on that server included
- Wireless : nearby Mush8 players, no router needed (keep Wi-Fi on)

In the game, the main menu then shows LAN Play, Online Play or Wireless Play

## Controls
Any controller works (Switch Pro, Joy-Con, DualSense, Xbox...), each one is a player. On macOS the keyboard works too (WASD stick, arrows d-pad, X/Return A, Z/Backspace B, Q/E L/R, 1/3 ZL/ZR, Tab +), on iOS there are touch controls when no controller is connected

The touchscreen works too : tap the game on iOS, click it on macOS

Motion controls (gyro steering) are off by default, turn them on in the launcher

An iPhone can be a controller too : open Mush8 on it and tap "Use as a controller for a Mac" (no game files needed). It finds the Mac running Mush8 on the same Wi-Fi and joins as the next player with its touch controls (and its motion, if Motion controls is on in the Mac launcher). Turn "iPhone controllers" on in the Mac launcher (it is by default), or on an iPad playing the game

On macOS, if your controller or a part of it (for example rumble) does not work, set "Gamepad driver" to SDL in the launcher

Any phone's browser can be a controller too, no app needed (Android included) : turn "Browser controllers" on in the Mac launcher (it is off by default) and open one of the two addresses shown there on the phone (same Wi-Fi). Each phone joins as the next player. Hold the phone sideways and pick a layout at the top :
- Normal : every button, for the menus
- Drive : steering, item, drift and a small A for the start, made for the game's auto-accelerate. The stick stays in place (Fixed) or appears under your thumb (Floating)

Add the page to the home screen (Share → Add to Home Screen) to get rid of the address bar

## Mods
Mush8 loads mods from a `mods` folder in the same folder as your game files. This works on macOS only for now. Each mod is a folder inside `mods`, and it must contain a `romfs` folder, an `exefs` folder, or both. Mush8 ignores other folders. In the launcher, use Game data → Mods → Customize to turn each mod on or off

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
```

`romfs` holds game files. A file replaces the game file at the same path. Mods apply from the top of the list to the bottom, so a lower mod wins when two mods replace the same file

`exefs` holds code patches, as `.ips` or `.pchtxt` files. Mush8 applies them when you Prepare, so Prepare again after you change them. A code patch works only for one game update: Mush8 skips patches that are not for the update you use (3.0.5 or 4.0.1). Mush8 does not load replacement code modules (`main`, `subsdk0`...)

If a code patch changes the same code as one of Mush8's own patches, the mod wins for the menus (main menu, 5-8 players row, ghost menu, ghost upload popup) and 60 fps split screen. Patches that change Mush8's 4K or Fill screen code are refused, turn the mod off to Prepare

### Edit files inside archives
Some mods change only a part of a large game archive, for example one character icon in `UI/cmn/menu.sarc`. Make a folder with the name of the archive, and put the changed part inside it. Mush8 then changes only that part, so many mods can edit the same archive. These archives can be edited:
- `.sarc` and `.szs` archives: a file or folder inside them replaces or edits the inner file with the same name
- `.bntx` texture packs: `<texture name>.png` replaces that texture. The image must have the same size as the texture
- `.msbt` text files: `<label>.txt` replaces the text with that label. For example, label `1029` in `Common.msbt` is Ludwig's name

If a mod replaces a whole archive, edits from mods lower in the list apply on top of its copy

### Convert a mod made for Atmosphère or an emulator
1. Make a folder for the mod in `mods`, for example `mods/My Mod`
2. Find the `romfs` folder in the download. It is often at `atmosphere/contents/0100152000022000/romfs`. Move it to `mods/My Mod/romfs`
3. If the download has game folders such as `Driver`, `Kart` or `UI` at its top, with no `romfs` folder, move them into `mods/My Mod/romfs`
4. Move the `.ips` and `.pchtxt` files to `mods/My Mod/exefs`. They are often in `exefs` or in `atmosphere/exefs_patches/<name>`
5. If the readme tells you to put images into an archive with Switch Toolbox, make the archive path as folders instead. For example, "menu.sarc → mn_L_CharaIcon_00.szs → timg → __Combined.bntx: tc_Chara" becomes `romfs/UI/cmn/menu.sarc/mn_L_CharaIcon_00.szs/timg/__Combined.bntx/tc_Chara_Ludwig.png`
6. If the readme tells you to change a name in `message.sarc`, write the new name in `romfs/UI/<language>/message.sarc/Common.msbt/<label>.txt`, for each language you play in

Mods that change tracks, characters or items can break LAN and online play with players who do not have the same mods

## Limitations
- free Apple IDs and the `.ipa` (AltStore / SideStore) don't get the extended virtual addressing entitlement, so the game runs with a smaller 5 GB memory layout there (a paid account gets the full one)
- Nintendo's online play is gone, use Online (lan-play) instead
- 5 to 8 players split screen is Switch 2 only, so it's hidden
- no amiibo (yet)
- the two WebM videos (credits, Switch 2 dialog) play as a slideshow on iOS

Contributions are welcome !
