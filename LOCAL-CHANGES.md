# Local integration changes

Developed against Mush8 commit `1b2e9b875f3beb94802624f055e7a041a473ee3c`,
then prepared on top of upstream `975040b` for this source-only feature branch.
Copyright and GPL-3.0 license remain in LICENSE and individual source files.
The original checkout is not edited by this integration.

* Accept an extracted base Program NCA through `--base-nca`, preserving native
  patched RomFS reads against the original update NSP and optional DLC NSP.
* Handle absent system-data queries at the SDK public filesystem boundary.
  This SDK otherwise aborts internally before the application receives an error.
* Add a local macOS SFNT shared-font fallback, sharing a single backing range
  among missing font slots. Firmware fonts take precedence when available.
  The generated font is local working data and is not distributed in this source.
* Add a build-ID-checked compatibility profile for missing MiiModel resources.
  Empty avatar objects preserve the game's constructor and completion protocol;
  unavailable thumbnail callbacks are guarded. This does not restore Mii models.
* Expose one SDK-validated local `Player` Mii parameter record to the fresh
  account database, so first-run selection does not require the firmware editor.
* Guard the second missing-avatar thumbnail callback used during LAN setup.
* Handle the missing GSysMii/GSysMiiFaceOnly layout, optional avatar transforms,
  expression changes and expression-state queries in the LAN lobby. Apply only
  to the verified 4.0.1 build, checking each overwritten instruction. Check the
  baked iOS null and non-null branches offline (16 execution cases).
* Bake the missing-Mii profile into the signed iOS Mach-O at build time; avoid
  modifying executable memory on the phone. The game code stays in local work/.
* Accept base.nca in the iOS launcher and initialize its LAN configuration and
  bundled local font fallback before displaying the launcher.
* Read macOS app-bundle launch configuration directly in nxhost and also accept
  focused AppKit keyboard events. Clear those keys when the game loses focus;
  merge them with GameController input without counting an axis twice.
* Split GPU mappings of the guest heap at Metal's maxBufferLength, instead of
  silently rendering from a zero buffer when a single large mapping fails on
  A15. Update the shader resource table, resource residency, index/blit ranges
  and cross-chunk vertex/uniform staging; translate each global-memory word
  separately so vector accesses can cross a chunk boundary.
* Version the AOT shader-cache directory for the segmented-heap resource ABI.
  Validate range edges/overflow, real Metal cross-chunk reads and writes, and
  rendering on a Mac forced to use four 1 GiB chunks.
* Avoid runtime 64-bit division in shader heap lookups for the common one- and
  two-buffer mappings. Invalidate old prelude caches with heap-segments-v2;
  repeat the real Metal cross-chunk read/write test after this change.

The surrounding project includes an earlier MIT d2r-mobile-derived C diagnostic
host. That host and this GPL Rust runtime have distinct build and validation
paths. The Rust host executes the game's ARM64 instructions directly and
translates NVN GPU calls and Maxwell shaders to Metal; Unicorn is only used by
the offline instruction verification tool.

* Add a build-ID/instruction-checked MK8DX 4.0.1 native aspect profile.
  Adjust world-camera projections and outermost UI layout separately, excluding
  the menu compositor camera to prevent double correction. Cover identified
  full-size backgrounds; retain a centered HUD safe area and original render
  texture resolution. Bake all iOS host redirects before signing.
* Remove the Mac fixed content-aspect constraint, observe actual Metal surface
  size on both platforms, and keep touch coordinates aligned with the UI.
  Validate wide-window menus/race HUD and a 16:9 full-screen return on Mac;
  both iPhone 15 Pro and 13 Pro updates were confirmed by the user.

## Local launcher settings (2026-10-17)

* Add a persisted iOS game-language picker, including system language. Parse the
  JSON configuration with serde_json and apply NX_LANGUAGE before set:sys caches
  its value; system selection removes inherited language overrides.
* Embed a macOS SwiftUI settings window in nxhost before guest initialization.
  Keep the app's nxhost executable identity and existing launch.json data paths.
* Expose off/LAN/Wireless and phone-pad hosting in the local Mac app packager;
  phone-pad hosting defaults on. Declare both phone-pad and multiplayer Bonjour
  services in the Mac bundle. Preserve upstream phone and wireless protocols.

## Publishing integration

* Keep the upstream non-empty firmware directory check and directory import fix,
  while allowing the explicit firmware-free compatibility profile.
* Add portable local font preparation, Mac app packaging and static iOS hook
  verification tools. Generated resources and personal paths are not committed.
* Wire FIRMWARE_FREE=1 into the iOS build and bundle its local fallback font.
  Keep the baked empty-avatar factory initialized if firmware is imported later.

* Add Online/Server selection to the embedded Mac settings, persist the relay
  address, reject empty addresses, and pass NX_LANPLAY_SERVER only in LAN-Play
  mode. Expose --network lanplay --server in the portable Mac app packager.

* Bring the embedded Mac launcher to iOS game-setting parity: prewarm, background
  compilation, rumble, motion and console mode; persist edits immediately and
  clear presence-based flags when disabled. Add a scrollable/resizable layout,
  required/optional file and module checks, in-place file selection with Rust
  argument handoff, DLC removal, original-path restore, memory and folder/log
  status, and local-network denial diagnostics. Do not add an outgoing Mac
  controller mode. Include synthetic-data launcher behavior verification.

* Add app-local native IOUSBHost input for Wolverine V3 TE 8K PC 1532:0a5a,
  gated by exact interface identity and XUSB report validation. Merge one raw
  controller into player allocation, retain existing Xbox button positions,
  apply stick dead zones and trigger thresholds, and clear state on disconnect.
  Show USB readiness in the Mac launcher; preserve the existing OS-controller
  and phone-pad paths. No driver installation or global virtual gamepad.
* Add the system Swift library rpath to the Mac link, so launchers that use Swift
  concurrency resolve their runtime before entering main. Exercise the packaged
  executable's CLI help entry point during app creation to catch dyld failures.
