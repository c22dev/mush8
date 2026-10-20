// MK8 Native local launcher. Uses Mush8's existing phone pad and wireless transports.
import AppKit
import SwiftUI
import Combine
import Network
import UniformTypeIdentifiers

struct LauncherFile: Identifiable {
    let id: String
    let title: String
    let optional: Bool
    let directory: Bool
}

@MainActor final class GameLauncher: ObservableObject {
    @Published var network = "off" { didSet { save() } }
    @Published var lanPlayServer = "" { didSet { save() } }
    @Published var phonePads = true { didSet { save() } }
    @Published var language = "system" { didSet { save() } }
    @Published var prewarm = false { didSet { save() } }
    @Published var backgroundCompile = true { didSet { save() } }
    @Published var rumble = true { didSet { save() } }
    @Published var motion = false { didSet { save() } }
    @Published var handheld = false { didSet { save() } }
    @Published var fileOverrides: [String: [String]] = [:] { didSet { save() } }
    @Published var availableMemory: UInt64 = 0
    @Published var fileStatus: [String: Bool] = [:]
    @Published var controllerStatus = "Razer USB：尚未检测"
    @Published var networkDenied = false
    @Published var errorMessage: String?

    let files = [
        LauncherFile(id: "base", title: "游戏本体 · NSP / Program NCA", optional: false, directory: false),
        LauncherFile(id: "update", title: "4.0.1 更新 · NSP", optional: false, directory: false),
        LauncherFile(id: "dlc", title: "Booster Course Pass · NSP", optional: true, directory: false),
        LauncherFile(id: "keys", title: "主机密钥 · prod.keys", optional: false, directory: false),
        LauncherFile(id: "firmware", title: "固件资源目录 · Firmware", optional: true, directory: true)
    ]
    private let defaults: UserDefaults
    private let config: [String: Any]
    private let originalArguments: [String]
    private var ready = false
    private var browser: NWBrowser?

