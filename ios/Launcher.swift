// Mush8 - Launcher.swift
// Constantin Clerc (c22dev) - 0.1
import Combine
import Network
import SwiftUI
import UIKit
import UniformTypeIdentifiers
import os

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

func lanIP() -> String? {
    var list: UnsafeMutablePointer<ifaddrs>?
    guard getifaddrs(&list) == 0, let first = list else { return nil }
    defer { freeifaddrs(list) }
    var found: [String: String] = [:]
    for p in sequence(first: first, next: { $0.pointee.ifa_next }) {
        let a = p.pointee
        guard let addr = a.ifa_addr, addr.pointee.sa_family == sa_family_t(AF_INET), a.ifa_flags & UInt32(IFF_UP) != 0 else { continue }
        let name = String(cString: a.ifa_name)
        let ip = addr.withMemoryRebound(to: sockaddr_in.self, capacity: 1) { String(cString: inet_ntoa($0.pointee.sin_addr)) }
        if found[name] == nil { found[name] = ip }
    }
    if let wifi = found["en0"] { return wifi }
    return found.filter { $0.key.hasPrefix("bridge") }.sorted { $0.key < $1.key }.first?.value
}

let docsURL = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
let configURL = docsURL.appendingPathComponent("config.json")

final class Settings: ObservableObject {
    @Published var network = "off"
    @Published var lanPlayServer = ""
    @Published var nextendoPid: UInt64 = 0
    @Published var nextendoName = ""
    @Published var nickname = ""
    @Published var nicknameSource = "nextendo"
    @Published var country = ""
    @Published var prewarm = true
    @Published var rumble = true
    @Published var rumbleMin = 0.45
    @Published var spatial = true
    @Published var backgroundCompile = true
    @Published var motion = false
    @Published var touchscreen = true
    @Published var handheld = true
    @Published var phonePads = false
    @Published var browserPads = false
    @Published var fillScreen = true
    @Published var hudAtEdges = true
    @Published var hudScale = 1.0
    @Published var disabledMods: [String] = []
    var extra: [String: Any] = [:]
    private var autosave: AnyCancellable?

    init() {
        defer {
            autosave = objectWillChange.debounce(for: .milliseconds(150), scheduler: RunLoop.main).sink { [weak self] in try? self?.save() }
        }
        guard let data = try? Data(contentsOf: configURL),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return }
        extra = json
        network = json["network"] as? String ?? ((json["lan"] as? Bool ?? false) ? "lan" : "off")
        lanPlayServer = json["lanplay_server"] as? String ?? ""
        nextendoPid = (json["nextendo_pid"] as? NSNumber)?.uint64Value ?? 0
        nextendoName = json["nextendo_name"] as? String ?? ""
        nickname = json["nickname"] as? String ?? ""
        nicknameSource = json["nickname_source"] as? String ?? nicknameSource
        country = json["country"] as? String ?? ""
        prewarm = json["prewarm_pipelines"] as? Bool ?? true
        rumble = json["rumble"] as? Bool ?? true
        rumbleMin = json["rumble_min"] as? Double ?? 0.45
        spatial = json["spatial_audio"] as? Bool ?? true
        backgroundCompile = json["background_compile"] as? Bool ?? true
        motion = json["motion_controls"] as? Bool ?? false
        touchscreen = json["touchscreen"] as? Bool ?? true
        handheld = json["handheld"] as? Bool ?? true
        phonePads = json["phone_pads"] as? Bool ?? false
        browserPads = json["browser_pads"] as? Bool ?? false
        fillScreen = json["aspect"] as? String != "16:9"
        hudAtEdges = json["hud"] as? String != "center"
        hudScale = json["hud_scale"] as? Double ?? 1.0
        disabledMods = json["disabled_mods"] as? [String] ?? []
    }

    func save() throws {
        var json = extra
        json["network"] = network
        json["lanplay_server"] = lanPlayServer
        json["nextendo_pid"] = NSNumber(value: nextendoPid)
        json["nextendo_name"] = nextendoName
        json["nickname"] = nickname
        json["nickname_source"] = nicknameSource
        json["country"] = country
        json["player_name"] = playerNickname(nextendo: network == "nextendo", source: nicknameSource, custom: nickname, nextendoName: nextendoName)
        json["player_country"] = playerCountry(nextendo: network == "nextendo", country: country)
        json["nextendo_token"] = nil
        json["lan"] = nil
        json["extra_pads"] = nil
        json["prewarm_pipelines"] = prewarm
        json["rumble"] = rumble
        json["rumble_min"] = rumbleMin
        json["spatial_audio"] = spatial
        json["background_compile"] = backgroundCompile
        json["motion"] = nil
        json["motion_controls"] = motion
        json["touchscreen"] = touchscreen
        json["handheld"] = handheld
        json["phone_pads"] = phonePads
        json["browser_pads"] = browserPads
        json["aspect"] = fillScreen ? "fill" : "16:9"
        json["hud"] = hudAtEdges ? "edges" : "center"
        json["hud_scale"] = hudScale
        json["disabled_mods"] = disabledMods
        let data = try JSONSerialization.data(withJSONObject: json, options: [.prettyPrinted, .sortedKeys])
        try data.write(to: configURL)
    }
}

