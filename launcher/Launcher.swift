// Mush8 - Launcher.swift
// Constantin Clerc (c22dev) - 0.1
import AppKit
import Combine
import SwiftUI
import SystemConfiguration

func nspKind(_ url: URL) -> String? {
    guard let h = try? FileHandle(forReadingFrom: url) else { return nil }
    defer { try? h.close() }
    guard let d = try? h.read(upToCount: 0x10000), d.count >= 0x10, d.prefix(4) == Data("PFS0".utf8) else { return nil }
    let u32 = { (o: Int) in Int(d[o]) | Int(d[o + 1]) << 8 | Int(d[o + 2]) << 16 | Int(d[o + 3]) << 24 }
    let start = 0x10 + u32(4) * 0x18, end = start + u32(8)
    guard end <= d.count else { return nil }
    let names = String(decoding: d[start..<end], as: UTF8.self).split(separator: "\0")
    guard let tik = names.first(where: { $0.hasSuffix(".tik") }), let tid = UInt64(tik.prefix(16), radix: 16) else { return nil }
    guard [0x0100152000022, 0x0100152000023].contains(tid >> 12) else { return "other" }
    switch tid & 0xFFF {
    case 0: return "base"
    case 0x800: return "update"
    default: return "dlc"
    }
}

let supportURL = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/Mush8")
let configURL = supportURL.appendingPathComponent("config.json")
let logURL = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Logs/Mush8.log")

func tool(_ name: String, in folder: URL) -> URL {
    let bundled = Bundle.main.bundleURL.appendingPathComponent("Contents/MacOS/\(name)")
    if FileManager.default.isExecutableFile(atPath: bundled.path) { return bundled }
    return folder.appendingPathComponent("target/release/\(name)")
}

private var nxpatchStamps: [String: String] = [:]

