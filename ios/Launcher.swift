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

let docsURL = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
let configURL = docsURL.appendingPathComponent("config.json")

final class Settings: ObservableObject {
    @Published var network = "off"
    @Published var language = "system"
    @Published var lanPlayServer = ""
    @Published var prewarm = true
    @Published var rumble = true
    @Published var backgroundCompile = true
    @Published var motion = false
    @Published var handheld = true
    @Published var phonePads = false
    var extra: [String: Any] = [:]
    private var autosave: AnyCancellable?

    init() {
        defer {
            autosave = objectWillChange.debounce(for: .milliseconds(150), scheduler: RunLoop.main).sink { [weak self] in try? self?.save() }
        }
        guard let data = try? Data(contentsOf: configURL),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return }
        extra = json
        language = json["language"] as? String ?? "system"
        network = json["network"] as? String ?? ((json["lan"] as? Bool ?? false) ? "lan" : "off")
        lanPlayServer = json["lanplay_server"] as? String ?? ""
        prewarm = json["prewarm_pipelines"] as? Bool ?? true
        rumble = json["rumble"] as? Bool ?? true
        backgroundCompile = json["background_compile"] as? Bool ?? true
        motion = json["motion_controls"] as? Bool ?? false
        handheld = json["handheld"] as? Bool ?? true
        phonePads = json["phone_pads"] as? Bool ?? false
    }

    func save() throws {
        var json = extra
        json["network"] = network
        json["language"] = language
        json["lanplay_server"] = lanPlayServer
        json["lan"] = nil
        json["extra_pads"] = nil
        json["prewarm_pipelines"] = prewarm
        json["rumble"] = rumble
        json["background_compile"] = backgroundCompile
        json["motion"] = nil
        json["motion_controls"] = motion
        json["handheld"] = handheld
        json["phone_pads"] = phonePads
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
        let url = docsURL.appendingPathComponent(name)
        var directory: ObjCBool = false
        if FileManager.default.fileExists(atPath: url.path, isDirectory: &directory) {
            return !directory.boolValue || !((try? FileManager.default.contentsOfDirectory(at: url, includingPropertiesForKeys: nil)) ?? []).isEmpty
        }
        return name == "base.nsp" && FileManager.default.fileExists(atPath: docsURL.appendingPathComponent("base.nca").path)
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
    GameFile(id: "Update", name: "upd4-0-1.nsp"),
    GameFile(id: "Booster Course Pass", name: "dlc.nsp", optional: true),
    GameFile(id: "Console Keys", name: "prod.keys"),
    GameFile(id: "Firmware", name: "Firmware", optional: true),
]

final class Files: ObservableObject {
    static let shared = Files()
    @Published var present: [String: Bool] = [:]
    @Published var copying: String?
    @Published var error: String?