final class LocalNetwork: ObservableObject {
    @Published var denied = false
    private var browser: NWBrowser?

    func request() {
        guard browser == nil else { return }
        let b = NWBrowser(for: .bonjour(type: "_mush8._tcp", domain: nil), using: NWParameters())
        b.stateUpdateHandler = { [weak self] state in
            var denied = false
            if case .waiting(let e) = state, case .dns(let code) = e { denied = code == -65570 }
            if case .failed(let e) = state, case .dns(let code) = e { denied = code == -65570 }
            DispatchQueue.main.async { self?.denied = denied }
        }
        b.start(queue: .main)
        browser = b
    }

    func stop() {
        browser?.cancel()
        browser = nil
    }
}

struct GameFile: Identifiable {
    let id: String
    let name: String
    var optional = false
    var present: Bool {
        let path = docsURL.appendingPathComponent(name).path
        var dir: ObjCBool = false
        guard FileManager.default.fileExists(atPath: path, isDirectory: &dir) else { return false }
        return !dir.boolValue || !((try? FileManager.default.contentsOfDirectory(atPath: path)) ?? []).isEmpty
    }

    func status(_ found: Bool) -> String {
        guard found else { return optional ? "none" : "missing" }
        let attrs = try? FileManager.default.attributesOfItem(atPath: docsURL.appendingPathComponent(name).path)
        guard attrs?[.type] as? FileAttributeType != .typeDirectory, let size = attrs?[.size] as? Int64 else { return "found" }
        return ByteCountFormatter.string(fromByteCount: size, countStyle: .file)
    }
}

func broadcastAvailable() -> Bool {
    guard let sec = dlopen("/System/Library/Frameworks/Security.framework/Security", RTLD_NOW) else { return false }
    defer { dlclose(sec) }
    typealias Create = @convention(c) (UnsafeMutableRawPointer?) -> UnsafeMutableRawPointer?
    typealias CopyEnt = @convention(c) (UnsafeMutableRawPointer?, UnsafeRawPointer?, UnsafeMutableRawPointer?) -> UnsafeMutableRawPointer?
    guard let createSym = dlsym(sec, "SecTaskCreateFromSelf"), let copySym = dlsym(sec, "SecTaskCopyValueForEntitlement") else { return false }
    let create = unsafeBitCast(createSym, to: Create.self)
    let copy = unsafeBitCast(copySym, to: CopyEnt.self)
    guard let task = create(nil) else { return false }
    defer { Unmanaged<CFTypeRef>.fromOpaque(task).release() }
    let key = Unmanaged.passUnretained("com.apple.developer.networking.multicast" as CFString).toOpaque()
    guard let value = copy(task, key, nil) else { return false }
    defer { Unmanaged<CFTypeRef>.fromOpaque(value).release() }
    let object = Unmanaged<CFTypeRef>.fromOpaque(value).takeUnretainedValue()
    guard CFGetTypeID(object) == CFBooleanGetTypeID() else { return false }
    return CFBooleanGetValue(Unmanaged<CFBoolean>.fromOpaque(value).takeUnretainedValue())
}

let gameFiles = [
    GameFile(id: "Base Game", name: "base.nsp"),
    GameFile(id: "Update", name: "update.nsp"),
    GameFile(id: "Booster Course Pass", name: "dlc.nsp", optional: true),
    GameFile(id: "Console Keys", name: "prod.keys"),
    GameFile(id: "Firmware", name: "Firmware"),
]