func gameCodeReady(in folder: URL, patches: [String]) -> Bool {
    let made = try? String(contentsOf: folder.appendingPathComponent("work/v-upd/patched/nxpatch.stamp"), encoding: .utf8)
    guard let made else { return false }
    let tool = tool("nxpatch", in: folder)
    let args = ["stamp"] + patches.flatMap { ["--patch", $0] }
    let modified = patches.map { p in
        (try? FileManager.default.attributesOfItem(atPath: folder.appendingPathComponent(p).path)[.modificationDate] as? Date).map { "\($0.timeIntervalSince1970)" } ?? ""
    }
    let key = ([tool.path] + args + modified).joined(separator: "\n")
    if nxpatchStamps[key] == nil {
        let p = Process()
        let pipe = Pipe()
        p.executableURL = tool
        p.arguments = args
        p.currentDirectoryURL = folder
        p.standardOutput = pipe
        guard (try? p.run()) != nil else { return true }
        p.waitUntilExit()
        nxpatchStamps[key] = String(data: pipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
    }
    return made.trimmingCharacters(in: .whitespacesAndNewlines) == nxpatchStamps[key]?.trimmingCharacters(in: .whitespacesAndNewlines)
}

func updateSource(_ s: Settings) -> String {
    let url = s.folderURL.appendingPathComponent(s.update).resolvingSymlinksInPath()
    let size = (try? FileManager.default.attributesOfItem(atPath: url.path)[.size] as? Int) ?? 0
    return "\(size)"
}

func preparedSource(in folder: URL) -> String? {
    try? String(contentsOf: folder.appendingPathComponent("work/v-upd/source"), encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines)
}

func preparedVersion(in folder: URL) -> String? {
    guard let d = try? Data(contentsOf: folder.appendingPathComponent("work/v-upd/control/control.nacp")), d.count >= 0x3070 else { return nil }
    let v = String(decoding: d[0x3060..<0x3070].prefix { $0 != 0 }, as: UTF8.self)
    return v.isEmpty ? nil : v
}

final class Settings: ObservableObject {
    @Published var folder = ""
    @Published var keys = "prod.keys"
    @Published var base = "base.nsp"
    @Published var update = "update.nsp"
    @Published var dlc = "dlc.nsp"
    @Published var network = "off"
    @Published var lanInterface = ""
    @Published var lanPlayServer = ""
    @Published var nextendoPid: UInt64 = 0
    @Published var nextendoName = ""
    @Published var nickname = ""
    @Published var nicknameSource = "nextendo"
    @Published var country = ""
    @Published var pipelineArchive = false
    @Published var prewarm = true
    @Published var rumble = true
    @Published var rumbleMin = 0.45
    @Published var padDriver = "apple"
    @Published var spatial = true
    @Published var backgroundCompile = true
    @Published var motion = false
    @Published var touchscreen = true
    @Published var phonePads = true
    @Published var browserPads = false
    @Published var handheld = false
    @Published var uhd = false
    @Published var split60 = true
    @Published var fillScreen = true
    @Published var hudAtEdges = true
    @Published var hudScale = 1.0
    @Published var disabledMods: [String] = []
    private var autosave: AnyCancellable?

    init() {
        defer {
            autosave = objectWillChange.debounce(for: .milliseconds(150), scheduler: RunLoop.main).sink { [weak self] in self?.save() }
        }
        guard let data = try? Data(contentsOf: configURL),
              let j = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return }
        folder = j["folder"] as? String ?? ""
        keys = j["keys"] as? String ?? keys
        base = j["base"] as? String ?? base
        update = j["update"] as? String ?? update
        dlc = j["dlc"] as? String ?? dlc
        network = j["network"] as? String ?? ((j["lan"] as? Bool ?? false) ? "lan" : "off")
        lanPlayServer = j["lanplay_server"] as? String ?? ""
        nextendoPid = (j["nextendo_pid"] as? NSNumber)?.uint64Value ?? 0
        nextendoName = j["nextendo_name"] as? String ?? ""
        nickname = j["nickname"] as? String ?? ""
        nicknameSource = j["nickname_source"] as? String ?? nicknameSource
        country = j["country"] as? String ?? ""
        lanInterface = j["lan_interface"] as? String ?? ""
        pipelineArchive = j["pipeline_archive"] as? Bool ?? false
        prewarm = j["prewarm_pipelines"] as? Bool ?? true
        rumble = j["rumble"] as? Bool ?? true
        rumbleMin = j["rumble_min"] as? Double ?? 0.45
        padDriver = j["gamepad_driver"] as? String ?? "apple"
        spatial = j["spatial_audio"] as? Bool ?? true
        backgroundCompile = j["background_compile"] as? Bool ?? true
        motion = j["motion_controls"] as? Bool ?? false
        touchscreen = j["touchscreen"] as? Bool ?? true
        phonePads = j["phone_pads"] as? Bool ?? true
        browserPads = j["browser_pads"] as? Bool ?? false
        handheld = j["handheld"] as? Bool ?? false
        uhd = j["uhd"] as? Bool ?? false
        split60 = j["split60"] as? Bool ?? true
        fillScreen = j["aspect"] as? String != "16:9"
        hudAtEdges = j["hud"] as? String != "center"
        hudScale = j["hud_scale"] as? Double ?? 1.0
        disabledMods = j["disabled_mods"] as? [String] ?? []
    }

    var folderURL: URL { URL(fileURLWithPath: folder) }

    func codePatches(_ mods: [Mod]) -> [String] { enabledCodePatches(mods, disabled: disabledMods) }

    func save() {
        let j: [String: Any] = [
            "folder": folder, "keys": keys, "base": base, "update": update, "dlc": dlc, "network": network,
            "lan_interface": lanInterface, "lanplay_server": lanPlayServer, "pipeline_archive": pipelineArchive,
            "nextendo_pid": NSNumber(value: nextendoPid), "nextendo_name": nextendoName, "nickname": nickname, "nickname_source": nicknameSource,
            "country": country,
            "prewarm_pipelines": prewarm, "rumble": rumble, "rumble_min": rumbleMin, "gamepad_driver": padDriver, "spatial_audio": spatial, "background_compile": backgroundCompile, "motion_controls": motion, "touchscreen": touchscreen,
            "handheld": handheld, "uhd": uhd, "split60": split60, "phone_pads": phonePads, "browser_pads": browserPads,
            "aspect": fillScreen ? "fill" : "16:9", "hud": hudAtEdges ? "edges" : "center", "hud_scale": hudScale,
            "disabled_mods": disabledMods,
        ]
        try? FileManager.default.createDirectory(at: supportURL, withIntermediateDirectories: true)
        if let data = try? JSONSerialization.data(withJSONObject: j, options: [.prettyPrinted, .sortedKeys]) {
            try? data.write(to: configURL)
            try? FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: configURL.path)
        }
    }

    var environment: [String: String] {
        var env = ProcessInfo.processInfo.environment
        env["NX_NET"] = network
        switch network {
        case "lan":
            env["NX_LAN"] = "1"
            if !lanInterface.isEmpty { env["NX_LAN_IF"] = lanInterface }
        case "lanplay":
            env["NX_LANPLAY_SERVER"] = lanPlayServer.trimmingCharacters(in: .whitespaces)
        case "nextendo":
            env["NX_NEXTENDO_PID"] = String(nextendoPid)
        default:
            break
        }
        env["NX_NICKNAME"] = playerNickname(nextendo: network == "nextendo", source: nicknameSource, custom: nickname, nextendoName: nextendoName)
        env["NX_COUNTRY"] = playerCountry(nextendo: network == "nextendo", country: country)
        if pipelineArchive { env["NX_PIPELINE_ARCHIVE"] = "1" }
        if prewarm { env["NX_PREWARM"] = "1" }
        if !rumble { env["NX_RUMBLE"] = "0" }
        env["NX_RUMBLE_MIN"] = String(rumbleMin)
        env["NX_PAD_DRIVER"] = padDriver
        if !spatial { env["NX_SPATIAL"] = "0" }
        if !backgroundCompile { env["NX_SYNC_SHADERS"] = "1" }
        env["NX_MOTION"] = motion ? "1" : "0"
        if !touchscreen { env["NX_TOUCHSCREEN"] = "0" }
        env["NX_PHONE_PADS"] = phonePads ? "1" : "0"
        if phonePads && browserPads { env["NX_BROWSER_PADS"] = "1" }
        if handheld { env["NX_HANDHELD"] = "1" } else if uhd { env["NX_4K"] = "1" }
        if split60 { env["NX_SPLIT60"] = "1" }
        if fillScreen { env["NX_ASPECT"] = "auto" }
        if !hudAtEdges { env["NX_HUD"] = "center" }
        if hudScale != 1 { env["NX_HUD_SCALE"] = String(hudScale) }
        return env
    }

    func arguments(_ mods: [Mod]) -> [String] {
        var a = ["--keys", keys, "--base", base, "--update", update]
        if FileManager.default.fileExists(atPath: folderURL.appendingPathComponent(dlc).path) { a += ["--dlc", dlc] }
        for m in mods where !disabledMods.contains(m.id) && m.romfs && m.loadable { a += ["--mod", m.path] }
        return a
    }
}