    init(config supplied: [String: Any]? = nil, defaults: UserDefaults = .standard) {
        self.defaults = defaults
        let configURL = Bundle.main.resourceURL?.appendingPathComponent("launch.json")
        config = supplied ?? configURL.flatMap { try? Data(contentsOf: $0) }
            .flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] } ?? [:]
        originalArguments = config["args"] as? [String] ?? []
        let env = ProcessInfo.processInfo.environment
        network = env["NX_NET"] ?? "off"
        lanPlayServer = env["NX_LANPLAY_SERVER"] ?? ""
        language = env["NX_LANGUAGE"] ?? "system"
        phonePads = env["NX_PHONE_PADS"] != "0"
        prewarm = env["NX_PREWARM"] != nil
        backgroundCompile = env["NX_SYNC_SHADERS"] == nil
        rumble = env["NX_RUMBLE"] != "0"
        motion = env["NX_MOTION"] == "1"
        handheld = env["NX_HANDHELD"] == "1"
        if let saved = defaults.dictionary(forKey: "gameSettings") {
            network = saved["network"] as? String ?? network
            lanPlayServer = saved["lanplay_server"] as? String ?? lanPlayServer
            phonePads = saved["phone_pads"] as? Bool ?? phonePads
            language = saved["language"] as? String ?? language
            prewarm = saved["prewarm_pipelines"] as? Bool ?? prewarm
            backgroundCompile = saved["background_compile"] as? Bool ?? backgroundCompile
            rumble = saved["rumble"] as? Bool ?? rumble
            motion = saved["motion_controls"] as? Bool ?? motion
            handheld = saved["handheld"] as? Bool ?? handheld
            fileOverrides = (saved["files"] as? [String: [String]] ?? [:]).filter { ["base", "update", "dlc", "keys", "firmware"].contains($0.key) }
        }
        ready = true
        refresh()
    }

    var serverAddress: String { lanPlayServer.trimmingCharacters(in: .whitespacesAndNewlines) }
    var needsNetwork: Bool { network != "off" || phonePads }
    var canStart: Bool {
        !originalArguments.isEmpty && (network != "lanplay" || !serverAddress.isEmpty)
            && files.filter { !$0.optional }.allSatisfy { fileStatus[$0.id] == true }
            && fileStatus["runtime"] == true
            && paths(for: "dlc").allSatisfy { validFile($0, directory: false) }
    }
    var workingDirectory: String { config["cwd"] as? String ?? FileManager.default.currentDirectoryPath }
    var networkDescription: String {
        switch network {
        case "lanplay": return "Mac 和 iPhone 都选 Online，填写同一 LAN-Play 服务器地址，再进入游戏中的 Online 菜单。省略端口时使用 11451；当前支持无需登录的服务器。"
        case "wireless": return "附近的 Mac / iPhone 均选择 Wireless，保持 Wi-Fi 开启，再进入游戏中的 Wireless 菜单建房或加入。"
        case "lan": return "两端选择 LAN 并连接同一局域网，再进入游戏中的“局域网”菜单。"
        default: return "联网模式关闭，仍可使用同一 Wi-Fi 上的 iPhone 作为控制器。"
        }
    }

    func save() {
        guard ready else { return }
        var saved = defaults.dictionary(forKey: "gameSettings") ?? [:]
        saved.merge(["network": network, "lanplay_server": serverAddress, "phone_pads": phonePads,
                     "language": language, "prewarm_pipelines": prewarm, "background_compile": backgroundCompile,
                     "rumble": rumble, "motion_controls": motion, "handheld": handheld, "files": fileOverrides]) { _, new in new }
        defaults.set(saved, forKey: "gameSettings")
    }

    private func values(_ flag: String) -> [String] {
        originalArguments.indices.compactMap { i in
            originalArguments[i] == "--\(flag)" && i + 1 < originalArguments.count ? originalArguments[i + 1] : nil
        }
    }
    func paths(for id: String) -> [String] {
        if let override = fileOverrides[id] { return override }
        if id == "base" { return values("base-nca").isEmpty ? values("base") : values("base-nca") }
        return values(id)
    }
    func absolutePath(_ path: String) -> String {
        URL(fileURLWithPath: path, relativeTo: URL(fileURLWithPath: workingDirectory, isDirectory: true)).standardizedFileURL.path
    }
    private func validFile(_ path: String, directory: Bool) -> Bool {
        let path = absolutePath(path)
        var isDirectory: ObjCBool = false
        guard FileManager.default.fileExists(atPath: path, isDirectory: &isDirectory),
              isDirectory.boolValue == directory, FileManager.default.isReadableFile(atPath: path) else { return false }
        if directory { return (try? FileManager.default.contentsOfDirectory(atPath: path).isEmpty) == false }
        return ((try? FileManager.default.attributesOfItem(atPath: path)[.size] as? NSNumber)?.uint64Value ?? 0) > 0
    }
    func refresh() {
        controllerStatus = WolverineUSB.shared.status
        var status: [String: Bool] = [:]
        for file in files {
            let paths = paths(for: file.id)
            status[file.id] = !paths.isEmpty && paths.allSatisfy { validFile($0, directory: file.directory) }
        }
        let moduleDirs = values("patched")
        let npdm = values("npdm")
        status["runtime"] = !moduleDirs.isEmpty && !npdm.isEmpty
            && moduleDirs.allSatisfy { dir in ["main", "subsdk0", "sdk"].allSatisfy { name in
                ["bin", "layout"].allSatisfy { ext in validFile("\(dir)/\(name).\(ext)", directory: false) }
            } }
            && npdm.allSatisfy { validFile($0, directory: false) }
        fileStatus = status
        var stats = vm_statistics64()
        var count = mach_msg_type_number_t(MemoryLayout<vm_statistics64>.size / MemoryLayout<integer_t>.size)
        let result = withUnsafeMutablePointer(to: &stats) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                host_statistics64(mach_host_self(), HOST_VM_INFO64, $0, &count)
            }
        }
        availableMemory = result == KERN_SUCCESS
            ? (UInt64(stats.free_count) + UInt64(stats.inactive_count) + UInt64(stats.purgeable_count)
               + UInt64(stats.speculative_count)) * UInt64(vm_kernel_page_size) : 0
    }

    func choose(_ file: LauncherFile) {
        let panel = NSOpenPanel()
        panel.title = "选择\(file.title)"
        panel.message = "直接使用所选文件，不会复制游戏数据。更新文件必须与已适配的 4.0.1 模块一致。"
        panel.canChooseDirectories = file.directory
        panel.canChooseFiles = !file.directory
        panel.allowsMultipleSelection = file.id == "dlc"
        if !file.directory {
            let extensions = file.id == "base" ? ["nsp", "nca"] : file.id == "keys" ? ["keys"] : ["nsp"]
            panel.allowedContentTypes = extensions.compactMap { UTType(filenameExtension: $0) }
        }
        if panel.runModal() == .OK {
            fileOverrides[file.id] = panel.urls.map(\.path)
            refresh()
        }
    }
    func resetFiles() { fileOverrides = [:]; refresh() }
    func removeDLC() { fileOverrides["dlc"] = []; refresh() }
    func reveal(_ path: String) { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: absolutePath(path))]) }
    func openFolder(_ flag: String? = nil) {
        let path = flag.flatMap { values($0).first } ?? workingDirectory
        NSWorkspace.shared.open(URL(fileURLWithPath: absolutePath(path), isDirectory: true))
    }

    // Only editable data flags are replaced; rewritten code, saves and diagnostics retain their bundle configuration.
    func launchArguments() -> [String] {
        let flags = Set(fileOverrides.keys.flatMap { $0 == "base" ? ["--base", "--base-nca"] : ["--\($0)"] })
        var args: [String] = []
        var index = 0
        while index < originalArguments.count {
            if flags.contains(originalArguments[index]) { index += 2 }
            else { args.append(originalArguments[index]); index += 1 }
        }
        for file in files {
            guard let paths = fileOverrides[file.id] else { continue }
            for path in paths {
                let flag = file.id == "base" && URL(fileURLWithPath: path).pathExtension.lowercased() == "nca" ? "base-nca" : file.id
                args += ["--\(flag)", absolutePath(path)]
            }
        }
        return args
    }

    func applySettings() throws {
        guard canStart else { throw NSError(domain: "MK8Launcher", code: 1, userInfo: [NSLocalizedDescriptionKey: "请检查必需文件及服务器地址。"] ) }
        let data = try JSONSerialization.data(withJSONObject: launchArguments())
        for key in ["NX_LAN", "NX_LAN_IF", "NX_LANPLAY_SERVER", "NX_LANGUAGE", "NX_PREWARM", "NX_SYNC_SHADERS", "NX_HANDHELD"] { unsetenv(key) }
        setenv("NX_NET", network, 1)
        if network == "lanplay" { setenv("NX_LANPLAY_SERVER", serverAddress, 1) }
        setenv("NX_PHONE_PADS", phonePads ? "1" : "0", 1)
        if language != "system" { setenv("NX_LANGUAGE", language, 1) }
        if prewarm { setenv("NX_PREWARM", "1", 1) }
        if !backgroundCompile { setenv("NX_SYNC_SHADERS", "1", 1) }
        setenv("NX_RUMBLE", rumble ? "1" : "0", 1)
        setenv("NX_MOTION", motion ? "1" : "0", 1)
        if handheld { setenv("NX_HANDHELD", "1", 1) }
        setenv("NX_MAC_LAUNCH_ARGS", String(decoding: data, as: UTF8.self), 1)
        save()
    }
    func play() {
        refresh()
        do { try applySettings(); stopNetworkCheck(); NSApp.stopModal(withCode: .OK) }
        catch { errorMessage = error.localizedDescription }
    }
    func checkNetwork() {
        guard needsNetwork else { stopNetworkCheck(); return }
        guard browser == nil else { return }
        let browser = NWBrowser(for: .bonjour(type: "_mush8._tcp", domain: nil), using: NWParameters())
        browser.stateUpdateHandler = { [weak self] state in
            var denied = false
            if case .waiting(let error) = state, case .dns(let code) = error { denied = code == -65570 }
            if case .failed(let error) = state, case .dns(let code) = error { denied = code == -65570 }
            Task { @MainActor in self?.networkDenied = denied }
        }
        self.browser = browser
        browser.start(queue: .main)
    }
    func stopNetworkCheck() { browser?.cancel(); browser = nil; networkDenied = false }
    func openNetworkSettings() {
        NSWorkspace.shared.open(URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork")!)
        stopNetworkCheck()
    }
}

