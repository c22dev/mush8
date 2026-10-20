# Apple-device adaptations for MK8 Deluxe 4.0.1

This feature branch contains the source adaptations developed against Mush8's
initial commit (`1b2e9b8`), rebased onto `975040b` (firmware requirement checks).
It retains the upstream GPL-3.0 license and author notices.

## Changes

- Optional firmware-free operation: local SFNT shared-font fallback, missing
  system-data errors at the SDK API boundary, a constructed empty Mii avatar,
  and guards for missing Mii layouts, thumbnails and expressions in LAN lobbies.
- A version-checked local Player Mii parameter record for fresh account setup.
- Extracted base Program NCA input through `--base-nca`, alongside NSP input.
- Native aspect adaptation for world cameras and outermost UI panes, preserving
  character proportions; Mac windows can resize and enter fullscreen.
- GPU heap mappings split at Metal's `maxBufferLength`, with cross-chunk vertex,
  index, uniform and copy handling. Common one/two-chunk shader paths avoid
  runtime 64-bit division, and shader caches use a revised resource ABI.
- Focused Mac keyboard input, cleared when focus is lost.
- Persistent iOS game-language selection, including system language.
- An embedded Mac settings window for local app bundles with iOS game-setting
  parity: language, networking, phone pads, rumble, motion, console mode, pipeline
  prewarming and background shader compilation, plus file and memory status.

The missing-resource and camera hooks are specific to MK8 Deluxe **4.0.1**, main
build ID `158c0c7399aabd7bf83795c2ef3484c700000000000000000000000000000000`.
Build-ID and instruction checks reject an unexpected executable; do not reuse
these offsets for another game or game version.

## Firmware-free limits

No Mii model, face texture or Mii editor is supplied. The compatibility path
keeps an empty, constructed avatar and normal job completion, rather than
recreating Nintendo's resources. Mii graphics are absent. Local font substitution
can differ from the original game's typography and symbol coverage.

Normal upstream builds still use your own firmware. To use this branch without
it, provide a local font and, for iOS, explicitly enable the baked no-Mii profile.
A signed iOS no-Mii binary keeps that profile even if firmware is later imported;
rebuild without the profile to use real Mii rendering. This branch also keeps
upstream's non-empty-folder and directory-import fixes.

## Prepare a local fallback font

Use a font available on your own machine. The helper creates a local BMP subset;
it does not download or distribute fonts.

```sh
python3 -m venv .venv
.venv/bin/python -m pip install fonttools
.venv/bin/python tools/prepare_host_font.py
```

The default input is macOS's installed Arial Unicode font and the output is
`work/host-font.ttf`. If that font is unavailable, supply `--source /path/to/font.ttf`.
Keep the generated font and any app embedding it local.

## Mac

Prepare your own `prod.keys`, `base.nsp`, `upd4-0-1.nsp` and optional `dlc.nsp` in
a local game directory. From this repository, extract and rewrite the modules
using upstream `nxfs` and `nxpatch`, then create a local app:

```sh
cargo build --locked --release
# Run these from the game directory (substitute your checkout path):
/path/to/mush8/target/release/nxfs extract --base base.nsp --update upd4-0-1.nsp --out work/v-upd
/path/to/mush8/target/release/nxpatch rewrite --out work/v-upd/patched work/v-upd/exefs/main work/v-upd/exefs/subsdk0 work/v-upd/exefs/sdk
# Return to the repository:
python3 tools/make_local_app.py --game-dir /path/to/game \
  --patched /path/to/game/work/v-upd/patched \
  --npdm /path/to/game/work/v-upd/exefs/main.npdm --network wireless
```

This creates `build/MK8 Native.app`. It stores only the compiled host and private
path configuration in that local bundle. The original game/key files stay at
the supplied locations. The app starts with an embedded settings window; choose
settings and start the game. `--network off` and `--network lan` are also supported.
Use `--base-nca /path/to/base-program.nca` when you already extracted that NCA;
this does not itself extract an XCI. The embedded settings window defaults phone
controllers on. Upstream `make-app.sh` and its original launcher remain available
for the normal firmware-backed setup.

### Mac launcher settings and files

The embedded launcher exposes the iOS game settings on Mac. Changes save
immediately, including when the window is closed without starting a game.

| Setting | Mac behavior |
| --- | --- |
| Game language | System language or the same language choices as iOS |
| Multiplayer | Off, LAN, Online/Server and Wireless |
| iPhone controllers | Hosts the existing phone-pad transport |
| Vibration / motion controls | Uses the existing runtime support; requires compatible controller hardware |
| Console mode | Docked 1080p or Handheld 720p; native aspect adaptation remains active |
| Prewarm pipelines | Builds recorded, available pipelines on utility workers; existing local defaults are retained |
| Background shader compiling | Asynchronous compilation when enabled, synchronous waiting when disabled |
| Game files | Required/optional status, choose existing files or firmware directory, remove DLC, restore original paths |
| System status | Available-memory estimate refreshed every two seconds, data/save folder and log shortcuts |
| Local network permission | Shows a denied-access message and a shortcut to macOS privacy settings |

Mac file selection references the selected files in place instead of copying
large archives. Multiple DLC NSPs are supported. Selections persist separately
for each application bundle. The Rust host receives the selected paths before
initializing game files. Missing/empty/unreadable required files or rewritten
modules disable Start; a selected missing DLC must be replaced or removed.
Firmware remains optional for this branch's compatibility path. Changing the
update does not regenerate rewritten modules: use the same 4.0.1 game build.