final class Files: ObservableObject {
    static let shared = Files()
    @Published var present: [String: Bool] = [:]
    @Published var copying: String?
    @Published var error: String?

    var ready: Bool { prepared || gameFiles.allSatisfy { $0.optional || present[$0.name] == true } }
    var prepared: Bool { FileManager.default.fileExists(atPath: docsURL.appendingPathComponent("prepared/romfs.bin").path) }

    func scan() {
        let fm = FileManager.default
        for n in (try? fm.contentsOfDirectory(atPath: docsURL.path)) ?? [] where n.lowercased().hasSuffix(".nsp") && !gameFiles.contains(where: { $0.name == n }) {
            let url = docsURL.appendingPathComponent(n)
            let dest = docsURL.appendingPathComponent(Files.target(for: url))
            if dest != url && !fm.fileExists(atPath: dest.path) { try? fm.moveItem(at: url, to: dest) }
        }
        present = Dictionary(uniqueKeysWithValues: gameFiles.map { ($0.name, $0.present) })
    }

    func copy(_ urls: [URL]) {
        error = nil
        DispatchQueue.global(qos: .userInitiated).async {
            for url in urls {
                let scoped = url.startAccessingSecurityScopedResource()
                defer { if scoped { url.stopAccessingSecurityScopedResource() } }
                let name = Files.target(for: url)
                DispatchQueue.main.async { self.copying = name }
                let dest = docsURL.appendingPathComponent(name)
                do {
                    if FileManager.default.fileExists(atPath: dest.path) { try FileManager.default.removeItem(at: dest) }
                    try FileManager.default.copyItem(at: url, to: dest)
                } catch {
                    DispatchQueue.main.async { self.error = "\(url.lastPathComponent): \(error.localizedDescription)" }
                }
            }
            DispatchQueue.main.async {
                self.copying = nil
                self.scan()
            }
        }
    }

    static func target(for url: URL) -> String {
        let n = url.lastPathComponent.lowercased()
        if n.hasSuffix(".keys") { return "prod.keys" }
        if url.hasDirectoryPath { return "Firmware" }
        if n.hasSuffix(".nsp") {
            switch nspKind(url) {
            case "base": return "base.nsp"
            case "update": return "update.nsp"
            case "dlc": return "dlc.nsp"
            case "other": return url.lastPathComponent
            default: break
            }
            if n.contains("upd") || n.contains("update") { return "update.nsp" }
            if n.contains("dlc") || n.contains("booster") || n.contains("aoc") { return "dlc.nsp" }
            if n.contains("base") || n.contains("mario") { return "base.nsp" }
        }
        return url.lastPathComponent
    }
}

struct LauncherView: View {
    @StateObject private var s = Settings()
    @ObservedObject private var files = Files.shared
    @StateObject private var network = LocalNetwork()
    @State private var error: String?
    @State private var launching = false
    @State private var shownRefusal: String?
    @State private var picking = false
    @State private var browsingMods = false
    @State private var inspectedMod: Mod?
    @State private var deletingMod: Mod?
    @State private var modActionError: String?
    @State private var mods: [Mod] = []
    @State private var pickingMiis = false
    @State private var miiCount = 0
    @State private var miiError: String?
    @State private var memory = os_proc_available_memory()
    @Environment(\.scenePhase) private var phase
    let play: () -> Void
    let controller: () -> Void