func enabledCodePatches(_ mods: [Mod], disabled: [String]) -> [String] {
    mods.filter { !disabled.contains($0.id) && $0.loadable }.flatMap(\.patches)
}

struct Check: Identifiable {
    let id: String
    let ok: Bool
    let detail: String
    var optional = false
}

func checks(_ s: Settings, mods: [Mod]) -> [Check] {
    let fm = FileManager.default
    let f = s.folderURL
    let has = { (p: String) in fm.fileExists(atPath: f.appendingPathComponent(p).path) }
    let shaderIndex = f.appendingPathComponent("work/shader_cache.noindex/heap-segments-v2/index.tsv").path
    let shaders = (try? fm.attributesOfItem(atPath: shaderIndex)[.size] as? Int) ?? nil
    let on = mods.filter { !s.disabledMods.contains($0.id) }.count
    return [
        Check(id: "Base Game", ok: has(s.base), detail: has(s.base) ? s.base : "not found"),
        Check(id: "Update", ok: has(s.update), detail: has(s.update) ? s.update : "not found"),
        Check(id: "Booster Course Pass", ok: has(s.dlc), detail: has(s.dlc) ? s.dlc : "none", optional: true),
        Check(id: "Mods", ok: !mods.isEmpty, detail: mods.isEmpty ? "none" : "\(on) of \(mods.count) on", optional: true),
        Check(id: "Game code for this Mac", ok: gameCodeReady(in: f, patches: s.codePatches(mods)) && preparedSource(in: f) == updateSource(s),
              detail: preparedVersion(in: f).map { "work/v-upd/patched (\($0))" } ?? "work/v-upd/patched"),
        Check(id: "Precompiled shaders", ok: (shaders ?? 0) > 0,
              detail: shaders == 0 ? "none, install the Metal Toolchain and Prepare again" : "work/shader_cache.noindex/heap-segments-v2", optional: shaders == 0),
        Check(id: "Console Keys", ok: has(s.keys), detail: has(s.keys) ? s.keys : "not found"),
        Check(id: "Firmware", ok: has("Firmware"), detail: has("Firmware") ? "Firmware/" : "not found"),
    ]
}

final class Preparer: ObservableObject {
    @Published var running = false
    @Published var line = ""
    @Published var error: String?

    func run(_ s: Settings, mods: [Mod], done: @escaping () -> Void) {
        let f = s.folderURL
        let disabled = s.disabledMods
        var steps: [(URL, () -> [String])] = []
        let fm = FileManager.default
        if preparedSource(in: f) != updateSource(s) || !fm.fileExists(atPath: f.appendingPathComponent("work/v-upd/exefs/main").path) {
            try? fm.removeItem(at: f.appendingPathComponent("work/v-upd"))
            try? fm.createDirectory(at: f.appendingPathComponent("work/v-upd"), withIntermediateDirectories: true)
            try? updateSource(s).write(to: f.appendingPathComponent("work/v-upd/source"), atomically: true, encoding: .utf8)
            let extract = ["extract", "--keys", s.keys, "--base", s.base, "--update", s.update, "--out", "work/v-upd"]
            steps.append((tool("nxfs", in: f), { extract }))
        }
        if !gameCodeReady(in: f, patches: s.codePatches(mods)) {
            steps.append((tool("nxpatch", in: f), { ["rewrite", "--out", "work/v-upd/patched", "work/v-upd/exefs/main", "work/v-upd/exefs/subsdk0", "work/v-upd/exefs/sdk"]
                + enabledCodePatches(installedMods(in: f), disabled: disabled).flatMap { ["--patch", $0] } }))
        }
        let precompile = s.arguments(mods) + ["--precompile"]
        steps.append((tool("nxhost", in: f), { precompile }))
        if s.pipelineArchive { setenv("NX_PIPELINE_ARCHIVE", "1", 1) } else { unsetenv("NX_PIPELINE_ARCHIVE") }
        running = true
        error = nil
        next(steps[...], folder: f, done: done)
    }

