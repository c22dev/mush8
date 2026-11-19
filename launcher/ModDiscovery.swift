// Mush8 - ModDiscovery.swift
// Constantin Clerc (c22dev) - 0.1
import Foundation

struct Mod: Identifiable, Sendable {
    let id: String
    let name: String
    let path: String
    let romfs: Bool
    let patches: [String]
    let modules: [String]
    let replacedModules: Bool
    let unsupportedFiles: [String]

    var displayTitle: String {
        guard let range = name.range(of: " [GB", options: .backwards), name.hasSuffix("]") else { return name }
        return String(name[..<range.lowerBound])
    }

    var loadable: Bool { !replacedModules && unsupportedFiles.isEmpty }

    var summary: String {
        var parts: [String] = []
        if romfs { parts.append("replaces game files") }
        if !patches.isEmpty { parts.append(patches.count == 1 ? "1 code patch" : "\(patches.count) code patches") }
        if !modules.isEmpty { parts.append(modules.count == 1 ? "1 code module" : "\(modules.count) code modules") }
        if replacedModules { parts.append("replaced code modules are not supported") }
        if !unsupportedFiles.isEmpty { parts.append("unsupported files: " + unsupportedFiles.joined(separator: ", ")) }
        return parts.isEmpty ? "no code patches in exefs" : parts.joined(separator: ", ")
    }
}

func caselessChild(_ dir: URL, _ name: String) -> URL? {
    let fm = FileManager.default
    return (try? fm.contentsOfDirectory(atPath: dir.path))?.sorted().compactMap { child -> URL? in
        let url = dir.appendingPathComponent(child)
        var isDir: ObjCBool = false
        return child.caseInsensitiveCompare(name) == .orderedSame && fm.fileExists(atPath: url.path, isDirectory: &isDir) && isDir.boolValue ? url : nil
    }.first
}

func modEntries(_ dir: URL, directories: Bool) -> [URL] {
    let fm = FileManager.default
    return ((try? fm.contentsOfDirectory(atPath: dir.path)) ?? []).filter { !$0.hasPrefix(".") }
        .map { dir.appendingPathComponent($0) }
        .filter {
            var isDir: ObjCBool = false
            return fm.fileExists(atPath: $0.path, isDirectory: &isDir) && isDir.boolValue == directories
        }
        .sorted { $0.lastPathComponent.localizedStandardCompare($1.lastPathComponent) == .orderedAscending }
}

func isCodePatch(_ url: URL) -> Bool {
    switch url.pathExtension.lowercased() {
    case "pchtxt": return patchBuildID(url).map { !$0.isEmpty && $0.allSatisfy { $0.isASCII && $0.isHexDigit } } ?? false
    case "ips": return isBuildID(url.lastPathComponent.split(separator: ".").first.map(String.init) ?? "")
    default: return false
    }
}

// subsdk1 to subsdk9 are extra code modules; the game's own modules are main, subsdk0 and sdk
func isCodeModule(_ url: URL) -> Bool {
    let name = url.lastPathComponent.lowercased()
    return url.pathExtension.isEmpty && name.count == 7 && name.hasPrefix("subsdk") && ("1"..."9").contains(String(name.last!))
}

func isBuildID(_ name: String) -> Bool {
    (16...64).contains(name.count) && name.allSatisfy { $0.isASCII && $0.isHexDigit }
}

func normalizedBuildID(_ id: String) -> String {
    String(id.trimmingCharacters(in: .whitespaces).lowercased().reversed().drop { $0 == "0" }.reversed())
}

func patchBuildID(_ url: URL) -> String? {
    guard url.pathExtension.caseInsensitiveCompare("pchtxt") == .orderedSame else {
        return url.lastPathComponent.split(separator: ".").first.map { normalizedBuildID(String($0)) }
    }
    guard let text = try? String(contentsOf: url, encoding: .utf8) else { return nil }
    var id: String?
    for line in text.split(whereSeparator: \.isNewline) {
        let line = line.trimmingCharacters(in: .whitespaces)
        if line.hasPrefix("@stop") { break }
        if line.hasPrefix("@nsobid-") { id = String(line.dropFirst("@nsobid-".count)) }
    }
    return id.map(normalizedBuildID)
}

func gameBuildIDs(in folder: URL) -> Set<String>? {
    #if os(iOS)
    guard let modules = Bundle.main.resourceURL?.appendingPathComponent("modules"),
          let names = try? FileManager.default.contentsOfDirectory(atPath: modules.path) else { return nil }
    let ids = names.filter { $0.hasSuffix(".layout") }.compactMap { name in
        (try? String(contentsOf: modules.appendingPathComponent(name), encoding: .utf8))?
            .split(whereSeparator: \.isNewline).first { $0.hasPrefix("module_id ") }
            .map { normalizedBuildID(String($0.dropFirst("module_id ".count))) }
    }
    #else
    let exefs = folder.appendingPathComponent("work/v-upd/exefs")
    let ids = ["main", "subsdk0", "sdk"].compactMap { name -> String? in
        guard let handle = try? FileHandle(forReadingFrom: exefs.appendingPathComponent(name)) else { return nil }
        defer { try? handle.close() }
        guard let header = try? handle.read(upToCount: 0x60), header.count == 0x60, header.prefix(4) == Data("NSO0".utf8) else { return nil }
        return normalizedBuildID(header[0x40..<0x60].map { String(format: "%02x", $0) }.joined())
    }
    #endif
    return ids.isEmpty ? nil : Set(ids)
}