    var body: some View {
        HStack(spacing: 0) {
            VStack(spacing: 16) {
                Text("Mush8").font(.largeTitle.weight(.bold)).multilineTextAlignment(.center)
                if files.ready && files.copying == nil {
                    if let error { Text(error).foregroundStyle(.red).font(.callout).multilineTextAlignment(.center) }
                    playButton
                } else {
                    importView
                }
                Button("Use as a controller") {
                    network.stop()
                    controller()
                }
                .font(.callout)
                Text("proudly made by c22dev").font(.caption).foregroundStyle(.secondary)
            }
            .padding(24)
            .frame(maxWidth: .infinity)
            NavigationStack {
                Form {
                    if !files.ready { gameFilesSection }
                    Section {
                        NavigationLink { multiplayerPage } label: {
                            CategoryLabel(title: "Multiplayer", icon: "person.2.fill", color: .green, value: modeName)
                        }
                        NavigationLink { modsPage } label: {
                            CategoryLabel(title: "Mods", icon: "puzzlepiece.extension.fill", color: .orange,
                                          value: mods.isEmpty ? nil : "\(mods.filter { !s.disabledMods.contains($0.id) }.count)/\(mods.count)")
                        }
                        NavigationLink { playerPage } label: {
                            CategoryLabel(title: "Player", icon: "person.crop.circle.fill", color: .purple, value: playerName)
                        }
                    }
                    Section {
                        NavigationLink { graphicsPage } label: {
                            CategoryLabel(title: "Graphics", icon: "display", color: .blue, value: s.handheld ? "Handheld" : "Docked")
                        }
                        NavigationLink { controlsPage } label: {
                            CategoryLabel(title: "Audio & Controls", icon: "gamecontroller.fill", color: .pink, value: nil)
                        }
                    }
                    Section {
                        NavigationLink { devicePage } label: {
                            CategoryLabel(title: "Device", icon: "iphone", color: memory < 7 << 29 ? .orange : .gray,
                                          value: ByteCountFormatter.string(fromByteCount: Int64(memory), countStyle: .memory))
                        }
                    }
                }
                .scrollEdgeEffectStyle(.soft, for: .top)
                .toolbar(.hidden, for: .navigationBar)
            }
            .frame(maxWidth: .infinity)
        }
        .preferredColorScheme(.dark)
        .fileImporter(isPresented: $picking, allowedContentTypes: pickingMiis ? [.data] : [.data, .folder], allowsMultipleSelection: true) { result in
            guard case .success(let urls) = result else { return }
            if pickingMiis {
                miiError = nil
                do { try importMiis(urls, into: docsURL) } catch { miiError = error.localizedDescription }
                miiCount = installedMiiCount(in: docsURL)
            } else {
                files.copy(urls)
            }
        }
        .fullScreenCover(isPresented: $browsingMods, onDismiss: { mods = installedMods(in: docsURL) }) {
            ModBrowserView(folder: docsURL) { name in
                s.disabledMods.removeAll { $0 == name }
                do { try s.save() } catch { self.error = "Could not save settings: \(error.localizedDescription)" }
                mods = installedMods(in: docsURL)
            }
        }
        .modActions(folder: docsURL, inspected: $inspectedMod, deleting: $deletingMod, error: $modActionError, delete: deleteMod)
        .onAppear {
            files.scan()
            mods = installedMods(in: docsURL)
            miiCount = installedMiiCount(in: docsURL)
            if usesLocalNetwork { network.request() }
        }
        .onChange(of: s.network) { if usesLocalNetwork { network.request() } }
        .onChange(of: s.phonePads) { if usesLocalNetwork { network.request() } }
        .onChange(of: phase) { _, p in
            if p == .active {
                files.scan()
                mods = installedMods(in: docsURL)
                miiCount = installedMiiCount(in: docsURL)
            }
        }
        .task {
            while !Task.isCancelled {
                memory = os_proc_available_memory()
                try? await Task.sleep(for: .seconds(2))
            }
        }
    }