    private func next(_ steps: ArraySlice<(URL, () -> [String])>, folder: URL, done: @escaping () -> Void) {
        guard let (exe, args) = steps.first.map({ ($0.0, $0.1()) }) else {
            running = false
            line = ""
            done()
            return
        }
        let p = Process()
        p.executableURL = exe
        p.arguments = args
        p.currentDirectoryURL = folder
        let pipe = Pipe()
        p.standardOutput = pipe
        p.standardError = pipe
        pipe.fileHandleForReading.readabilityHandler = { h in
            guard let text = String(data: h.availableData, encoding: .utf8),
                  let last = text.split(separator: "\n").last else { return }
            DispatchQueue.main.async { self.line = String(last) }
        }
        p.terminationHandler = { p in
            pipe.fileHandleForReading.readabilityHandler = nil
            DispatchQueue.main.async {
                if p.terminationStatus == 0 {
                    self.next(steps.dropFirst(), folder: folder, done: done)
                } else {
                    self.running = false
                    self.error = "\(exe.lastPathComponent) failed: \(self.line)"
                    done()
                }
            }
        }
        do { try p.run() } catch {
            running = false
            self.error = "Could not run \(exe.lastPathComponent): \(error.localizedDescription)"
        }
    }
}

struct NetInterface: Hashable {
    let name: String
    let ip: String
}

func interfaces() -> [NetInterface] {
    var out: [NetInterface] = []
    var ifs: UnsafeMutablePointer<ifaddrs>?
    guard getifaddrs(&ifs) == 0, let first = ifs else { return out }
    defer { freeifaddrs(ifs) }
    for p in sequence(first: first, next: { $0.pointee.ifa_next }) {
        let flags = Int32(p.pointee.ifa_flags)
        guard let a = p.pointee.ifa_addr, a.pointee.sa_family == UInt8(AF_INET),
              flags & IFF_UP != 0, flags & IFF_LOOPBACK == 0 else { continue }
        var host = [CChar](repeating: 0, count: Int(NI_MAXHOST))
        if getnameinfo(a, socklen_t(a.pointee.sa_len), &host, socklen_t(host.count), nil, 0, NI_NUMERICHOST) == 0 {
            out.append(NetInterface(name: String(cString: p.pointee.ifa_name), ip: String(cString: host)))
        }
    }
    return out
}

func availableMemory() -> UInt64 {
    #if os(iOS)
    return UInt64(os_proc_available_memory())
    #else
    return systemAvailableMemory()
    #endif
}

func systemAvailableMemory() -> UInt64 {
    var stats = vm_statistics64()
    var count = mach_msg_type_number_t(MemoryLayout<vm_statistics64>.size / MemoryLayout<integer_t>.size)
    let r = withUnsafeMutablePointer(to: &stats) {
        $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) { host_statistics64(mach_host_self(), HOST_VM_INFO64, $0, &count) }
    }
    guard r == KERN_SUCCESS else { return 0 }
    let pages = UInt64(stats.free_count) + UInt64(stats.inactive_count) + UInt64(stats.purgeable_count) + UInt64(stats.speculative_count)
    return pages * UInt64(vm_kernel_page_size)
}

func bytes(_ n: UInt64) -> String { ByteCountFormatter.string(fromByteCount: Int64(n), countStyle: .memory) }

func lanIP() -> String? {
    let fd = socket(AF_INET, SOCK_DGRAM, 0)
    guard fd >= 0 else { return nil }
    defer { close(fd) }
    var dst = sockaddr_in()
    dst.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
    dst.sin_family = sa_family_t(AF_INET)
    dst.sin_port = in_port_t(9).bigEndian
    inet_pton(AF_INET, "192.0.2.1", &dst.sin_addr)
    var local = sockaddr_in()
    var len = socklen_t(MemoryLayout<sockaddr_in>.size)
    let connected = withUnsafePointer(to: &dst) {
        $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { connect(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size)) }
    }
    let named = withUnsafeMutablePointer(to: &local) { $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { getsockname(fd, $0, &len) } }
    guard connected == 0, named == 0 else { return nil }
    return String(cString: inet_ntoa(local.sin_addr))
}

final class SystemInfo: ObservableObject {
    @Published var available: UInt64 = availableMemory()
    @Published var nets: [NetInterface] = interfaces()
    @Published var lan: String? = lanIP()
    private var timer: Timer?

    init() {
        timer = Timer.scheduledTimer(withTimeInterval: 2, repeats: true) { [weak self] _ in
            self?.available = availableMemory()
            self?.nets = interfaces()
            self?.lan = lanIP()
        }
    }
}

let phonePadHost = "http://\((SCDynamicStoreCopyLocalHostName(nil) as String?) ?? "this-mac").local:2904"

func launch(_ s: Settings, mods: [Mod]) -> String {
    FileManager.default.changeCurrentDirectoryPath(s.folder)
    for (k, v) in s.environment { setenv(k, v, 1) }
    let prevLog = logURL.deletingPathExtension().appendingPathExtension("prev.log")
    try? FileManager.default.removeItem(at: prevLog)
    try? FileManager.default.moveItem(at: logURL, to: prevLog)
    let fd = open(logURL.path, O_WRONLY | O_CREAT | O_TRUNC, 0o644)
    if fd >= 0 {
        dup2(fd, STDOUT_FILENO)
        dup2(fd, STDERR_FILENO)
        close(fd)
    }
    let game = tool("nxhost", in: s.folderURL).path
    var argv: [UnsafeMutablePointer<CChar>?] = ([game] + s.arguments(mods)).map { strdup($0) } + [nil]
    execv(game, &argv)
    return "Could not start the game (\(String(cString: strerror(errno))))"
}