    var ready: Bool { gameFiles.allSatisfy { $0.optional || present[$0.name] == true } }

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
            case "update": return "upd4-0-1.nsp"
            case "dlc": return "dlc.nsp"
            case "other": return url.lastPathComponent
            default: break
            }
            if n.contains("upd") || n.contains("update") || n.contains("v4") { return "upd4-0-1.nsp" }
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
    @State private var picking = false
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
                Button("Use as a controller for a Mac") {
                    try? s.save()
                    network.stop()
                    controller()
                }
                .font(.callout)
                Text("proudly made by c22dev").font(.caption).foregroundStyle(.secondary)
            }
            .padding(24)
            .frame(maxWidth: .infinity)
            Form {
                if !files.ready { gameFilesSection }
                Section("游戏语言 / Game language") {
                    Picker("Language", selection: $s.language) {
                        Text("跟随系统 / System").tag("system")
                        Text("简体中文").tag("zh-Hans")
                        Text("繁體中文").tag("zh-Hant")
                        Text("English (US)").tag("en-US")
                        Text("English (UK)").tag("en-GB")
                        Text("日本語").tag("ja")
                        Text("한국어").tag("ko")
                        Text("Français").tag("fr")
                        Text("Français (Canada)").tag("fr-CA")
                        Text("Deutsch").tag("de")
                        Text("Italiano").tag("it")
                        Text("Español").tag("es")
                        Text("Español (Latinoamérica)").tag("es-419")
                        Text("Nederlands").tag("nl")
                        Text("Português (Portugal)").tag("pt-PT")
                        Text("Русский").tag("ru")
                    }
                    caption("仅更改游戏内语言，下次点 Play 生效。Launcher language is unchanged.")
                }
                Section("Multiplayer") {
                    Picker("Mode", selection: $s.network) {
                        Text("Off").tag("off")
                        Text("LAN").tag("lan")
                        Text("Online").tag("lanplay")
                        Text("Wireless (nearby)").tag("wireless")
                    }
                    switch s.network {
                    case "lan":
                        caption(broadcastAvailable()
                                ? "Switch and Mush8 players on the same network can join"
                                : "Mush8 players on the same network can join")
                    case "lanplay":
                        TextField("Server (host:port)", text: $s.lanPlayServer)
                            .textInputAutocapitalization(.never).autocorrectionDisabled().keyboardType(.URL)
                        caption("Lan-Play.com clients on the same server can join")
                    case "wireless":
                        caption("Nearby Mush8 clients can join (keep Wi-Fi on)")
                    default:
                        EmptyView()
                    }
                    if network.denied && usesLocalNetwork {
                        Text("Local network access was denied : other players can't be reached")
                            .font(.caption).foregroundStyle(.red)
                        Button("Turn it on in Settings") {
                            if let url = URL(string: UIApplication.openSettingsURLString) { UIApplication.shared.open(url) }
                        }.font(.caption)
                    }
                    Toggle("Vibration", isOn: $s.rumble)
                    Toggle("Motion controls", isOn: $s.motion)
                }
                Section {
                    Toggle("iPhone controllers", isOn: $s.phonePads)
                } footer: {
                    Text("Nearby iPhones can join as players with \"Use as a controller\"")
                }
                Section {
                    Picker("Console mode", selection: $s.handheld) {
                        Text("Docked (1080p)").tag(false)
                        Text("Handheld (720p)").tag(true)
                    }
                } footer: {
                    Text("Handheld renders at 720p: lighter on the GPU and battery")
                }
                Section {
                    Toggle("Prewarm pipelines", isOn: $s.prewarm)
                    Toggle("Background shader compiling", isOn: $s.backgroundCompile)
                } footer: {
                    Text("Builds the game's graphics pipelines in the background at startup")
                }
                Section {
                    HStack {
                        Text("Memory available")
                        Spacer()
                        VStack(alignment: .trailing) {
                            Text(ByteCountFormatter.string(fromByteCount: Int64(memory), countStyle: .memory))
                                .foregroundStyle(.secondary).monospacedDigit()
                            if memory < 4 << 30 {
                                Text("4 GB is recommended: close other apps").font(.caption).foregroundStyle(.orange)
                            }
                        }
                    }
                }
                if files.ready { gameFilesSection }
            }
            .frame(maxWidth: .infinity)
        }
        .preferredColorScheme(.dark)
        .fileImporter(isPresented: $picking, allowedContentTypes: [.data, .folder], allowsMultipleSelection: true) { result in
            if case .success(let urls) = result { files.copy(urls) }
        }
        .onAppear {
            files.scan()
            if usesLocalNetwork { network.request() }
        }
        .onChange(of: s.network) { if usesLocalNetwork { network.request() } }
        .onChange(of: s.phonePads) { if usesLocalNetwork { network.request() } }
        .onChange(of: phase) { _, p in if p == .active { files.scan() } }
        .task {
            while !Task.isCancelled {
                memory = os_proc_available_memory()
                try? await Task.sleep(for: .seconds(2))
            }
        }
    }

    var gameFilesSection: some View {
        Section("Game files") {
            ForEach(gameFiles) { f in
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
                Button("Import game files") { picking = true }
                    .controlSize(.large)
                    .buttonStyle(.borderedProminent)
                Text("Import prod.keys, the base game and update .nsp files (+DLC/Firmware)/put them in On My iPhone")
                    .font(.caption).foregroundStyle(.secondary).multilineTextAlignment(.center)
            }
            if let e = files.error { Text(e).foregroundStyle(.red).font(.callout).multilineTextAlignment(.center) }
        }
    }

    @ViewBuilder var playButton: some View {
        let button = Button(action: start) {
            Text("Play").font(.title3.weight(.semibold)).frame(maxWidth: 220)
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

    var usesLocalNetwork: Bool { s.network == "lan" || s.network == "wireless" || s.phonePads }

    func caption(_ text: String) -> some View {
        Text(text).font(.caption).foregroundStyle(.secondary)
    }

    func start() {
        if s.network == "lanplay" && s.lanPlayServer.trimmingCharacters(in: .whitespaces).isEmpty {
            error = "Enter a LAN-Play server (host:port)"
            return
        }
        do { try s.save() } catch { self.error = "Could not save settings: \(error.localizedDescription)"; return }
        network.stop()
        play()
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
