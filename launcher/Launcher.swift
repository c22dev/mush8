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

private var nxpatchStamps: [URL: String] = [:]

func gameCodeReady(in folder: URL) -> Bool {
    let made = try? String(contentsOf: folder.appendingPathComponent("work/v-upd/patched/nxpatch.stamp"), encoding: .utf8)
    guard let made else { return false }
    let tool = tool("nxpatch", in: folder)
    if nxpatchStamps[tool] == nil {
        let p = Process()
        let pipe = Pipe()
        p.executableURL = tool
        p.arguments = ["stamp"]
        p.standardOutput = pipe
        guard (try? p.run()) != nil else { return true }
        p.waitUntilExit()
        nxpatchStamps[tool] = String(data: pipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
    }
    return made.trimmingCharacters(in: .whitespacesAndNewlines) == nxpatchStamps[tool]?.trimmingCharacters(in: .whitespacesAndNewlines)
}

final class Settings: ObservableObject {
    @Published var folder = ""
    @Published var keys = "prod.keys"
    @Published var base = "base.nsp"
    @Published var update = "upd4-0-1.nsp"
    @Published var dlc = "dlc.nsp"
    @Published var network = "off"
    @Published var lanInterface = ""
    @Published var lanPlayServer = ""
    @Published var pipelineArchive = false
    @Published var prewarm = true
    @Published var rumble = true
    @Published var spatial = true
    @Published var backgroundCompile = true
    @Published var motion = false
    @Published var phonePads = true
    @Published var browserPads = false
    @Published var padTerminal = false
    @Published var handheld = false
    @Published var uhd = false
    @Published var split60 = true
    @Published var fillScreen = true
    @Published var hudAtEdges = true
    @Published var hudScale = 1.0
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
        lanInterface = j["lan_interface"] as? String ?? ""
        pipelineArchive = j["pipeline_archive"] as? Bool ?? false
        prewarm = j["prewarm_pipelines"] as? Bool ?? true
        rumble = j["rumble"] as? Bool ?? true
        spatial = j["spatial_audio"] as? Bool ?? true
        backgroundCompile = j["background_compile"] as? Bool ?? true
        motion = j["motion_controls"] as? Bool ?? false
        phonePads = j["phone_pads"] as? Bool ?? true
        browserPads = j["browser_pads"] as? Bool ?? false
        padTerminal = j["browser_pads_terminal"] as? Bool ?? false
        handheld = j["handheld"] as? Bool ?? false
        uhd = j["uhd"] as? Bool ?? false
        split60 = j["split60"] as? Bool ?? true
        fillScreen = j["aspect"] as? String != "16:9"
        hudAtEdges = j["hud"] as? String != "center"
        hudScale = j["hud_scale"] as? Double ?? 1.0
    }

    var folderURL: URL { URL(fileURLWithPath: folder) }

    func save() {
        let j: [String: Any] = [
            "folder": folder, "keys": keys, "base": base, "update": update, "dlc": dlc, "network": network,
            "lan_interface": lanInterface, "lanplay_server": lanPlayServer, "pipeline_archive": pipelineArchive,
            "prewarm_pipelines": prewarm, "rumble": rumble, "spatial_audio": spatial, "background_compile": backgroundCompile, "motion_controls": motion,
            "handheld": handheld, "uhd": uhd, "split60": split60, "phone_pads": phonePads, "browser_pads": browserPads,
            "browser_pads_terminal": padTerminal,
            "aspect": fillScreen ? "fill" : "16:9", "hud": hudAtEdges ? "edges" : "center", "hud_scale": hudScale,
        ]
        try? FileManager.default.createDirectory(at: supportURL, withIntermediateDirectories: true)
        if let data = try? JSONSerialization.data(withJSONObject: j, options: [.prettyPrinted, .sortedKeys]) {
            try? data.write(to: configURL)
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
        default:
            break
        }
        if pipelineArchive { env["NX_PIPELINE_ARCHIVE"] = "1" }
        if prewarm { env["NX_PREWARM"] = "1" }
        if !rumble { env["NX_RUMBLE"] = "0" }
        if !spatial { env["NX_SPATIAL"] = "0" }
        if !backgroundCompile { env["NX_SYNC_SHADERS"] = "1" }
        env["NX_MOTION"] = motion ? "1" : "0"
        env["NX_PHONE_PADS"] = phonePads ? "1" : "0"
        if handheld { env["NX_HANDHELD"] = "1" } else if uhd { env["NX_4K"] = "1" }
        if split60 { env["NX_SPLIT60"] = "1" }
        if fillScreen { env["NX_ASPECT"] = "auto" }
        if !hudAtEdges { env["NX_HUD"] = "center" }
        if hudScale != 1 { env["NX_HUD_SCALE"] = String(hudScale) }
        return env
    }

    var arguments: [String] {
        var a = ["--keys", keys, "--base", base, "--update", update]
        if FileManager.default.fileExists(atPath: folderURL.appendingPathComponent(dlc).path) { a += ["--dlc", dlc] }
        return a
    }
}