    var modeName: String {
        switch s.network {
        case "lan": return "LAN"
        case "lanplay": return "Online"
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
                    Text("Online").tag("lanplay")
                    Text("Wireless (nearby)").tag("wireless")
                    Text("Nextendo").tag("nextendo")
                }
                if s.network == "nextendo" {
                    NextendoSignIn(pid: $s.nextendoPid, name: $s.nextendoName)
                }
                if s.network == "lanplay" {
                    TextField("Server (host:port)", text: $s.lanPlayServer)
                        .textInputAutocapitalization(.never).autocorrectionDisabled().keyboardType(.URL)
                }
                if network.denied && usesLocalNetwork {
                    Text("Local network access was denied : other players can't be reached")
                        .font(.caption).foregroundStyle(.red)
                    Button("Turn it on in Settings") {
                        if let url = URL(string: UIApplication.openSettingsURLString) { UIApplication.shared.open(url) }
                    }.font(.caption)
                }
            } footer: {
                if let note = multiplayerNote { Text(note) }
            }
            Section {
                Toggle("Remote controllers", isOn: $s.phonePads)
                if s.phonePads {
                    Toggle("Web controllers", isOn: $s.browserPads)
                    if s.browserPads {
                        LabeledContent("Address", value: lanIP().map { "http://\($0):2904" } ?? "no Wi-Fi")
                            .textSelection(.enabled)
                    }
                }
            } footer: {
                Text(s.phonePads && s.browserPads
                    ? "Phones on the same Wi-Fi open this in a browser after Play"
                    : "Nearby iPhones join with \"Use as a controller\"")
            }
        }
    }

    var modsPage: some View {
        CategoryPage(title: "Mods") {
            Section {
                Button {
                    browsingMods = true
                } label: {
                    Label("Browse GameBanana…", systemImage: "globe").fontWeight(.medium)
                }
                .disabled(files.copying != nil)
            }
            Section {
                if mods.isEmpty {
                    Text("No mods installed.").foregroundStyle(.secondary)
                }
                ForEach(mods) { mod in
                    ModRow(mod: mod, disabled: $s.disabledMods, inspect: { inspectedMod = mod }, delete: { deletingMod = mod })
                }
            } header: {
                ModsHeader(mods: mods, disabled: s.disabledMods)
            } footer: {
                Text("Swipe to delete, long press for files. Lower mods win conflicts. Code patches don't run on iOS")
            }
        }
    }

    var graphicsPage: some View {
        CategoryPage(title: "Graphics") {
            Section {
                Picker("Console mode", selection: $s.handheld) {
                    Text("Docked (1080p)").tag(false)
                    Text("Handheld (720p)").tag(true)
                }
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
                Text("Handheld is lighter on the GPU and battery")
            }
            Section {
                Toggle("Prewarm pipelines", isOn: $s.prewarm)
                Toggle("Background shader compiling", isOn: $s.backgroundCompile)
            } footer: {
                Text("Builds graphics pipelines at startup")
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
                    HStack {
                        Text("Rumble cutoff")
                        Slider(value: $s.rumbleMin, in: 0...0.9, step: 0.05)
                        Text("\(Int((s.rumbleMin * 100).rounded())) %").monospacedDigit().foregroundStyle(.secondary)
                    }
                }
            } footer: {
                if s.rumble { Text("Weaker rumble is ignored, which stops the buzz on Xbox and PlayStation controllers. Phone and Switch controllers aren't affected") }
            }
            Section {
                Toggle("Motion controls", isOn: $s.motion)
                Toggle("Touchscreen", isOn: $s.touchscreen)
            } footer: {
                Text("Turn off Touchscreen to stop taps on the race reaching the game")
            }
        }
    }

    var devicePage: some View {
        CategoryPage(title: "Device") {
            Section {
                HStack {
                    Text("Memory available")
                    Spacer()
                    VStack(alignment: .trailing) {
                        Text(ByteCountFormatter.string(fromByteCount: Int64(memory), countStyle: .memory))
                            .foregroundStyle(.secondary).monospacedDigit()
                        if memory < 7 << 29 {
                            Text("3.5 GB is recommended: close other apps").font(.caption).foregroundStyle(.orange)
                        }
                    }
                }
            }
            gameFilesSection
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
                Button("Import Miis…") {
                    pickingMiis = true
                    picking = true
                }
                    .disabled(files.copying != nil)
                if miiCount > 0 {
                    Button("Remove custom Miis", role: .destructive) {
                        miiError = nil
                        do { try removeMiis(in: docsURL) } catch { miiError = error.localizedDescription }
                        miiCount = installedMiiCount(in: docsURL)
                    }
                }
                if let miiError { Text(miiError).foregroundStyle(.red).font(.callout) }
            } header: {
                if miiCount > 0 { Text("\(miiCount) custom Mii\(miiCount == 1 ? "" : "s")") }
            } footer: {
                Text("Shown on your ghosts and online, 10 characters max. The country sets your Nextendo flag. Miis: a MiiDatabase.dat, .charinfo or .storedata")
            }
        }
    }

    func deleteMod(_ mod: Mod) {
        deletingMod = nil
        do {
            try deleteInstalledMod(mod, in: docsURL)
            s.disabledMods.removeAll { $0 == mod.id }
            mods = installedMods(in: docsURL)
            try s.save()
        } catch { modActionError = error.localizedDescription }
    }

    var gameFilesSection: some View {
        Section("Game files") {
            if files.prepared {
                HStack {
                    Image(systemName: "checkmark.circle.fill").foregroundStyle(.green)
                    Text("Prepared game data")
                }
            }
            ForEach(gameFiles.filter { !files.prepared || !$0.name.hasSuffix(".nsp") || files.present[$0.name] == true }) { f in
                HStack {
                    Image(systemName: files.present[f.name] == true ? "checkmark.circle.fill" : (f.optional ? "minus.circle" : "xmark.circle.fill"))
                        .foregroundStyle(files.present[f.name] == true ? .green : (f.optional ? .secondary : .red))
                    Text(f.id)
                    Spacer()
                    Text(f.status(files.present[f.name] == true)).font(.caption).foregroundStyle(.secondary)
                }
            }
        }
    }

    @ViewBuilder var importView: some View {
        VStack(spacing: 12) {
            if let name = files.copying {
                ProgressView()
                Text("Copying \(name)…").font(.callout)
            } else {
                Button("Import game files") {
                    pickingMiis = false
                    picking = true
                }
                    .controlSize(.large)
                    .buttonStyle(.borderedProminent)
                Text("Import prod.keys, the base game and update .nsp files and the Firmware folder (+DLC)/put them in On My iPhone")
                    .font(.caption).foregroundStyle(.secondary).multilineTextAlignment(.center)
            }
            if let e = files.error { Text(e).foregroundStyle(.red).font(.callout).multilineTextAlignment(.center) }
        }
    }

    @ViewBuilder var playButton: some View {
        let button = Button(action: start) {
            Text("Play").font(.title3.weight(.semibold)).opacity(launching ? 0 : 1)
                .overlay { if launching { ProgressView().tint(.white) } }
                .frame(maxWidth: 220)
        }
        .controlSize(.large)
        #if compiler(>=6.2)
        if #available(iOS 26.0, *) {
            button.buttonStyle(.glassProminent)
        } else {
            button.buttonStyle(.borderedProminent)
        }
        #else
        button.buttonStyle(.borderedProminent)
        #endif
    }

    var multiplayerNote: String? {
        switch s.network {
        case "lan":
            return broadcastAvailable() ? "Switch and Mush8 players on the same network can join" : "Mush8 players on the same network can join"
        case "lanplay":
            return "Lan-Play.com clients on the same server can join"
        case "wireless":
            return "Nearby Mush8 clients can join (keep Wi-Fi on)"
        case "nextendo":
            return "Online with Switch players. Needs a Nextendo account and update 4.0.1"
        default:
            return nil
        }
    }

    var usesLocalNetwork: Bool { s.network == "lan" || s.network == "wireless" || s.phonePads }

    func start() {
        let enabled = installedMods(in: docsURL).filter { !s.disabledMods.contains($0.id) }
        if let mod = enabled.first(where: { !$0.loadable }) {
            error = "\(mod.displayTitle) can't load: \(mod.summary). Turn it off in Mods."
            return
        }
        if let mod = enabled.first(where: { !$0.patches.isEmpty }) {
            error = "\(mod.displayTitle) has code patches, which don't run on iOS. Turn it off in Mods."
            return
        }
        if s.network == "lanplay" && s.lanPlayServer.trimmingCharacters(in: .whitespaces).isEmpty {
            error = "Enter a LAN-Play server (host:port)"
            return
        }
        do { try s.save() } catch { self.error = "Could not save settings: \(error.localizedDescription)"; return }
        guard s.network == "nextendo" else {
            network.stop()
            play()
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
                await Nextendo.syncMii(folder: docsURL, login: login)
                try? s.save()
                setenv("NX_NEXTENDO_TOKEN", login.token, 1)
                setenv("NX_NEXTENDO_ACCESS", login.access, 1)
                let gate = await Nextendo.onlineGate(login.token)
                setenv("NX_NEXTENDO_GATE", gate.log, 1)
                let refusal = gate.problem.map { "\(login.pid) \($0)" }
                if let problem = gate.problem, shownRefusal != refusal {
                    shownRefusal = refusal
                    self.error = problem + " Tap Play again to start anyway: online play won't work until then."
                    return
                }
                shownRefusal = nil
                network.stop()
                play()
            } catch {
                self.error = error.localizedDescription
            }
        }
    }
}