struct LauncherView: View {
    @StateObject private var s = Settings()
    @StateObject private var sys = SystemInfo()
    @StateObject private var prep = Preparer()
    @State private var launchError: String?
    @State private var launching = false
    @State private var shownRefusal: String?
    @State private var tick = 0
    @State private var copied: String?
    @State private var mods: [Mod] = []
    @State private var miiCount = 0
    @State private var miiError: String?
    @State private var pageOpen = false

    var body: some View {
        let _ = tick
        let list = checks(s, mods: mods)
        let ready = list.allSatisfy { $0.ok || $0.optional }
        VStack(spacing: 16) {
            HStack(spacing: 12) {
                Image(nsImage: NSApp.applicationIconImage).resizable().frame(width: 48, height: 48)
                Text("Mush8").font(.title2.weight(.semibold))
                Spacer()
            }
            NavigationStack {
                Form {
                    if !ready { dataSection(list) }
                    Section {
                        NavigationLink { multiplayerPage } label: {
                            CategoryLabel(title: "Multiplayer", icon: "person.2.fill", color: .green, value: modeName)
                        }
                        if let mods = list.first(where: { $0.id == "Mods" }) {
                            NavigationLink { ModsView(s: s, onInstalled: { tick += 1 }) } label: {
                                CategoryLabel(title: "Mods", icon: "puzzlepiece.extension.fill", color: .orange, value: mods.detail)
                            }
                            .disabled(s.folder.isEmpty || prep.running)
                        }
                        NavigationLink { playerPage } label: {
                            CategoryLabel(title: "Player", icon: "person.crop.circle.fill", color: .purple, value: playerName)
                        }
                    }
                    Section {
                        NavigationLink { graphicsPage } label: {
                            CategoryLabel(title: "Graphics", icon: "display", color: .blue, value: s.handheld ? "Handheld" : s.uhd ? "4K" : "1080p")
                        }
                        NavigationLink { controlsPage } label: {
                            CategoryLabel(title: "Audio & Controls", icon: "gamecontroller.fill", color: .pink, value: nil)
                        }
                    }
                    Section {
                        NavigationLink { devicePage(list) } label: {
                            CategoryLabel(title: "Device", icon: "desktopcomputer", color: sys.available < 4 << 30 ? .orange : .gray, value: bytes(sys.available))
                        }
                    }
                }
                .formStyle(.grouped)
                .onAppear { pageOpen = false }
                .onDisappear { pageOpen = true }
            }
            if let e = prep.error ?? launchError { Text(e).foregroundStyle(.red).font(.callout) }
            if prep.running {
                ProgressView().controlSize(.small)
                Text(prep.line).font(.caption.monospaced()).lineLimit(1).truncationMode(.middle)
            } else if ready {
                Button(action: play) {
                    Text("Play").font(.title3.weight(.semibold)).opacity(launching ? 0 : 1)
                        .overlay { if launching { ProgressView().controlSize(.small).tint(.white) } }
                        .frame(maxWidth: .infinity)
                }
                    .controlSize(.large).buttonStyle(.borderedProminent).keyboardShortcut(pageOpen ? nil : .defaultAction)
            } else {
                Button(action: prepare) { Text("Prepare game data").frame(maxWidth: .infinity) }
                    .controlSize(.large).buttonStyle(.borderedProminent)
                    .disabled(pageOpen || s.folder.isEmpty || !list.filter { ["Console Keys", "Base Game", "Update", "Firmware"].contains($0.id) }.allSatisfy(\.ok))
                Text("Compiles the game's shaders AOT (may take a few mins)")
                    .font(.caption).foregroundStyle(.secondary).multilineTextAlignment(.center)
            }
            Text("proudly made by c22dev").font(.caption).foregroundStyle(.secondary)
        }
        .padding(20)
        .frame(width: 520, height: 770)
        .onAppear { reload() }
        .onChange(of: tick) { reload() }
        .onChange(of: s.folder) { reload() }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in tick += 1 }
    }

    func dataSection(_ list: [Check]) -> some View {
        Section("Game data") {
            HStack {
                Text(s.folder.isEmpty ? "No folder chosen" : s.folder).lineLimit(1).truncationMode(.head)
                    .foregroundStyle(s.folder.isEmpty ? .secondary : .primary)
                Spacer()
                Button("Choose…", action: chooseFolder).disabled(prep.running)
            }
            ForEach(list.filter { $0.id != "Mods" }) { c in
                HStack {
                    Image(systemName: c.ok ? "checkmark.circle.fill" : c.optional ? "minus.circle" : "xmark.circle")
                        .foregroundStyle(c.ok ? .green : c.optional ? .secondary : .red)
                    Text(c.id)
                    Spacer()
                    Text(c.detail).font(.caption.monospaced()).foregroundStyle(.secondary)
                }
            }
        }
    }

    func labeled(_ title: String, _ detail: String?) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title)
            if let detail { Text(detail).font(.caption).foregroundStyle(.secondary) }
        }
    }

    var multiplayerNote: String? {
        switch s.network {
        case "lan": return "Switch and Mush8 players on the same network can join"
        case "lanplay": return "Lan-Play.com clients on the same server can join"
        case "wireless": return "Nearby Mush8 clients can join (keep Wi-Fi on)"
        case "nextendo": return "Online with Switch players. Needs a Nextendo account and update 4.0.1"
        default: return nil
        }
    }

    var modeName: String {
        switch s.network {
        case "lan": return "LAN"
        case "lanplay": return "Lan-Play"
        case "wireless": return "Wireless"
        case "nextendo": return "Nextendo"
        default: return "Off"
        }
    }

    var multiplayerPage: some View {
        CategoryPage(title: "Multiplayer") {
            Section {
                Picker("Mode", selection: $s.network) {
                    Text("Off").tag("off")
                    Text("LAN").tag("lan")
                    Text("Lan-Play").tag("lanplay")
                    Text("Wireless (nearby)").tag("wireless")
                    Text("Nextendo").tag("nextendo")
                }
                switch s.network {
                case "lan":
                    Picker("Network", selection: $s.lanInterface) {
                        Text("Automatic").tag("")
                        ForEach(sys.nets, id: \.self) { n in Text("\(n.name) · \(n.ip)").tag(n.name) }
                    }
                case "lanplay":
                    TextField("Server", text: $s.lanPlayServer, prompt: Text("host:port"))
                case "nextendo":
                    NextendoSignIn(pid: $s.nextendoPid, name: $s.nextendoName)
                default:
                    EmptyView()
                }
            } footer: {
                if let note = multiplayerNote { Text(note).font(.caption).foregroundStyle(.secondary) }
            }
            Section {
                Toggle("Remote controllers", isOn: $s.phonePads)
                if s.phonePads {
                    Toggle("Web controllers", isOn: $s.browserPads)
                    if s.browserPads {
                        addressRow("Mac's address", phonePadHost)
                        addressRow("Or by IP", sys.lan.map { "http://\($0):2904" })
                    }
                }
            } footer: {
                Text(s.phonePads && s.browserPads
                    ? "Phones on the same Wi-Fi open one of these in a browser after Play"
                    : "Nearby iPhones join with \"Use as a controller\"")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
    }

    func devicePage(_ list: [Check]) -> some View {
        CategoryPage(title: "Device") {
            systemSection
            dataSection(list)
        }
    }

    var playerName: String? {
        let name = playerNickname(nextendo: s.network == "nextendo", source: s.nicknameSource, custom: s.nickname, nextendoName: s.nextendoName)
        return name.isEmpty ? nil : name
    }

    var playerPage: some View {
        CategoryPage(title: "Player") {
            Section {
                PlayerIdentity(nextendo: s.network == "nextendo", nextendoName: s.nextendoName,
                               nameSource: $s.nicknameSource, name: $s.nickname, country: $s.country)
                HStack {
                    Text(miiCount == 0 ? "No custom Miis" : "\(miiCount) custom Mii\(miiCount == 1 ? "" : "s")").foregroundStyle(.secondary)
                    Spacer()
                    if miiCount > 0 {
                        Button("Remove") {
                            miiError = nil
                            do { try removeMiis(in: s.folderURL) } catch { miiError = error.localizedDescription }
                            tick += 1
                        }
                    }
                    Button("Import…", action: chooseMiis)
                }
                .disabled(s.folder.isEmpty || prep.running)
                if let miiError { Text(miiError).foregroundStyle(.red).font(.callout) }
            } footer: {
                Text("Shown on your ghosts and online, 10 characters max. The country sets your Nextendo flag. Miis: a MiiDatabase.dat, .charinfo or .storedata")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
    }

    var graphicsPage: some View {
        CategoryPage(title: "Graphics") {
            Section {
                Picker("Console mode", selection: Binding(get: { s.handheld ? 2 : s.uhd ? 1 : 0 }, set: { s.handheld = $0 == 2; s.uhd = $0 == 1 })) {
                    Text("Docked (1080p)").tag(0)
                    Text("Docked (4K)").tag(1)
                    Text("Handheld (720p)").tag(2)
                }
                Toggle(isOn: $s.split60) { labeled("60 fps split screen", "For 3 or 4 players, needs a fast GPU") }
                Picker("Screen", selection: $s.fillScreen) {
                    Text("16:9").tag(false)
                    Text("Fill screen").tag(true)
                }
                if s.fillScreen {
                    Picker("HUD", selection: $s.hudAtEdges) {
                        Text("Screen edges").tag(true)
                        Text("Centered").tag(false)
                    }
                }
                HStack {
                    Text("HUD size")
                    Slider(value: $s.hudScale, in: 0.5...1.5, step: 0.05)
                    Text("\(Int((s.hudScale * 100).rounded())) %").monospacedDigit().foregroundStyle(.secondary)
                }
            } footer: {
                Text("Handheld is lighter on the GPU and battery").font(.caption).foregroundStyle(.secondary)
            }
            Section {
                Toggle("Pipeline archive", isOn: $s.pipelineArchive)
                Toggle("Prewarm pipelines", isOn: $s.prewarm)
                Toggle("Background shader compiling", isOn: $s.backgroundCompile)
            } footer: {
                Text("Builds graphics pipelines at startup").font(.caption).foregroundStyle(.secondary)
            }
        }
    }

    var controlsPage: some View {
        CategoryPage(title: "Audio & Controls") {
            Section {
                Toggle("Spatial audio", isOn: $s.spatial)
            }
            Section {
                Toggle("Vibration", isOn: $s.rumble)
                if s.rumble {
                    VStack(alignment: .leading, spacing: 2) {
                        HStack {
                            Text("Rumble cutoff")
                            Slider(value: $s.rumbleMin, in: 0...0.9, step: 0.05)
                            Text("\(Int((s.rumbleMin * 100).rounded())) %").monospacedDigit().foregroundStyle(.secondary)
                        }
                        Text("Weaker rumble is ignored, which stops the buzz on Xbox and PlayStation controllers. Switch controllers aren't affected")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                }
            }
            Section {
                Toggle("Motion controls", isOn: $s.motion)
                Toggle(isOn: $s.touchscreen) {
                    labeled("Touchscreen", "Clicking the game window taps the touchscreen")
                }
                Picker(selection: $s.padDriver) {
                    Text("Apple").tag("apple")
                    Text("SDL").tag("sdl")
                } label: {
                    labeled("Gamepad driver", "SDL supports more controllers, Apple follows System Settings remapping")
                }
            }
        }
    }

    var systemSection: some View {
        Section {
            row("Memory available", bytes(sys.available), warn: sys.available < 4 << 30 ? "4 GB of free RAM is recommended" : nil)
        }
    }

    func addressRow(_ label: String, _ value: String?) -> some View {
        HStack {
            Text(label)
            Spacer()
            Text(value ?? "no network").foregroundStyle(.secondary).monospacedDigit().textSelection(.enabled)
            if let v = value {
                Button {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(v, forType: .string)
                    copied = v
                    DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { if copied == v { copied = nil } }
                } label: {
                    Image(systemName: copied == v ? "checkmark" : "doc.on.doc").frame(width: 16)
                }
                .buttonStyle(.borderless)
                .help("Copy")
            }
        }
    }

    func row(_ label: String, _ value: String, warn: String? = nil) -> some View {
        HStack {
            Text(label)
            Spacer()
            VStack(alignment: .trailing) {
                Text(value).foregroundStyle(.secondary).monospacedDigit()
                if let w = warn { Text(w).font(.caption).foregroundStyle(.orange) }
            }
        }
    }

    func chooseFolder() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.message = "Choose the folder with your game files"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        s.folder = url.path
        if let nsps = try? FileManager.default.contentsOfDirectory(atPath: url.path).filter({ $0.lowercased().hasSuffix(".nsp") }).sorted() {
            let kinds = nsps.map { nspKind(url.appendingPathComponent($0)) }
            func pick(_ kind: String, _ word: String) -> String? {
                nsps.indices.first(where: { kinds[$0] == kind }).map { nsps[$0] }
                    ?? nsps.indices.first(where: { kinds[$0] == nil && nsps[$0].lowercased().contains(word) }).map { nsps[$0] }
            }
            if let b = pick("base", "base") { s.base = b }
            if let u = pick("update", "upd") { s.update = u }
            if let d = pick("dlc", "dlc") { s.dlc = d }
        }
        if let k = try? FileManager.default.contentsOfDirectory(atPath: url.path).first(where: { $0.lowercased().hasSuffix(".keys") && $0.lowercased().contains("prod") }) { s.keys = k }
        s.save()
    }

    func reload() {
        mods = installedMods(in: s.folderURL)
        miiCount = s.folder.isEmpty ? 0 : installedMiiCount(in: s.folderURL)
    }

    func chooseMiis() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = true
        panel.message = "Choose a MiiDatabase.dat or Mii files"
        guard panel.runModal() == .OK else { return }
        miiError = nil
        do { try importMiis(panel.urls, into: s.folderURL) } catch { miiError = error.localizedDescription }
        tick += 1
    }

    func prepare() {
        s.save()
        mods = installedMods(in: s.folderURL)
        prep.run(s, mods: mods) { tick += 1 }
    }

    func play() {
        mods = installedMods(in: s.folderURL)
        guard checks(s, mods: mods).allSatisfy({ $0.ok || $0.optional }) else {
            launchError = "Your game files or mods changed. Prepare again before playing."
            return
        }
        if let mod = mods.first(where: { !s.disabledMods.contains($0.id) && !$0.loadable }) {
            launchError = "\(mod.displayTitle) can't load: \(mod.summary). Turn it off in Mods."
            return
        }
        if s.network == "lanplay" && s.lanPlayServer.trimmingCharacters(in: .whitespaces).isEmpty {
            launchError = "Enter a LAN-Play server (host:port)"
            return
        }
        s.save()
        guard s.network == "nextendo" else {
            launchError = launch(s, mods: mods)
            return
        }
        guard !launching else { return }
        launching = true
        Task { @MainActor in
            defer { launching = false }
            do {
                let login = try await Nextendo.gameLogin()
                s.nextendoPid = login.pid
                s.nextendoName = login.name
                if s.country.isEmpty && !login.country.isEmpty { s.country = login.country }
                await Nextendo.syncMii(folder: s.folderURL, login: login)
                s.save()
                setenv("NX_NEXTENDO_TOKEN", login.token, 1)
                setenv("NX_NEXTENDO_ACCESS", login.access, 1)
                setenv("NX_NEXTENDO_REFRESH", NextendoKeychain.read("refresh_token"), 1)
                let gate = await Nextendo.onlineGate(login.token)
                setenv("NX_NEXTENDO_GATE", gate.log, 1)
                let refusal = gate.problem.map { "\(login.pid) \($0)" }
                if let problem = gate.problem, shownRefusal != refusal {
                    shownRefusal = refusal
                    launchError = problem + " Press Play again to start anyway: online play won't work until then."
                    return
                }
                shownRefusal = nil
                launchError = launch(s, mods: mods)
            } catch {
                launchError = error.localizedDescription
            }
        }
    }
}

struct ModsView: View {
    @ObservedObject var s: Settings
    let onInstalled: () -> Void
    @State private var mods: [Mod] = []
    @State private var skipped: [String: String] = [:]
    @State private var browsing = false
    @State private var inspectedMod: Mod?
    @State private var deletingMod: Mod?
    @State private var actionError: String?

    var body: some View {
        CategoryPage(title: "Mods") {
            Section {
                if mods.isEmpty {
                    Text("No mods installed. Browse GameBanana to add one.").foregroundStyle(.secondary)
                }
                ForEach(mods) { m in
                    ModRow(mod: m, disabled: $s.disabledMods, warning: patchWarning(m, skipped: skipped), inspect: { inspectedMod = m }, delete: { deletingMod = m })
                }
            } header: {
                ModsHeader(mods: mods, disabled: s.disabledMods)
            } footer: {
                Text("Lower mods win conflicts. Code patches apply on the next Prepare")
                    .font(.caption).foregroundStyle(.secondary)
            }
        } accessory: {
            Button("Show in Finder") { NSWorkspace.shared.open(s.folderURL.appendingPathComponent("mods")) }
                .disabled(mods.isEmpty)
            Button("Browse GameBanana…") { browsing = true }
        }
        .onAppear(perform: reload)
        .onDisappear(perform: onInstalled)
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in reload() }
        .modActions(folder: s.folderURL, inspected: $inspectedMod, deleting: $deletingMod, error: $actionError, delete: deleteMod)
        .sheet(isPresented: $browsing) {
            ModBrowserView(folder: s.folderURL) { name in
                s.disabledMods.removeAll { $0 == name }
                s.save()
                mods = installedMods(in: s.folderURL)
                onInstalled()
            }
        }
    }

    func reload() {
        mods = installedMods(in: s.folderURL)
        skipped = skippedPatches(in: s.folderURL)
    }

    func deleteMod(_ mod: Mod) {
        deletingMod = nil
        do {
            try deleteInstalledMod(mod, in: s.folderURL)
            s.disabledMods.removeAll { $0 == mod.id }
            s.save()
            mods = installedMods(in: s.folderURL)
            onInstalled()
        } catch { actionError = error.localizedDescription }
    }
}

struct CategoryLabel: View {
    let title: String
    let icon: String
    let color: Color
    let value: String?

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: icon)
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(.white)
                .frame(width: 24, height: 24)
                .background(color.gradient, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
            Text(title)
            Spacer()
            if let value { Text(value).foregroundStyle(.secondary) }
        }
    }
}