struct Check: Identifiable {
    let id: String
    let ok: Bool
    let detail: String
    var optional = false
}

func checks(_ s: Settings) -> [Check] {
    let fm = FileManager.default
    let f = s.folderURL
    let has = { (p: String) in fm.fileExists(atPath: f.appendingPathComponent(p).path) }
    return [
        Check(id: "Base Game", ok: has(s.base), detail: has(s.base) ? s.base : "not found"),
        Check(id: "Update", ok: has(s.update), detail: has(s.update) ? s.update : "not found"),
        Check(id: "Booster Course Pass", ok: has(s.dlc), detail: has(s.dlc) ? s.dlc : "none", optional: true),
        Check(id: "Game code for this Mac", ok: gameCodeReady(in: f), detail: "work/v-upd/patched"),
        Check(id: "Precompiled shaders", ok: has("work/shader_cache.noindex/index.tsv"), detail: "work/shader_cache.noindex"),
        Check(id: "Console Keys", ok: has(s.keys), detail: has(s.keys) ? s.keys : "not found"),
        Check(id: "Firmware", ok: has("Firmware"), detail: has("Firmware") ? "Firmware/" : "not found"),
    ]
}

final class Preparer: ObservableObject {
    @Published var running = false
    @Published var line = ""
    @Published var error: String?

    func run(_ s: Settings, done: @escaping () -> Void) {
        let f = s.folderURL
        var steps: [(URL, [String])] = []
        if !FileManager.default.fileExists(atPath: f.appendingPathComponent("work/v-upd/exefs/main").path) {
            steps.append((tool("nxfs", in: f), ["extract", "--keys", s.keys, "--base", s.base, "--update", s.update, "--out", "work/v-upd"]))
        }
        if !gameCodeReady(in: f) {
            steps.append((tool("nxpatch", in: f), ["rewrite", "--out", "work/v-upd/patched", "work/v-upd/exefs/main", "work/v-upd/exefs/subsdk0", "work/v-upd/exefs/sdk"]))
        }
        steps.append((tool("nxhost", in: f), s.arguments + ["--precompile"]))
        if s.pipelineArchive { setenv("NX_PIPELINE_ARCHIVE", "1", 1) } else { unsetenv("NX_PIPELINE_ARCHIVE") }
        running = true
        error = nil
        next(steps[...], folder: f, done: done)
    }

