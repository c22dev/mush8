// Mush8 - Miis.swift
// Constantin Clerc (c22dev) - 0.1
import Foundation

let miiDatabaseName = "MiiDatabase.dat"
let charInfoSize = 0x58
let storeDataSize = 0x44
let nfdbSize = 0x1A98

func miiCount(_ data: Data) -> Int {
    if data.count >= nfdbSize && data.prefix(4) == Data("NFDB".utf8) { return min(Int(data[data.startIndex + nfdbSize - 3]), 100) }
    return data.count == charInfoSize || data.count == storeDataSize ? 1 : 0
}

func miisFolder(in folder: URL) -> URL { folder.appendingPathComponent("miis") }

func installedMiiCount(in folder: URL) -> Int {
    let dir = miisFolder(in: folder)
    let names = (try? FileManager.default.contentsOfDirectory(atPath: dir.path)) ?? []
    return min(names.reduce(0) { n, name in n + ((try? Data(contentsOf: dir.appendingPathComponent(name))).map(miiCount) ?? 0) }, 100)
}

func importMiis(_ urls: [URL], into folder: URL) throws {
    let dir = miisFolder(in: folder)
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    for url in urls {
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        let data = try Data(contentsOf: url)
        guard miiCount(data) > 0 else {
            throw NSError(domain: "Mush8", code: 1, userInfo: [NSLocalizedDescriptionKey: "\(url.lastPathComponent) isn't a MiiDatabase.dat, .charinfo or .storedata file"])
        }
        let isDatabase = data.count >= nfdbSize && data.prefix(4) == Data("NFDB".utf8)
        try data.write(to: dir.appendingPathComponent(isDatabase ? miiDatabaseName : url.lastPathComponent), options: .atomic)
    }
}

func removeMiis(in folder: URL) throws {
    let dir = miisFolder(in: folder)
    if FileManager.default.fileExists(atPath: dir.path) { try FileManager.default.removeItem(at: dir) }
}
