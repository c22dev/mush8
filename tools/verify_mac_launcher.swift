// Compile with macos/Launcher.swift. Uses synthetic files, never game data or networking.
import Foundation

@main struct VerifyMacLauncher {
    @MainActor static func main() throws {
        let fm = FileManager.default
        let root = fm.temporaryDirectory.appendingPathComponent("mk8-launcher-\(UUID().uuidString)")
        let suite = "org.mush8.launcher-test.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        try fm.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? fm.removeItem(at: root); defaults.removePersistentDomain(forName: suite) }
        func put(_ name: String) throws {
            let url = root.appendingPathComponent(name)
            try fm.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            try Data([1]).write(to: url)
        }
        for name in ["base.nca", "新 本体.nsp", "update.nsp", "prod.keys", "main.npdm"] { try put(name) }
        for name in ["main", "subsdk0", "sdk"] { for ext in ["bin", "layout"] { try put("patched/\(name).\(ext)") } }
        let args = ["--patched", "patched", "--npdm", "main.npdm", "--keys", "prod.keys",
                    "--base-nca", "base.nca", "--update", "update.nsp", "--firmware", "Firmware", "--save", "save"]
        defaults.set(["future_option": "preserved"], forKey: "gameSettings")
        let game = GameLauncher(config: ["cwd": root.path, "args": args], defaults: defaults)
        game.network = "off"
        precondition(game.canStart, "Optional firmware and DLC must not block startup")
        precondition(game.fileStatus["runtime"] == true && game.fileStatus["firmware"] == false)
        game.network = "lanplay"
        game.lanPlayServer = "  \n "
        precondition(!game.canStart, "Empty relay must block startup")
        game.lanPlayServer = "  localhost:11451 \n"
        game.prewarm = true; game.backgroundCompile = false
        game.rumble = false; game.motion = true; game.handheld = true
        game.phonePads = false; game.language = "en-US"
        try game.applySettings()
        func env(_ key: String) -> String? { getenv(key).map { String(cString: $0) } }
        precondition(env("NX_PREWARM") == "1" && env("NX_SYNC_SHADERS") == "1")
        precondition(env("NX_RUMBLE") == "0" && env("NX_MOTION") == "1" && env("NX_HANDHELD") == "1")
        precondition(env("NX_LANPLAY_SERVER") == "localhost:11451" && env("NX_PHONE_PADS") == "0")
        let restored = GameLauncher(config: ["cwd": root.path, "args": args], defaults: defaults)
        precondition(restored.prewarm && !restored.backgroundCompile && !restored.rumble && restored.motion && restored.handheld)
        precondition(restored.lanPlayServer == "localhost:11451" && restored.language == "en-US" && !restored.phonePads)
        precondition(defaults.dictionary(forKey: "gameSettings")?["future_option"] as? String == "preserved")
        game.network = "lan"; game.language = "system"; game.prewarm = false
        game.backgroundCompile = true; game.rumble = true; game.motion = false; game.handheld = false
        setenv("NX_LAN", "1", 1); setenv("NX_LAN_IF", "stale", 1)
        try game.applySettings()
        for key in ["NX_PREWARM", "NX_SYNC_SHADERS", "NX_HANDHELD", "NX_LANGUAGE", "NX_LANPLAY_SERVER", "NX_LAN", "NX_LAN_IF"] {
            precondition(env(key) == nil, "Stale presence flag: \(key)")
        }
        precondition(env("NX_RUMBLE") == "1" && env("NX_MOTION") == "0")
        game.fileOverrides["base"] = [root.appendingPathComponent("新 本体.nsp").path]
        game.fileOverrides["dlc"] = ["missing.nsp"]
        game.refresh()
        precondition(!game.canStart, "A selected missing DLC must be removed or replaced")
        game.removeDLC()
        precondition(game.canStart)
        try game.applySettings()
        let launched = try JSONDecoder().decode([String].self, from: Data(env("NX_MAC_LAUNCH_ARGS")!.utf8))
        precondition(!launched.contains("--base-nca") && launched.contains("--base"))
        precondition(launched.contains(root.appendingPathComponent("新 本体.nsp").path))
        precondition(launched.contains("--save") && launched.contains("save"))
        let fileRestore = GameLauncher(config: ["cwd": root.path, "args": args], defaults: defaults)
        precondition(fileRestore.paths(for: "base") == game.paths(for: "base"))
        game.resetFiles()
        precondition(game.launchArguments() == args)
        try fm.removeItem(at: root.appendingPathComponent("patched/sdk.layout"))
        game.refresh()
        precondition(!game.canStart && game.fileStatus["runtime"] == false)
        try put("patched/sdk.layout")
        game.refresh()
        precondition(game.canStart)
        try fm.removeItem(at: root.appendingPathComponent("prod.keys"))
        game.refresh()
        precondition(!game.canStart && game.fileStatus["keys"] == false)
        do { try game.applySettings(); preconditionFailure("Missing keys must block application") } catch { }
        print("Mac launcher checks passed: persistence, both flag directions, relay validation, Unicode paths, optional data, required files, module readiness and argument handoff.")
    }
}
