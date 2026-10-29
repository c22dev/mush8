// Mush8 - ModDiscovery.swift
import Foundation

struct Mod: Identifiable, Sendable {
    let id: String
    let name: String
    let path: String
    let romfs: Bool
    let patches: [String]
    let replacedModules: Bool
    let unsupportedFiles: [String]

    var displayTitle: String {
        guard let range = name.range(of: " [GB", options: .backwards), name.hasSuffix("]") else { return name }
        return String(name[..<range.lowerBound])
    }

    var summary: String {
        var parts: [String] = []
        if romfs { parts.append("replaces game files") }
        if !patches.isEmpty { parts.append(patches.count == 1 ? "1 code patch" : "\(patches.count) code patches") }
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
        caselessChild($0, "romfs") != nil || caselessChild($0, "exefs") != nil || modEntries($0, directories: false).contains(where: unsupported)
    }
    let romfs = content.flatMap { caselessChild($0, "romfs") }
    let exefs = content.flatMap { caselessChild($0, "exefs") }
    let files = exefs.map { modEntries($0, directories: false) } ?? []
    var patches = files.filter { ["ips", "pchtxt"].contains($0.pathExtension.lowercased()) }
    if let patchRoot = caselessChild(atmosphere ?? dir, "exefs_patches") {
        patches += modEntries(patchRoot, directories: true).flatMap { modEntries($0, directories: false) }
            .filter { ["ips", "pchtxt"].contains($0.pathExtension.lowercased()) }
    }
    guard content != nil || !patches.isEmpty, let path = relative(dir) else { return nil }
    let patchPaths = patches.compactMap(relative)
    guard patchPaths.count == patches.count else { return nil }
    let unsupportedFiles = (content.map { modEntries($0, directories: false) } ?? []).filter(unsupported)
        + files.filter { $0.pathExtension.caseInsensitiveCompare("stub") == .orderedSame }
    return Mod(id: id, name: name, path: path, romfs: romfs != nil,
               patches: patchPaths, replacedModules: files.contains {
                   let name = $0.lastPathComponent.lowercased()
                   return ["main", "sdk", "rtld"].contains(name) || (name.hasPrefix("subsdk") && $0.pathExtension.isEmpty)
               }, unsupportedFiles: unsupportedFiles.map(\.lastPathComponent))
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