    private func next(_ steps: ArraySlice<(URL, [String])>, folder: URL, done: @escaping () -> Void) {
        guard let (exe, args) = steps.first else {
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

// same as phonepad.py: the address of the interface the default route leaves from, nothing is sent
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

let phonePadHost = "http://\((SCDynamicStoreCopyLocalHostName(nil) as String?) ?? "this-mac").local:8088"

// started right before the exec into the game, which keeps our pid, so the pad stops by itself once the game quits
func startPhonePad(inTerminal: Bool) {
    guard let script = Bundle.main.url(forResource: "phonepad", withExtension: nil)?.appendingPathComponent("phonepad.py") else { return }
    let args = ["/usr/bin/python3", "-I", script.path, "--exit-with-pid", String(getpid())]
    let p = Process()
    if inTerminal {
        // a .command file opens in Terminal without asking for automation access, -g keeps it behind the game
        let cmd = supportURL.appendingPathComponent("phonepad.command")
        let line = args.map { "'" + $0.replacingOccurrences(of: "'", with: "'\\''") + "'" }.joined(separator: " ")
        try? "#!/bin/sh\nclear\nexec \(line)\n".write(to: cmd, atomically: true, encoding: .utf8)
        chmod(cmd.path, 0o755)
        p.executableURL = URL(fileURLWithPath: "/usr/bin/open")
        p.arguments = ["-g", "-a", "Terminal", cmd.path]
    } else {
        p.executableURL = URL(fileURLWithPath: args[0])
        p.arguments = Array(args.dropFirst())
    }
    try? p.run()
    if inTerminal { p.waitUntilExit() }
}

func launch(_ s: Settings) -> String {
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
    if s.phonePads && s.browserPads { startPhonePad(inTerminal: s.padTerminal) }
    let game = tool("nxhost", in: s.folderURL).path
    var argv: [UnsafeMutablePointer<CChar>?] = ([game] + s.arguments).map { strdup($0) } + [nil]
    execv(game, &argv)
    return "Could not start the game (\(String(cString: strerror(errno))))"
}

struct LauncherView: View {
    @StateObject private var s = Settings()
    @StateObject private var sys = SystemInfo()
    @StateObject private var prep = Preparer()
    @State private var launchError: String?
    @State private var tick = 0
    @State private var copied: String?

    var body: some View {
        let list = checks(s)
        let ready = list.allSatisfy { $0.ok || $0.optional }
        VStack(spacing: 16) {
            HStack(spacing: 12) {
                Image(nsImage: NSApp.applicationIconImage).resizable().frame(width: 48, height: 48)
                Text("Mush8").font(.title2.weight(.semibold))
                Spacer()
            }
            Form {
                if !ready { dataSection(list) }
                settingsSection
                systemSection
                if ready { dataSection(list) }
            }
            .formStyle(.grouped)
            if let e = prep.error ?? launchError { Text(e).foregroundStyle(.red).font(.callout) }
            if prep.running {
                ProgressView().controlSize(.small)
                Text(prep.line).font(.caption.monospaced()).lineLimit(1).truncationMode(.middle)
            } else if ready {
                Button(action: play) { Text("Play").font(.title3.weight(.semibold)).frame(maxWidth: .infinity) }
                    .controlSize(.large).buttonStyle(.borderedProminent).keyboardShortcut(.defaultAction)
            } else {
                Button(action: prepare) { Text("Prepare game data").frame(maxWidth: .infinity) }
                    .controlSize(.large).buttonStyle(.borderedProminent)
                    .disabled(s.folder.isEmpty || !list.filter { ["Console Keys", "Base Game", "Update", "Firmware"].contains($0.id) }.allSatisfy(\.ok))
                Text("Compiles the game's shaders AOT (may take a few mins)")
                    .font(.caption).foregroundStyle(.secondary).multilineTextAlignment(.center)
            }
            Text("proudly made by c22dev").font(.caption).foregroundStyle(.secondary)
        }
        .padding(20)
        .frame(width: 520, height: 770)
    }

    func dataSection(_ list: [Check]) -> some View {
        Section("Game data") {
            HStack {
                Text(s.folder.isEmpty ? "No folder chosen" : s.folder).lineLimit(1).truncationMode(.head)
                    .foregroundStyle(s.folder.isEmpty ? .secondary : .primary)
                Spacer()
                Button("Choose…", action: chooseFolder)
            }
            ForEach(list) { c in
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

    func caption(_ text: String) -> some View {
        Text(text).font(.caption).foregroundStyle(.secondary)
    }

    var settingsSection: some View {
        Section("Settings") {
            Picker("Console mode", selection: Binding(get: { s.handheld ? 2 : s.uhd ? 1 : 0 }, set: { s.handheld = $0 == 2; s.uhd = $0 == 1 })) {
                Text("Docked (1080p)").tag(0)
                Text("Docked (4K)").tag(1)
                Text("Handheld (720p)").tag(2)
            }
            Toggle("60 fps split screen", isOn: $s.split60)
            caption("3 or 4 players get every screen drawn each frame, needs a fast GPU")
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
            caption("Fill screen widens the race to the whole display, menus and HUD keep their proportions")
            Picker("Multiplayer", selection: $s.network) {
                Text("Off").tag("off")
                Text("LAN").tag("lan")
                Text("Online").tag("lanplay")
                Text("Wireless (nearby)").tag("wireless")
            }
            switch s.network {
            case "lan":
                Picker("Network", selection: $s.lanInterface) {
                    Text("Automatic").tag("")
                    ForEach(sys.nets, id: \.self) { n in Text("\(n.name) · \(n.ip)").tag(n.name) }
                }
                caption("Switch and Mush8 players on the same network can join")
            case "lanplay":
                TextField("Server (host:port)", text: $s.lanPlayServer)
                caption("Lan-Play.com clients on the same server can join")
            case "wireless":
                caption("Nearby Mush8 clients can join (keep Wi-Fi on)")
            default:
                EmptyView()
            }
            Toggle("Pipeline archive", isOn: $s.pipelineArchive)
            Toggle("Prewarm pipelines", isOn: $s.prewarm)
            Toggle("Background shader compiling", isOn: $s.backgroundCompile)
            caption("Builds the game's graphics pipelines in the background at startup")
            Toggle("Vibration", isOn: $s.rumble)
            Toggle("Motion controls", isOn: $s.motion)
            Toggle("Spatial audio", isOn: $s.spatial)
            caption("Plays the game's 5.1 mix in spatial audio. Choose Fixed or Head Tracked for AirPods in Control Center")
            Toggle("iPhone controllers", isOn: $s.phonePads)
            caption("iPhones on this network join as players with Mush8's \"Use as a controller\"")
            if s.phonePads {
                Toggle("Browser controllers", isOn: $s.browserPads)
                if s.browserPads {
                    addressRow("Phone address", phonePadHost)
                    addressRow("Or by IP", sys.lan.map { "http://\($0):8088" })
                    caption("Open one in the phone's browser on the same Wi-Fi, it joins as a player, no app needed. The name stays the same, the IP can change")
                    Toggle("Show pad events in Terminal", isOn: $s.padTerminal)
                    caption(s.padTerminal ? "A Terminal window behind the game lists the phones joining and leaving" : "The pad's messages go to the Mush8 log")
                } else {
                    caption("Any phone's browser can be a controller, no app needed")
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

    func prepare() {
        s.save()
        prep.run(s) { tick += 1 }
    }

    func play() {
        if s.network == "lanplay" && s.lanPlayServer.trimmingCharacters(in: .whitespaces).isEmpty {
            launchError = "Enter a LAN-Play server (host:port)"
            return
        }
        s.save()
        launchError = launch(s)
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