The settings content scrolls and the Start button stays at the bottom of the
resizable window. Mac defaults retain the previous local behavior (Docked,
prewarm off unless explicitly configured), rather than changing existing saved
preferences. Disabling prewarm or enabling background compilation removes the
corresponding presence-based environment flag; a string value of `0` would
incorrectly leave those features enabled.

As requested for the local integration, Mac does not gain an outgoing
"use this Mac as another Mac's controller" mode. Its existing ability to receive
phone controllers is retained.

### Razer Wolverine V3 TE 8K PC on Mac

The local Mac runtime includes a native IOUSBHost input route for USB
`1532:0a5a`, interface 0 with class/subclass/protocol `ff/5d/01`. It references
only Apple's system frameworks; it does not install a driver or depend on a
Homebrew USB library. The launcher shows connection/readiness status.

Connect the controller before or after launching. The route continuously queues
interrupt reads, validates 20-byte XUSB state packets, and joins the first free
player slot alongside existing GameController and iPhone pads. Unplugging clears
its state and frees the slot. Existing OS-recognized Wolverine input is preferred.
One exact-model raw USB controller is supported at a time.

The physical Xbox layout matches the runtime's existing Xbox controllers:
right-side **B** confirms as Switch A, bottom **A** cancels as Switch B; Y/X,
LB/RB, LT/RT, Menu/View, stick clicks and both analog sticks are mapped.
Sticks have radial dead zones and triggers use the normal digital threshold.
Guide has no guest Home-menu action. Rear buttons follow firmware mappings and
are not exposed as extra independent guest buttons. Raw USB rumble and motion
are not implemented. This is support inside this runtime, not system-wide
GameController or SDL registration. Other Razer models are not admitted.

For a standalone transport check without game data, close any game/probe that
already owns the controller, then build and run:

```sh
xcrun swiftc -parse-as-library macos/WolverineUSB.swift tools/verify_wolverine_usb.swift -o /tmp/verify-wolverine-usb
/tmp/verify-wolverine-usb 30
```

Operate the buttons, sticks and triggers during that window. The tool prints
readiness and changed XUSB snapshots. A successful open or LED alone does not
verify live input.

The IOUSBHost registry/API adaptation is attributed in
`THIRD-PARTY-NOTICES.md`; the source project and installed signed driver are left
unchanged. Current build and physical-input test coverage is in `VALIDATION.md`.

## iOS

Follow upstream prerequisites and prepare the game files in the repository's
local game-data directory as expected by `make-ios.sh`. Generate the font above,
then build with your own signing configuration:

```sh
FIRMWARE_FREE=1 TEAM=YOUR_TEAM_ID BUNDLE_ID=your.own.bundle.id ./make-ios.sh --install
```

`FIRMWARE_FREE=1` sets `NX_MISSING_MII=1` before compiling the iOS executable and
bundles `work/host-font.ttf` under `fallback/`. `NX_FALLBACK_FONT` can select another
local font. The game instructions and all no-Mii/camera redirects are baked before
signing; no executable-memory writes are performed on iOS. The generated signed
app contains your game-derived executable sections: keep it local, outside Git.

The iOS launcher accepts `base.nca` or `base.nsp`. Update installs retain the
application's game files and configuration. Choose **Game language** on the
launcher before Play; this changes the game language, not all launcher labels.

## Phone controller and Wireless

For a phone controller, start the Mac game with phone controllers enabled and
choose **Use as a controller for a Mac** on the iPhone launcher. Use the same
Wi-Fi. A phone uses the first available player slot; keyboard input still merges
into player one. Controller mode does not require game data on the phone.

For independent multiplayer, each device needs game data. Set both launchers to
Wireless (or both to LAN) and use the corresponding network entry in the game's
main menu. Keep the iPhone app foreground and the screen awake. iOS backgrounding
pauses the guest and can cause a connection timeout; rejoin after a disconnection.

Local testing confirmed Mac/iPhone room joins, race entry and vehicle movement
synchronization on the same Wi-Fi. Router-free AWDL, Switch interoperability and
full-length race endurance were not verified. These are limited smoke tests,
not a claim of complete game compatibility.

### Online / Server on Mac

In the embedded Mac settings window, choose **Online · Server** and enter the
same LAN-Play server host/IP and optional port as the iPhone's Online mode.
The address is trimmed and saved when you start the game. Empty addresses disable
Start; changing to LAN/Wireless/off removes the server override for that launch.
A server entered previously remains available when you select Online again.
The existing LAN-Play transport uses port 11451 when omitted and supports servers
without a login requirement. Use the corresponding Online entry in the game.

The portable packager also accepts:

```sh
python3 tools/make_local_app.py --game-dir /path/to/game --network lanplay --server example.org:11451
```

The settings, persistence and launch environment have been checked locally;
public-server matchmaking/racing was not tested in this change.

## Validation

The original integrated build was exercised on a Mac, iPhone 15 Pro and iPhone
13 Pro. Source-branch build and automated verification are recorded separately
in `VALIDATION.md`.

```sh
cargo test --locked --release -p nxhost -p nxshader
cargo test --locked --release -p nxhost gpu_reads_and_writes_across_heap_chunks -- --ignored --exact nvn::draw::state::segmented_heap_tests::gpu_reads_and_writes_across_heap_chunks
# Local iOS executable only; requires Python unicorn for the second command:
python3 tools/verify_display_hooks.py /path/to/ios/nxhost --report /tmp/display-check.json
python3 tools/verify_lan_avatar_guards.py /path/to/ios/nxhost --report /tmp/avatar-check.json
```

No game archives, keys, firmware, extracted modules, fonts, shader caches, signed
apps, provisioning profiles, private logs or machine-specific launch.json files
are included in this branch.