struct CategoryPage<Content: View>: View {
    let title: String
    @ViewBuilder let content: Content
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        Form { content }
            .scrollEdgeEffectStyle(.soft, for: .top)
            .safeAreaInset(edge: .top, spacing: 0) { header }
            .toolbar(.hidden, for: .navigationBar)
            .background(SwipeBackEnabler())
    }

    var header: some View {
        ZStack {
            Text(title).font(.headline)
            HStack {
                Button { dismiss() } label: {
                    Image(systemName: "chevron.left")
                        .font(.system(size: 16, weight: .semibold))
                        .foregroundStyle(.primary)
                        .frame(width: 36, height: 36)
                        .glassEffect(.regular.interactive(), in: .circle)
                        .frame(width: 44, height: 44)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(Text("Back"))
                Spacer()
            }
        }
        .padding(.horizontal, 12)
        .padding(.top, 10)
        .padding(.bottom, 6)
    }
}

private struct SwipeBackEnabler: UIViewControllerRepresentable {
    func makeUIViewController(context: Context) -> Controller { Controller() }
    func updateUIViewController(_ controller: Controller, context: Context) {}

    final class Controller: UIViewController, UIGestureRecognizerDelegate {
        private var previous: [(UIGestureRecognizer, UIGestureRecognizerDelegate?)] = []