func modEntries(_ dir: URL) -> [URL] {
    ((try? FileManager.default.contentsOfDirectory(atPath: dir.path)) ?? []).filter { !$0.hasPrefix(".") }.map { dir.appendingPathComponent($0) }
}

func stubFiles(in dir: URL, depth: Int = 0) -> [URL] {
    guard depth < 64 else { return [] }
    return modEntries(dir).flatMap { url -> [URL] in
        if url.pathExtension.caseInsensitiveCompare("stub") == .orderedSame { return [url] }
        var isDir: ObjCBool = false
        return FileManager.default.fileExists(atPath: url.path, isDirectory: &isDir) && isDir.boolValue ? stubFiles(in: url, depth: depth + 1) : []
    }
}

func modInfo(at dir: URL, relativeTo folder: URL, id: String) -> Mod? {
    func relative(_ url: URL) -> String? {
        let prefix = folder.path + "/"
        guard url.path.hasPrefix(prefix) else { return nil }
        return String(url.path.dropFirst(prefix.count))
    }
    func unsupported(_ url: URL) -> Bool {
        let name = url.lastPathComponent.lowercased()
        return name == "exefs.nsp" || name == "romfs.bin" || name.hasSuffix(".stub")
    }

    let name = dir.lastPathComponent
    guard name.count != 16 || UInt64(name, radix: 16) == nil || name == "0100152000022000" else { return nil }
    let atmosphere = caselessChild(dir, "atmosphere")
    let roots = [dir,
                 atmosphere.flatMap { caselessChild($0, "contents") }.flatMap { caselessChild($0, "0100152000022000") },
                 caselessChild(dir, "contents").flatMap { caselessChild($0, "0100152000022000") },
                 caselessChild(dir, "0100152000022000")].compactMap { $0 }
    let content = roots.first {
        caselessChild($0, "romfs") != nil || caselessChild($0, "exefs") != nil || modEntries($0).contains(where: unsupported)
    }
    let romfs = content.flatMap { caselessChild($0, "romfs") }
    let exefs = content.flatMap { caselessChild($0, "exefs") }
    let files = exefs.map { modEntries($0, directories: false) } ?? []
    var patches = files.filter(isCodePatch)
    for patchRoot in [caselessChild(dir, "exefs_patches"), atmosphere.flatMap { caselessChild($0, "exefs_patches") }].compactMap({ $0 }) {
        patches += modEntries(patchRoot, directories: true).flatMap { modEntries($0, directories: false) }.filter(isCodePatch)
    }
    let hasPatches = !patches.isEmpty
    #if os(iOS)
    let filterPatches = true
    #else
    let filterPatches = id.hasPrefix("./")
    #endif
    if filterPatches && hasPatches {
        let game = gameBuildIDs(in: folder)
        patches = patches.filter { patch in game.map { ids in patchBuildID(patch).map(ids.contains) ?? false } ?? false }
    }
    guard content != nil || hasPatches, let path = relative(dir) else { return nil }
    let patchPaths = patches.compactMap(relative)
    guard patchPaths.count == patches.count else { return nil }
    // code modules run on macOS only, so iOS counts them as replaced game modules
    #if os(iOS)
    let modules: [URL] = []
    let extraModules = false
    #else
    let modules = files.filter(isCodeModule)
    let extraModules = true
    #endif
    let romfsStubs = romfs.map { stubFiles(in: $0) } ?? []
    let exefsEntries = exefs.map { modEntries($0) } ?? []
    let plugins = (romfs.flatMap { caselessChild($0, "skyline") }.flatMap { caselessChild($0, "plugins") }.map { modEntries($0) } ?? [])
        .filter { $0.pathExtension.caseInsensitiveCompare("nro") == .orderedSame }
    let npdm = exefsEntries.first { $0.lastPathComponent.caseInsensitiveCompare("main.npdm") == .orderedSame }
    let badNpdm = npdm.map { url in
        let header = (try? FileHandle(forReadingFrom: url)).flatMap { handle in
            defer { try? handle.close() }
            return try? handle.read(upToCount: 0x20)
        } ?? Data()
        return !(header.count == 0x20 && header.prefix(4) == Data("META".utf8) && header[0xC] & 1 != 0)
    } ?? false
    let unsupportedFiles = (content.map { modEntries($0) } ?? []).filter(unsupported)
        + exefsEntries.filter { $0.pathExtension.caseInsensitiveCompare("stub") == .orderedSame }
        + romfsStubs + plugins + (badNpdm ? npdm.map { [$0] } ?? [] : [])
    return Mod(id: id, name: name, path: path, romfs: romfs != nil,
               patches: patchPaths, modules: modules.compactMap(relative), replacedModules: exefsEntries.contains {
                   let name = $0.lastPathComponent.lowercased()
                   return ["main", "sdk", "rtld"].contains(name) || (name.hasPrefix("subsdk") && $0.pathExtension.isEmpty && !(extraModules && isCodeModule($0)))
               }, unsupportedFiles: unsupportedFiles.map(\.lastPathComponent))
}