struct MK8LauncherView: View {
    @StateObject private var game = GameLauncher()
    private let timer = Timer.publish(every: 2, on: .main, in: .common).autoconnect()
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("MK8 Native").font(.largeTitle.bold())
            Text("马里奥赛车 8 豪华版 · 本地原生运行").foregroundStyle(.secondary)
            ScrollView {
                VStack(alignment: .leading, spacing: 20) {
                    GroupBox("游戏语言") { languagePicker.padding(6) }
                    GroupBox("多人连接") { networkSettings.padding(6) }
                    GroupBox("控制器") {
                        VStack(alignment: .leading, spacing: 10) {
                            Text(game.controllerStatus).font(.caption).foregroundStyle(.secondary)
                            Text("支持直连 Wolverine V3 TE 8K PC（1532:0a5a），按键位置与现有 Xbox 手柄一致；此直连模式暂不提供震动或体感。").font(.caption).foregroundStyle(.secondary)
                            Toggle("震动 / Vibration", isOn: $game.rumble)
                            Toggle("体感控制 / Motion controls", isOn: $game.motion)
                            Text("震动和体感需要支持相应功能的控制器；Mac 键盘没有震动和陀螺仪。").font(.caption).foregroundStyle(.secondary)
                            Toggle("允许 iPhone 作为控制器", isOn: $game.phonePads)
                            Text("手机和 Mac 连接同一 Wi‑Fi。Mac 启动游戏后，手机点“Use as a controller for a Mac”。").font(.caption).foregroundStyle(.secondary)
                        }.padding(6).frame(maxWidth: .infinity, alignment: .leading)
                    }
                    GroupBox("画面与着色器") {
                        VStack(alignment: .leading, spacing: 10) {
                            Picker("主机模式", selection: $game.handheld) {
                                Text("Docked · 1080p").tag(false)
                                Text("Handheld · 720p").tag(true)
                            }.pickerStyle(.segmented)
                            Text("720p 可减轻 GPU 负担；两种模式均保持当前窗口的原生宽屏比例。").font(.caption).foregroundStyle(.secondary)
                            Toggle("预热管线 / Prewarm pipelines", isOn: $game.prewarm)
                            Text("启动时预先建立已记录的渲染管线。可能延长启动时间，减少进入场景时的停顿。").font(.caption).foregroundStyle(.secondary)
                            Toggle("后台着色器编译 / Background shader compiling", isOn: $game.backgroundCompile)
                            Text("开启后在后台编译新着色器以减少等待；尚未完成的效果可能暂时缺失。关闭后会等待编译完成。").font(.caption).foregroundStyle(.secondary)
                        }.padding(6).frame(maxWidth: .infinity, alignment: .leading)
                    }
                    GroupBox("游戏文件") { fileSettings.padding(6) }
                    GroupBox("系统状态") {
                        VStack(alignment: .leading, spacing: 8) {
                            Text(game.availableMemory == 0 ? "可用内存：读取中" : "可用内存（估算）：\(ByteCountFormatter.string(fromByteCount: Int64(game.availableMemory), countStyle: .memory))")
                            if game.availableMemory > 0 && game.availableMemory < 4 * 1024 * 1024 * 1024 {
                                Text("可用内存较少，运行时可能出现内存压力。").font(.caption).foregroundStyle(.orange)
                            }
                            HStack {
                                Button("运行数据目录") { game.openFolder() }
                                Button("存档目录") { game.openFolder("save") }
                                if let log = gameLog { Button("显示日志") { game.reveal(log) } }
                            }
                        }.padding(6).frame(maxWidth: .infinity, alignment: .leading)
                    }
                }.padding(.trailing, 6)
            }
            Divider()
            HStack {
                if !game.canStart { Text("请补齐必需文件或 Online 服务器地址。").font(.caption).foregroundStyle(.orange) }
                Spacer()
                Button("开始游戏") { game.play() }.keyboardShortcut(.defaultAction).disabled(!game.canStart)
            }
        }.padding(24).frame(minWidth: 600, idealWidth: 660, minHeight: 550, idealHeight: 790)
        .onAppear { game.checkNetwork(); WolverineUSB.shared.start() }
        .onDisappear { game.stopNetworkCheck() }
        .onChange(of: game.needsNetwork) { game.checkNetwork() }
        .onReceive(timer) { _ in game.refresh(); game.checkNetwork() }
        .alert("无法启动", isPresented: Binding(get: { game.errorMessage != nil }, set: { if !$0 { game.errorMessage = nil } })) {
            Button("好") { game.errorMessage = nil }
        } message: { Text(game.errorMessage ?? "") }
    }
    private var gameLog: String? {
        guard let url = Bundle.main.resourceURL?.appendingPathComponent("launch.json"),
              let data = try? Data(contentsOf: url),
              let config = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return nil }
        return config["log"] as? String
    }
    private var networkSettings: some View {
        VStack(alignment: .leading, spacing: 10) {
            Picker("多人连接", selection: $game.network) {
                Text("关闭").tag("off"); Text("LAN").tag("lan")
                Text("Online · Server").tag("lanplay"); Text("Wireless").tag("wireless")
            }.pickerStyle(.segmented)
            if game.network == "lanplay" {
                TextField("Server (host:port)，例如 example.org:11451", text: $game.lanPlayServer).textFieldStyle(.roundedBorder)
                if game.serverAddress.isEmpty { Text("请先填写 LAN-Play 服务器地址。").font(.caption).foregroundStyle(.orange) }
            }
            Text(game.networkDescription).font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            if game.networkDenied {
                Text("本地网络访问被关闭，无法发现房间或手机控制器。请在系统设置中允许此应用访问本地网络。").font(.caption).foregroundStyle(.orange)
                Button("打开本地网络设置") { game.openNetworkSettings() }
            }
        }.frame(maxWidth: .infinity, alignment: .leading)
    }
    private var fileSettings: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("选择已有文件直接使用，不会复制数据。当前适配版本：4.0.1。更换游戏版本需重新提取并适配代码。").font(.caption).foregroundStyle(.secondary)
            ForEach(game.files) { file in
                VStack(alignment: .leading, spacing: 3) {
                    HStack {
                        Image(systemName: game.fileStatus[file.id] == true ? "checkmark.circle.fill" : "exclamationmark.circle")
                            .foregroundStyle(game.fileStatus[file.id] == true ? Color.green : file.optional ? Color.secondary : Color.orange)
                        Text(file.title).font(.callout)
                        if file.optional { Text("可选").font(.caption).foregroundStyle(.secondary) }
                        Spacer()
                        Button("选择…") { game.choose(file) }.accessibilityLabel("选择\(file.title)")
                        if file.id == "dlc" && !game.paths(for: "dlc").isEmpty { Button("移除") { game.removeDLC() } }
                    }
                    ForEach(Array(game.paths(for: file.id).enumerated()), id: \.offset) { _, path in
                        Text(path).font(.caption).foregroundStyle(.secondary).lineLimit(2).textSelection(.enabled)
                            .help(game.absolutePath(path)).onTapGesture(count: 2) { game.reveal(path) }
                    }
                    if game.fileStatus[file.id] != true {
                        Text(file.id == "dlc" && !game.paths(for: "dlc").isEmpty ? "所选 DLC 不可读，请重新选择或移除。" : file.optional ? "未提供或不可读；可不使用此资源。" : "缺失、为空或不可读，请重新选择。")
                            .font(.caption).foregroundStyle(file.optional ? Color.secondary : Color.orange)
                    }
                }
            }
            Label(game.fileStatus["runtime"] == true ? "已适配的代码模块已就绪" : "已适配的代码模块缺失，请重新生成本地应用", systemImage: game.fileStatus["runtime"] == true ? "checkmark.circle" : "exclamationmark.triangle")
                .font(.caption).foregroundStyle(game.fileStatus["runtime"] == true ? Color.secondary : Color.orange)
            Text("未提供固件时沿用本项目的兼容资源路径。").font(.caption).foregroundStyle(.secondary)
            HStack {
                Button("刷新文件状态") { game.refresh() }
                Button("恢复应用原始文件路径") { game.resetFiles() }.disabled(game.fileOverrides.isEmpty)
            }
        }.frame(maxWidth: .infinity, alignment: .leading)
    }
    private var languagePicker: some View {
        Picker("游戏语言", selection: $game.language) {
            Text("跟随系统").tag("system"); Text("简体中文").tag("zh-Hans"); Text("繁體中文").tag("zh-Hant")
            Text("English (US)").tag("en-US"); Text("English (UK)").tag("en-GB")
            Text("日本語").tag("ja"); Text("한국어").tag("ko"); Text("Français").tag("fr")
            Text("Français (Canada)").tag("fr-CA"); Text("Deutsch").tag("de"); Text("Italiano").tag("it")
            Text("Español").tag("es"); Text("Español (Latinoamérica)").tag("es-419")
            Text("Nederlands").tag("nl"); Text("Português (Portugal)").tag("pt-PT"); Text("Русский").tag("ru")
        }
    }
}

@MainActor private final class SettingsWindowDelegate: NSObject, NSWindowDelegate {
    func windowWillClose(_ notification: Notification) { NSApp.abortModal() }
}

// Runs before guest initialization, in nxhost itself, preserving the app's file permissions.
@_cdecl("mk8ShowMacSettings")
@MainActor public func mk8ShowMacSettings() -> Int32 {
    let app = NSApplication.shared
    app.setActivationPolicy(.regular)
    app.finishLaunching()
    let height = min(790, (NSScreen.main?.visibleFrame.height ?? 850) - 80)
    let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 660, height: height),
                          styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
    window.minSize = NSSize(width: 648, height: 570)
    window.title = "MK8 Native 设置"
    window.isReleasedWhenClosed = false
    let delegate = SettingsWindowDelegate()
    window.delegate = delegate
    window.contentView = NSHostingView(rootView: MK8LauncherView())
    window.center()
    window.makeKeyAndOrderFront(nil)
    app.activate(ignoringOtherApps: true)
    let result = app.runModal(for: window)
    window.orderOut(nil)
    withExtendedLifetime(delegate) {}
    return result == .OK ? 1 : 0
}