        var gestures: [UIGestureRecognizer] {
            [navigationController?.interactivePopGestureRecognizer, navigationController?.interactiveContentPopGestureRecognizer].compactMap { $0 }
        }

        override func viewDidAppear(_ animated: Bool) {
            super.viewDidAppear(animated)
            for pop in gestures where pop.delegate !== self {
                previous.append((pop, pop.delegate))
                pop.delegate = self
                pop.isEnabled = true
            }
        }

        override func viewWillDisappear(_ animated: Bool) {
            super.viewWillDisappear(animated)
            for (pop, delegate) in previous where pop.delegate === self { pop.delegate = delegate }
            previous = []
        }

        func gestureRecognizerShouldBegin(_ gestureRecognizer: UIGestureRecognizer) -> Bool {
            (navigationController?.viewControllers.count ?? 0) > 1
        }

        func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer, shouldReceive touch: UITouch) -> Bool {
            sequence(first: touch.view, next: { $0?.superview }).allSatisfy { !($0 is UISlider) }
        }
    }
}

struct CategoryLabel: View {
    let title: String
    let icon: String
    let color: Color
    let value: String?

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: icon)
                .font(.system(size: 15, weight: .semibold))
                .foregroundStyle(.white)
                .frame(width: 30, height: 30)
                .background(color.gradient, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
            Text(title)
            Spacer()
            if let value { Text(value).foregroundStyle(.secondary) }
        }
    }
}

private final class LauncherDoor {
    var host: UIViewController?
    var done: (() -> Void)?
    func close(then next: @escaping () -> Void) {
        guard done != nil else { return }
        done = nil
        host?.view.isHidden = true
        host?.dismiss(animated: false)
        next()
    }
}

@_cdecl("mush8_show_launcher")
public func mush8ShowLauncher(_ viewController: UnsafeMutableRawPointer, _ play: @convention(c) () -> Void, _ controller: @convention(c) () -> Void) {
    let parent = Unmanaged<UIViewController>.fromOpaque(viewController).takeUnretainedValue()
    let door = LauncherDoor()
    door.done = {}
    let host = UIHostingController(rootView: LauncherView(play: { door.close(then: { play() }) }, controller: { door.close(then: { controller() }) }))
    door.host = host
    host.modalPresentationStyle = .fullScreen
    parent.present(host, animated: false)
}

private final class AccessBox: @unchecked Sendable {
    var value: String?
}

@_cdecl("mush8_nextendo_access")
public func mush8NextendoAccess() -> UnsafeMutablePointer<CChar>? {
    let done = DispatchSemaphore(value: 0)
    let box = AccessBox()
    Task.detached {
        box.value = try? await Nextendo.freshAccess()
        done.signal()
    }
    guard done.wait(timeout: .now() + 30) == .success, let access = box.value, !access.isEmpty else { return nil }
    return strdup(access)
}

@_cdecl("mush8_each_mod")
public func mush8EachMod(_ context: UnsafeMutableRawPointer, _ callback: @convention(c) (UnsafeMutableRawPointer, UnsafePointer<CChar>) -> Void) {
    let json = (try? Data(contentsOf: configURL)).flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] }
    let disabled = json?["disabled_mods"] as? [String] ?? []
    for mod in installedMods(in: docsURL) where !disabled.contains(mod.id) && mod.romfs && mod.loadable {
        docsURL.appendingPathComponent(mod.path).path.withCString { callback(context, $0) }
    }
}