func skippedPatches(in folder: URL) -> [String: String] {
    let text = (try? String(contentsOf: folder.appendingPathComponent("work/v-upd/patched/nxpatch.skipped"), encoding: .utf8)) ?? ""
    return Dictionary(text.split(whereSeparator: \.isNewline).compactMap { line -> (String, String)? in
        let parts = line.split(separator: "\t", maxSplits: 1).map(String.init)
        return parts.count == 2 ? (parts[0], parts[1]) : nil
    }, uniquingKeysWith: { $1 })
}

func patchWarning(_ mod: Mod, skipped: [String: String]) -> String? {
    let reasons = mod.patches.compactMap { skipped[$0] }
    if let failed = reasons.first(where: { !$0.hasPrefix("it is for build ") }) { return "A code patch was skipped: \(failed)" }
    return !mod.patches.isEmpty && reasons.count == mod.patches.count ? "No code patch matches this game update" : nil
}

func installedMods(in folder: URL) -> [Mod] {
    let folder = folder.resolvingSymlinksInPath()
    let root = caselessChild(folder, "mods") ?? folder.appendingPathComponent("mods")
    var mods = modEntries(root, directories: true).compactMap { modInfo(at: $0, relativeTo: folder, id: $0.lastPathComponent) }
    if let atmosphere = caselessChild(folder, "atmosphere"), let installed = modInfo(at: atmosphere, relativeTo: folder, id: "./" + atmosphere.lastPathComponent) {
        mods.append(installed)
    }
    return mods
}

struct ModFileEntry: Identifiable, Sendable {
    let path: String
    let depth: Int
    let directory: Bool
    let symbolicLink: Bool
    let size: Int64
    var id: String { path }
    var name: String { (path as NSString).lastPathComponent }
}

struct ModFileTree: Sendable {
    var entries: [ModFileEntry] = []
    var truncated = false
}

struct ModError: LocalizedError {
    let message: String
    var errorDescription: String? { message }
    init(_ message: String) { self.message = message }
}

func canDeleteMod(_ mod: Mod) -> Bool {
    let parts = mod.path.split(separator: "/", omittingEmptySubsequences: false)
    return parts.count == 2 && parts[0].lowercased() == "mods" && !parts[1].isEmpty
        && parts[1] != "." && parts[1] != ".." && mod.id == String(parts[1])
}

private func managedModURL(_ mod: Mod, in folder: URL) throws -> URL {
    let folder = folder.resolvingSymlinksInPath().standardizedFileURL
    let parts = mod.path.split(separator: "/", omittingEmptySubsequences: false)
    guard folder.isFileURL, !parts.isEmpty, !parts.contains("."), !parts.contains(".."), !parts.contains("") else {
        throw ModError("The mod path is invalid.")
    }
    let url = folder.appendingPathComponent(mod.path).standardizedFileURL
    let parent = url.deletingLastPathComponent()
    guard url.path.hasPrefix(folder.path + "/"), parent.resolvingSymlinksInPath().path == parent.path,
          installedMods(in: folder).contains(where: { $0.id == mod.id && $0.path == mod.path }) else {
        throw ModError("The mod folder changed or its parent is a symbolic link. Refresh the Mods list.")
    }
    return url
}

func deleteInstalledMod(_ mod: Mod, in folder: URL) throws {
    guard canDeleteMod(mod) else {
        throw ModError("The shared Atmosphere folder is read-only here because it may contain other games' mods.")
    }
    try FileManager.default.removeItem(at: managedModURL(mod, in: folder))
}

func modFileTree(_ mod: Mod, in folder: URL, limit: Int = 2_000, depthLimit: Int = 32) throws -> ModFileTree {
    let root = try managedModURL(mod, in: folder)
    let fm = FileManager.default
    var tree = ModFileTree()
    func visit(_ url: URL, path: String, depth: Int) throws {
        try Task.checkCancellation()
        guard tree.entries.count < limit else { tree.truncated = true; return }
        let attributes = try fm.attributesOfItem(atPath: url.path)
        let type = attributes[.type] as? FileAttributeType
        let directory = type == .typeDirectory
        tree.entries.append(ModFileEntry(path: path, depth: depth, directory: directory,
                                         symbolicLink: type == .typeSymbolicLink,
                                         size: (attributes[.size] as? NSNumber)?.int64Value ?? 0))
        guard directory else { return }
        guard depth < depthLimit else { tree.truncated = true; return }
        let children = try fm.contentsOfDirectory(atPath: url.path).sorted { $0.localizedStandardCompare($1) == .orderedAscending }
        for child in children {
            guard tree.entries.count < limit else { tree.truncated = true; break }
            try visit(url.appendingPathComponent(child), path: path.isEmpty ? child : path + "/" + child, depth: depth + 1)
        }
    }
    try visit(root, path: root.lastPathComponent, depth: 0)
    return tree
}
