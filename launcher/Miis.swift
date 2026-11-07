// Mush8 - Miis.swift
// Constantin Clerc (c22dev) - 0.1
import Foundation

let miiDatabaseName = "MiiDatabase.dat"
let charInfoSize = 0x58
let storeDataSize = 0x44
let nfdbSize = 0x1A98
let maxMiis = 100

func isMiiDatabase(_ data: Data) -> Bool { data.count >= nfdbSize && data.prefix(4) == Data("NFDB".utf8) }

func isMiiFile(_ data: Data) -> Bool { isMiiDatabase(data) || data.count == charInfoSize || data.count == storeDataSize }

func miiIDs(_ data: Data) -> [Data] {
    let bytes = [UInt8](data.prefix(nfdbSize))
    if isMiiDatabase(data) {
        return (0..<min(Int(bytes[nfdbSize - 3]), maxMiis)).map { i in
            let id = 4 + i * storeDataSize + 0x30
            return Data(bytes[id..<id + 16])
        }
    }
    switch data.count {
    case charInfoSize: return [Data(bytes[0..<16])]
    case storeDataSize: return [Data(bytes[0x30..<0x40])]
    default: return []
    }
}

func miisFolder(in folder: URL) -> URL { folder.appendingPathComponent("miis") }

private func miiHeader(at url: URL) -> Data? {
    guard let size = (try? url.resourceValues(forKeys: [.fileSizeKey]))?.fileSize else { return nil }
    if size == charInfoSize || size == storeDataSize { return try? Data(contentsOf: url) }
    guard size >= nfdbSize, let file = try? FileHandle(forReadingFrom: url) else { return nil }
    defer { try? file.close() }
    return try? file.read(upToCount: nfdbSize)
}

func installedMiiCount(in folder: URL) -> Int {
    let dir = miisFolder(in: folder)
    let names = (try? FileManager.default.contentsOfDirectory(atPath: dir.path)) ?? []
    var ids = Set<Data>()
    for name in names {
        if let data = miiHeader(at: dir.appendingPathComponent(name)) { ids.formUnion(miiIDs(data)) }
    }
    return min(ids.count, maxMiis)
}

private func miiDestination(for name: String, data: Data, in dir: URL) -> URL {
    if isMiiDatabase(data) { return dir.appendingPathComponent(miiDatabaseName) }
    var file = name as NSString
    if name.caseInsensitiveCompare(miiDatabaseName) == .orderedSame {
        file = (data.count == storeDataSize ? "mii.storedata" : "mii.charinfo") as NSString
    }
    var url = dir.appendingPathComponent(file as String)
    var copy = 2
    while let existing = try? Data(contentsOf: url), existing != data {
        let numbered = "\(file.deletingPathExtension) \(copy)"
        url = dir.appendingPathComponent(file.pathExtension.isEmpty ? numbered : "\(numbered).\(file.pathExtension)")
        copy += 1
    }
    return url
}

func importMiis(_ urls: [URL], into folder: URL) throws {
    let dir = miisFolder(in: folder)
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    var skipped: [String] = []
    for url in urls {
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        guard let data = try? Data(contentsOf: url), isMiiFile(data) else {
            skipped.append(url.lastPathComponent)
            continue
        }
        try data.write(to: miiDestination(for: url.lastPathComponent, data: data, in: dir), options: .atomic)
    }
    if !skipped.isEmpty {
        let names = skipped.joined(separator: ", ")
        throw NSError(domain: "Mush8", code: 1, userInfo: [NSLocalizedDescriptionKey: "Skipped \(names): not a MiiDatabase.dat, .charinfo or .storedata file"])
    }
}

func removeMiis(in folder: URL) throws {
    let dir = miisFolder(in: folder)
    if FileManager.default.fileExists(atPath: dir.path) { try FileManager.default.removeItem(at: dir) }
}
