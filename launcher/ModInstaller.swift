// Mush8 - ModInstaller.swift
import Foundation

enum ModInstaller {
    private static let expandedLimit: Int64 = 2 * 1024 * 1024 * 1024
    private static let entryLimit = 20_000

    static func install(mod: BananaMod, file: BananaFile, in folder: URL,
                        progress: @escaping @Sendable (BananaDownloadProgress?) -> Void = { _ in }) async throws -> String {
        #if os(iOS)
        guard !["ips", "pchtxt"].contains((file.name as NSString).pathExtension.lowercased()) else {
            throw ModError("iOS code patches require rebuilding the app on a Mac. Choose a RomFS-only mod.")
        }
        #endif
        let download = FileManager.default.temporaryDirectory.appendingPathComponent("Mush8-mod-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: download, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: download) }
        let archive = download.appendingPathComponent("download")
        try await GameBanana.download(file, to: archive) { progress($0) }
        try Task.checkCancellation()
        progress(nil)
        let worker = Task.detached(priority: .userInitiated) {
            try installArchive(archive, mod: mod, file: file, in: folder)
        }
        return try await withTaskCancellationHandler {
            try await worker.value
        } onCancel: {
            worker.cancel()
        }
    }

    static func installArchive(_ archive: URL, mod: BananaMod, file: BananaFile, in folder: URL) throws -> String {
        let fm = FileManager.default
        guard mod.id > 0, file.id > 0 else { throw ModError("Invalid GameBanana item ID.") }
        var title = ""
        for character in mod.name {
            guard !character.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) || "/\\:".unicodeScalars.contains($0) }) else { continue }
            if title.utf8.count + String(character).utf8.count > 100 { break }
            title.append(character)
        }
        title = title.trimmingCharacters(in: .whitespacesAndNewlines.union(CharacterSet(charactersIn: ".")))
        let name = "\(title.isEmpty ? "Mod" : title) [GB\(mod.id)-\(file.id)]"
        let mods = caselessChild(folder, "mods") ?? folder.appendingPathComponent("mods", isDirectory: true)
        try fm.createDirectory(at: mods, withIntermediateDirectories: true)
        let destination = mods.appendingPathComponent(name, isDirectory: true)
        guard !fm.fileExists(atPath: destination.path) else { throw ModError("\(name) is already installed. Existing files were kept.") }
        let staging = mods.appendingPathComponent(".install-\(UUID().uuidString)", isDirectory: true)
        try fm.createDirectory(at: staging, withIntermediateDirectories: false)
        defer { try? fm.removeItem(at: staging) }
        let unpacked = staging.appendingPathComponent("payload", isDirectory: true)
        try fm.createDirectory(at: unpacked, withIntermediateDirectories: false)
        if ["ips", "pchtxt"].contains((file.name as NSString).pathExtension.lowercased()) {
            let components = try safePath(file.name)
            guard components.count == 1 else { throw ModError("A code patch must have a plain filename.") }
            let exefs = unpacked.appendingPathComponent("exefs", isDirectory: true)
            try fm.createDirectory(at: exefs, withIntermediateDirectories: false)
            try fm.copyItem(at: archive, to: exefs.appendingPathComponent(file.name))
        } else {
            try extract(archive, to: unpacked)
        }
        let packages = try packages(in: unpacked)
        guard packages.count == 1 else {
            throw ModError(packages.isEmpty
                ? "This download has no Mario Kart 8 Deluxe romfs/exefs/exefs_patches layout. Choose another distribution file."
                : "This archive contains multiple mod variants. Choose a download with one variant; they were not merged.")
        }
        let payload = packages[0]
        try validate(payload)
        try Task.checkCancellation()
        // Publish only the fully checked package, on the same filesystem as mods.
        try fm.moveItem(at: payload, to: destination)
        return name
    }

    private static func safePath(_ name: String) throws -> [String] {
        guard !name.hasPrefix("/"), !name.contains("\\"), !name.contains(":"), name.utf8.count <= 1024,
              !name.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains) else {
            throw ModError("The archive contains an unsafe path: \(name)")
        }
        let parts = name.split(separator: "/").map(String.init)
        guard !parts.contains(".."), parts.count <= 64 else { throw ModError("The archive contains an unsafe path: \(name)") }
        return parts.filter { $0 != "." }
    }

    private static func extract(_ file: URL, to folder: URL) throws {
        guard let reader = archive_read_new() else { throw ModError("Could not create the archive reader.") }
        defer { archive_read_free(reader) }
        archive_read_support_format_zip(reader)
        archive_read_support_format_7zip(reader)
        archive_read_support_format_rar(reader)
        archive_read_support_format_rar5(reader)
        func failure() -> ModError {
            ModError(archive_error_string(reader).map { String(cString: $0) } ?? "The archive is damaged, encrypted, or unsupported.")
        }
        guard archive_read_open_filename(reader, file.path, 64 * 1024) == 0 else { throw failure() }
        var entry: OpaquePointer?
        var entries = 0
        var expanded: Int64 = 0
        var declared: Int64 = 0
        var seen: [String: (String, Bool)] = [:]
        var buffer = [UInt8](repeating: 0, count: 64 * 1024)
        while true {
            try Task.checkCancellation()
            let status = archive_read_next_header(reader, &entry)
            if status == 1 { break }
            guard status == 0, let entry else { throw failure() }
            entries += 1
            guard entries <= entryLimit else { throw ModError("The archive contains more than \(entryLimit) entries.") }
            guard let raw = archive_entry_pathname(entry), let path = String(validatingCString: raw) else {
                throw ModError("The archive contains a filename that is not valid UTF-8.")
            }
            let type = archive_entry_filetype(entry)
            let directory = type == mode_t(0o040000)
            guard (directory || type == mode_t(0o100000)), archive_entry_symlink(entry) == nil, archive_entry_hardlink(entry) == nil else {
                throw ModError("Archive links and special files are not supported: \(path)")
            }
            let parts = try safePath(path)
            if parts.isEmpty && directory { continue }
            guard !parts.isEmpty else { throw ModError("The archive contains an empty filename.") }
            let normalized = parts.joined(separator: "/")
            let key = normalized.precomposedStringWithCanonicalMapping.lowercased()
            if let old = seen[key], !(directory && old.1 && old.0 == normalized) {
                throw ModError("The archive contains colliding filenames: \(normalized)")
            }
            seen[key] = (normalized, directory)
            let size = archive_entry_size(entry)
            guard size >= 0, size <= expandedLimit - declared else { throw ModError("The archive exceeds the 2 GiB extraction limit.") }
            declared += size
            let target = parts.reduce(folder) { $0.appendingPathComponent($1) }
            if directory {
                try FileManager.default.createDirectory(at: target, withIntermediateDirectories: true)
                continue
            }
            try FileManager.default.createDirectory(at: target.deletingLastPathComponent(), withIntermediateDirectories: true)
            guard FileManager.default.createFile(atPath: target.path, contents: nil) else { throw ModError("Could not create \(normalized).") }
            let output = try FileHandle(forWritingTo: target)
            do {
                defer { try? output.close() }
                var written: Int64 = 0
                while true {
                    try Task.checkCancellation()
                    let count = archive_read_data(reader, &buffer, buffer.count)
                    if count == 0 { break }
                    guard count > 0 else { throw failure() }
                    guard Int64(count) <= expandedLimit - expanded else { throw ModError("The archive exceeds the 2 GiB extraction limit.") }
                    expanded += Int64(count)
                    written += Int64(count)
                    try output.write(contentsOf: buffer.prefix(count))
                }
                guard written == size else { throw ModError("The archive has an incorrect size for \(normalized).") }
            }
        }
    }

    private static func packages(in directory: URL, depth: Int = 0) throws -> [URL] {
        try Task.checkCancellation()
        let name = directory.lastPathComponent
        if name.count == 16, UInt64(name, radix: 16) != nil, name != "0100152000022000" { return [] }
        if modInfo(at: directory, relativeTo: directory.deletingLastPathComponent(), id: name) != nil { return [directory] }
        guard depth < 12 else { throw ModError("The archive has too many wrapper folders.") }
        return try modEntries(directory, directories: true).filter { $0.lastPathComponent != "__MACOSX" }
            .flatMap { try packages(in: $0, depth: depth + 1) }
    }

    private static func validate(_ directory: URL) throws {
        guard let mod = modInfo(at: directory, relativeTo: directory.deletingLastPathComponent(), id: directory.lastPathComponent) else {
            throw ModError("No supported mod was found.")
        }
        guard !mod.replacedModules, mod.unsupportedFiles.isEmpty else { throw ModError("Native ExeFS modules, packed files, and .stub removal are not supported by Mush8.") }
        #if os(iOS)
        guard mod.patches.isEmpty else { throw ModError("iOS code patches require rebuilding the app on a Mac. Choose a RomFS-only mod.") }
        #endif
        guard let files = FileManager.default.enumerator(at: directory, includingPropertiesForKeys: nil) else { throw ModError("Could not inspect the downloaded files.") }
        for case let file as URL in files {
            try Task.checkCancellation()
            let path = file.path.lowercased()
            if path.hasSuffix(".stub") || (path.contains("/romfs/skyline/plugins/") && path.hasSuffix(".nro")) {
                throw ModError("Skyline plugins and .stub removal are not supported by Mush8.")
            }
            if path.hasSuffix("/exefs/main.npdm") {
                let handle = try FileHandle(forReadingFrom: file)
                defer { try? handle.close() }
                let data = try handle.read(upToCount: 0x20) ?? Data()
                guard data.count >= 0x20, data.prefix(4) == Data("META".utf8), data[0xC] & 1 != 0 else {
                    throw ModError("The mod has an invalid or ARM32 main.npdm; Mush8 requires AArch64.")
                }
            }
        }
    }
}
