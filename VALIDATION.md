# Validation of the source branch

Date: 2026-10-18. Base: upstream `975040b`.

| Check | Result |
| --- | --- |
| macOS Release build (`nxhost`, `nxfs`, `nxpatch`) | Passed, locked dependencies |
| iOS arm64 Release build with embedded local 4.0.1 modules and `NX_MISSING_MII=1` | Passed |
| `cargo test --release -p nxhost -p nxshader` | 9 passed; real-GPU test ignored by default |
| Explicit real-Metal `gpu_reads_and_writes_across_heap_chunks` | 1 passed |
| Static iOS display redirects and original-code continuation stubs | 3 sites passed |
| Offline iOS no-Mii null/non-null execution checks | 16 cases passed |
| Python helper syntax and `make-ios.sh` shell syntax | Passed |
| Local fallback-font helper | Passed; generated SFNT fits the shared-memory limit |
| Portable local Mac app packaging and ad-hoc signature verification | Passed |
| Packaged Mac settings window | Opened with Wireless, phone controllers and system-language settings; cancel exited |
| `git diff --check` and staged source/privacy inspection | Passed before push |

No private game bytes or generated app artifacts accompany these checks in Git.
Unicorn is used only for the offline instruction checks, not for running the game.

The earlier integrated version was installed and exercised on an Apple Silicon
Mac, iPhone 15 Pro and iPhone 13 Pro. The user confirmed widescreen proportions,
iOS English selection/persistence, phone control of Mac menus, and Mac/iPhone
LAN/Wireless room joining, race entry and vehicle movement synchronization.
A subsequent Wireless disconnect followed iPhone backgrounding/locking; keep
both clients foreground. These earlier device tests do not constitute a fresh
installation test of this publishing checkout's updated packaging scripts.

Full-length races, complete game coverage, router-free AWDL and a real Switch
peer have not been verified. Existing firmware-backed operation and every game
language were not separately retested in this source publishing pass.

## Mac Online/Server follow-up (2026-10-18)

- Mac Release build passed after adding the Online selection and Server field.
- The actual SwiftUI settings component was exercised in an isolated local probe:
  an empty address disables Start; a padded address is trimmed, persisted and
  passed as NX_NET=lanplay / NX_LANPLAY_SERVER; reopening restores it; selecting
  LAN removes the environment override while retaining the saved address.
- Both local packaging entry points reject --network lanplay without --server.
- Public LAN-Play server matchmaking/racing was not tested.

## Mac launcher settings parity (2026-10-18)

- Final Mac Release build passed. All three local app variants were regenerated
  with the new executable and their existing game/save configuration. Their
  ad-hoc signatures passed strict verification, code sections match the final
  build, and all three configurations passed the actual data/module path checks.
- `tools/verify_mac_launcher.swift`, compiled with the actual Mac launcher source,
  passed synthetic-data checks for persisted settings, unknown-key preservation,
  both directions of all new environment switches, stale relay/language removal,
  missing required files and modules, optional firmware/DLC, DLC removal, Unicode
  and space-containing file paths, restoration of bundle paths, and the JSON
  argument handoff consumed by the Rust app entry point.
- This check creates only disposable synthetic files and performs no networking:

```sh
xcrun swiftc -parse-as-library macos/Launcher.swift macos/WolverineUSB.swift tools/verify_mac_launcher.swift -o /tmp/verify-mac-launcher
/tmp/verify-mac-launcher
```

- The computer-use native connection failed during the fresh GUI check (native
  pipe closed, including after reconnect). Window rendering, native file-panel
  interaction and the denied-permission UI were not verified in this pass. No
  fresh game/race run is claimed for this launcher-only update.

## Razer Wolverine V3 TE 8K PC input (2026-10-19)

- Fresh USB enumeration confirmed 1532:0a5a and interface 0 ff/5d/01.
- The actual native IOUSBHost bridge captured live input: an initial run recorded
  132 changed states including right-stick motion; a second run captured 115
  changed states including Xbox A/B, left/right stick changes, 25 trigger pairs
  and a final neutral state. Disconnect/reconnect transitions were observed.
  The user confirmed the indicator lit after initialization. Opening the
  interface alone was not considered a successful input test.
- Mac Release build and 3 selected `wolverine::tests` passed, covering every
  mapped button bit, packet validation, stick direction/dead zones, trigger
  threshold and release, plus a captured-model A/RB/trigger combination.
- The existing synthetic-data Mac launcher checks passed with the USB source
  linked. They do not open a USB device or use game data.
- Fixed a discovered startup regression: Swift concurrency was referenced by an
  @rpath install name without an LC_RPATH. The Mac link now includes
  /usr/lib/swift. The final runtime and all three signed local app copies passed
  `--help` dynamic-loader startup checks; installed code matches the final build.
- Portable local app packaging passed with the new packaged-startup check.
  The standalone USB diagnostic compiled; its invalid-duration guard was checked
  without interrupting the running game.
- The real game log assigned the raw Razer controller to player 1 and continued
  rendering. The user confirmed both menu navigation and vehicle control on
  the final installed build.
- GUI automation again failed (native pipe closed/timeout), so the user opened
  the app manually. No agent-observed visual check is claimed.

Rumble, motion, an independent rear-button mapping, Guide/Home handling,
multiple simultaneous raw USB Wolverines and system-wide virtual-controller
registration are outside this input implementation. Complete physical testing
of every button and full-race endurance were not performed.