struct CategoryPage<Content: View, Accessory: View>: View {
    let title: String
    @ViewBuilder let content: Content
    @ViewBuilder let accessory: Accessory
    @Environment(\.dismiss) private var dismiss

    init(title: String, @ViewBuilder content: () -> Content, @ViewBuilder accessory: () -> Accessory = { EmptyView() }) {
        self.title = title
        self.content = content()
        self.accessory = accessory()
    }

    var body: some View {
        VStack(spacing: 0) {
            ZStack {
                Text(title).font(.headline)
                HStack {
                    Button { dismiss() } label: {
                        Label("Back", systemImage: "chevron.left")
                    }
                    .keyboardShortcut(.cancelAction)
                    Spacer()
                }
            }
            .padding(.horizontal, 20)
            .padding(.top, 8)
            Form { content }.formStyle(.grouped)
            if Accessory.self != EmptyView.self {
                HStack { accessory; Spacer() }
                    .padding(.horizontal, 20)
                    .padding(.bottom, 8)
            }
        }
        .navigationBarBackButtonHidden()
    }
}

@main
struct Mush8Launcher: App {
    var body: some Scene {
        WindowGroup("Mush8") { LauncherView() }
            .windowStyle(.hiddenTitleBar)
            .windowResizability(.contentSize)
    }
}
