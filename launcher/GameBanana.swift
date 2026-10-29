// Mush8 - GameBanana.swift
import Foundation
import CryptoKit

struct BananaMod: Identifiable, Sendable {
    let id: Int
    let name: String
    let author: String
    let pageURL: URL
    let thumbnailURL: URL?
    let heroURL: URL?
    let categoryName: String?
    let version: String?
    let likeCount: Int
    let viewCount: Int
    let dateUpdated: Int?
}

struct BananaFile: Identifiable, Sendable {
    let id: Int
    let name: String
    let size: Int64
    let url: URL
    let md5: String?
    let version: String?
    let note: String?
    let downloadCount: Int
    let dateAdded: Int?
}

struct BananaDownloadProgress: Sendable {
    let receivedBytes: Int64
    let totalBytes: Int64?

    var fraction: Double? {
        guard let totalBytes, totalBytes > 0 else { return nil }
        return min(1, max(0, Double(receivedBytes) / Double(totalBytes)))
    }
}

struct BananaCategory: Identifiable, Sendable {
    let id: Int
    let name: String
}

enum BananaSort: String, CaseIterable, Identifiable, Sendable {
    case newest = "Generic_Newest"
    case updated = "Generic_LatestModified"
    case mostDownloaded = "Generic_MostDownloaded"
    case mostLiked = "Generic_MostLiked"
    var id: String { rawValue }
    var label: String {
        switch self {
        case .newest: return "Newest"
        case .updated: return "Updated"
        case .mostDownloaded: return "Popular"
        case .mostLiked: return "Top Rated"
        }
    }
}

enum GameBanana {
    static let gameID = 6507
    static let downloadLimit: Int64 = 512 * 1024 * 1024
    private static let responseLimit = 4 * 1024 * 1024

    static func categories() async throws -> [BananaCategory] {
        let records: [CategoryRecord] = try await request("Mod/Categories", parameters: [
            URLQueryItem(name: "_idGameRow", value: String(gameID)), URLQueryItem(name: "_sSort", value: "count"),
        ])
        return records.filter { $0.id > 0 && !$0.name.isEmpty && !$0.obsolete }.map {
            BananaCategory(id: $0.id, name: $0.name)
        }
    }

    static func search(_ query: String, page: Int = 1, categoryID: Int? = nil, sort: BananaSort = .newest) async throws -> [BananaMod] {
        let query = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard page > 0, query.count <= 200, categoryID.map({ $0 > 0 }) ?? true else { throw ModError("Invalid search, category, or page number.") }
        if let id = try modID(query) { return [try await details(id).mod] }
        guard !query.contains(",") else { throw ModError("GameBanana search cannot match commas. Remove commas or paste the mod URL.") }
        var parameters = [URLQueryItem(name: "_nPage", value: String(page)), URLQueryItem(name: "_nPerpage", value: "15"),
                          URLQueryItem(name: "_sSort", value: sort.rawValue), URLQueryItem(name: "_aFilters[Generic_Game]", value: String(gameID))]
        if let categoryID { parameters.append(URLQueryItem(name: "_aFilters[Generic_Category]", value: String(categoryID))) }
        if !query.isEmpty { parameters.append(URLQueryItem(name: "_aFilters[Generic_Name]", value: "contains,\(query)")) }
        let response: Listing = try await request("Mod/Index", parameters: parameters)
        return response.records.filter { $0.model == "Mod" && $0.id > 0 && $0.game.id == gameID }.map(\.mod)
    }

    static func files(for mod: BananaMod) async throws -> [BananaFile] {
        let detail = try await details(mod.id)
        guard let files = detail.files else { throw ModError("GameBanana returned no file information.") }
        return try files.filter { !$0.archived }.map { file in
            guard file.id > 0, file.size >= 0, trusted(file.url), validMD5(file.md5) else {
                throw ModError("GameBanana returned invalid download information.")
            }
            return BananaFile(id: file.id, name: file.name, size: file.size, url: file.url, md5: file.md5?.nilIfEmpty,
                              version: file.version?.nilIfEmpty, note: file.description?.nilIfEmpty,
                              downloadCount: file.downloadCount ?? 0, dateAdded: file.dateAdded)
        }
    }

    static func download(_ file: BananaFile, to destination: URL,
                         progress: @escaping @Sendable (BananaDownloadProgress) -> Void = { _ in }) async throws {
        try Task.checkCancellation()
        guard trusted(file.url), destination.isFileURL, file.size >= 0, validMD5(file.md5) else {
            throw ModError("Invalid download URL, checksum, or destination.")
        }
        guard file.size <= downloadLimit else { throw ModError("This download exceeds the 512 MiB limit.") }
        let expectedBytes = file.size > 0 ? file.size : nil
        let guardDelegate = NetworkGuard(limit: downloadLimit, expectedBytes: expectedBytes, progress: progress)
        let session = session(resourceTimeout: 300, delegate: guardDelegate)
        defer { session.invalidateAndCancel() }
        progress(BananaDownloadProgress(receivedBytes: 0, totalBytes: expectedBytes))
        do {
            let (temporary, response) = try await guardDelegate.download(URLRequest(url: file.url), in: session)
            defer { try? FileManager.default.removeItem(at: temporary) }
            try check(response, guardDelegate: guardDelegate)
            try verify(temporary, file: file)
            try Task.checkCancellation()
            let size = (try FileManager.default.attributesOfItem(atPath: temporary.path)[.size] as? NSNumber)?.int64Value ?? 0
            try FileManager.default.createDirectory(at: destination.deletingLastPathComponent(), withIntermediateDirectories: true)
            // Keep an existing destination intact if a caller accidentally reuses its path.
            try FileManager.default.moveItem(at: temporary, to: destination)
            progress(BananaDownloadProgress(receivedBytes: size, totalBytes: size > 0 ? size : nil))
        } catch {
            if Task.isCancelled { throw CancellationError() }
            throw guardDelegate.failure ?? error
        }
    }

    private static func details(_ id: Int) async throws -> Record {
        guard id > 0 else { throw ModError("Invalid GameBanana mod ID.") }
        let detail: Record = try await request("Mod/\(id)", parameters: [URLQueryItem(name: "_csvProperties", value: "_idRow,_sName,_aSubmitter,_aGame,_aFiles,_aPreviewMedia,_aRootCategory,_aCategory,_nLikeCount,_nViewCount,_tsDateUpdated")])
        guard detail.id == id, detail.game.id == gameID else {
            throw ModError("This mod is not for Mario Kart 8 Deluxe.")
        }
        return detail
    }

    private static func session(resourceTimeout: TimeInterval = 60, delegate: URLSessionDelegate? = nil) -> URLSession {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.timeoutIntervalForRequest = 60
        configuration.timeoutIntervalForResource = resourceTimeout
        configuration.httpAdditionalHeaders = ["User-Agent": "Mush8/0.1", "Accept": "application/json"]
        return URLSession(configuration: configuration, delegate: delegate, delegateQueue: nil)
    }

    private static func request<T: Decodable>(_ path: String, parameters: [URLQueryItem]) async throws -> T {
        try Task.checkCancellation()
        var components = URLComponents(string: "https://gamebanana.com/apiv11/\(path)")!
        components.queryItems = parameters
        let guardDelegate = NetworkGuard(limit: Int64(responseLimit))
        let session = session()
        defer { session.invalidateAndCancel() }
        do {
            let (bytes, response) = try await session.bytes(for: URLRequest(url: components.url!), delegate: guardDelegate)
            try check(response, guardDelegate: guardDelegate)
            guard response.expectedContentLength <= responseLimit else { throw ModError("GameBanana's response is too large.") }
            var data = Data()
            for try await byte in bytes {
                guard data.count < responseLimit else { throw ModError("GameBanana's response is too large.") }
                data.append(byte)
                if data.count.isMultiple(of: 4096) { try Task.checkCancellation() }
            }
            try Task.checkCancellation()
            guard !data.isEmpty else { throw ModError("GameBanana returned an empty response. Please try again later.") }
            do { return try JSONDecoder().decode(T.self, from: data) }
            catch { throw ModError("GameBanana returned an unexpected response. Please try again later.") }
        } catch {
            if Task.isCancelled { throw CancellationError() }
            throw guardDelegate.failure ?? error
        }
    }

    private static func check(_ response: URLResponse, guardDelegate: NetworkGuard) throws {
        if let failure = guardDelegate.failure { throw failure }
        guard let response = response as? HTTPURLResponse, let url = response.url, trusted(url) else {
            throw ModError("GameBanana returned an invalid response URL.")
        }
        guard response.statusCode == 200 else {
            if response.statusCode == 429 { throw ModError("GameBanana is rate limiting requests. Please wait before trying again.") }
            throw ModError("GameBanana returned HTTP \(response.statusCode). Please try again later.")
        }
    }

    fileprivate static func trusted(_ url: URL) -> Bool {
        guard url.scheme?.lowercased() == "https", let host = url.host?.lowercased(),
              host == "gamebanana.com" || host.hasSuffix(".gamebanana.com"),
              url.user == nil, url.password == nil, url.port == nil || url.port == 443 else { return false }
        return true
    }

    private static func modID(_ query: String) throws -> Int? {
        if !query.isEmpty && query.utf8.allSatisfy({ $0 >= 48 && $0 <= 57 }) {
            guard let id = Int(query), id > 0 else { throw ModError("Invalid GameBanana mod ID.") }
            return id
        }
        guard query.contains("://") else { return nil }
        guard let url = URL(string: query), trusted(url), ["gamebanana.com", "www.gamebanana.com"].contains(url.host?.lowercased() ?? "") else {
            throw ModError("Enter a GameBanana mod name, ID, or https://gamebanana.com/mods/… URL.")
        }
        let parts = url.path.split(separator: "/")
        guard parts.count == 2, parts[0] == "mods", parts[1].utf8.allSatisfy({ $0 >= 48 && $0 <= 57 }),
              let id = Int(parts[1]), id > 0 else { throw ModError("Enter a GameBanana /mods/… URL.") }
        return id
    }

    private static func validMD5(_ value: String?) -> Bool {
        guard let value, !value.isEmpty else { return true }
        return value.utf8.count == 32 && value.utf8.allSatisfy { (48...57).contains($0) || (65...70).contains($0) || (97...102).contains($0) }
    }

    private static func verify(_ url: URL, file: BananaFile) throws {
        let size = (try FileManager.default.attributesOfItem(atPath: url.path)[.size] as? NSNumber)?.int64Value ?? -1
        guard size >= 0, size <= downloadLimit else { throw ModError("This download exceeds the 512 MiB limit.") }
        guard file.size == 0 || size == file.size else { throw ModError("The downloaded file size does not match GameBanana's metadata.") }
        if let expected = file.md5, !expected.isEmpty {
            guard validMD5(expected) else { throw ModError("GameBanana returned an invalid checksum.") }
            let handle = try FileHandle(forReadingFrom: url)
            defer { try? handle.close() }
            var hash = Insecure.MD5()
            while let data = try handle.read(upToCount: 1024 * 1024), !data.isEmpty {
                try Task.checkCancellation()
                hash.update(data: data)
            }
            let actual = hash.finalize().map { String(format: "%02x", $0) }.joined()
            guard actual.caseInsensitiveCompare(expected) == .orderedSame else { throw ModError("The downloaded file failed its checksum check.") }
        }
    }

    private struct Submitter: Decodable { let name: String; enum CodingKeys: String, CodingKey { case name = "_sName" } }
    private struct Game: Decodable { let id: Int; enum CodingKeys: String, CodingKey { case id = "_idRow" } }
    private struct CategoryRef: Decodable { let name: String; enum CodingKeys: String, CodingKey { case name = "_sName" } }
    private struct PreviewImage: Decodable {
        let baseURL: String?
        let file: String?
        let file220: String?
        let file530: String?
        enum CodingKeys: String, CodingKey { case baseURL = "_sBaseUrl", file = "_sFile", file220 = "_sFile220", file530 = "_sFile530" }
        func resolved(_ name: String?) -> URL? {
            guard let baseURL, let name = name?.nilIfEmpty, let url = URL(string: "\(baseURL)/\(name)"), GameBanana.trusted(url) else { return nil }
            return url
        }
        var thumb: URL? { resolved(file220) ?? resolved(file530) ?? resolved(file) }
        var hero: URL? { resolved(file530) ?? resolved(file) ?? resolved(file220) }
    }
    private struct PreviewMedia: Decodable {
        let images: [PreviewImage]?
        enum CodingKeys: String, CodingKey { case images = "_aImages" }
    }
    private struct CategoryRecord: Decodable {
        let id: Int
        let name: String
        let obsolete: Bool
        enum CodingKeys: String, CodingKey { case id = "_idRow", name = "_sName", obsolete = "_bIsObsolete" }
    }
    private struct Record: Decodable {
        let id: Int
        let name: String
        let model: String?
        let submitter: Submitter?
        let game: Game
        let previewMedia: PreviewMedia?
        let rootCategory: CategoryRef?
        let subCategory: CategoryRef?
        let category: CategoryRef?
        let files: [File]?
        let dateUpdated: Int?
        let version: String?
        let likeCount: Int?
        let viewCount: Int?
        let dateModified: Int?
        let dateAdded: Int?
        var mod: BananaMod {
            let image = previewMedia?.images?.first
            return BananaMod(id: id, name: name, author: submitter?.name ?? "Unknown author",
                             pageURL: URL(string: "https://gamebanana.com/mods/\(id)")!,
                             thumbnailURL: image?.thumb, heroURL: image?.hero,
                             categoryName: category?.name.nilIfEmpty ?? subCategory?.name.nilIfEmpty ?? rootCategory?.name.nilIfEmpty,
                             version: version?.nilIfEmpty,
                             likeCount: likeCount ?? 0, viewCount: viewCount ?? 0,
                             dateUpdated: dateUpdated ?? dateModified ?? dateAdded)
        }
        enum CodingKeys: String, CodingKey {
            case id = "_idRow", name = "_sName", model = "_sModelName", submitter = "_aSubmitter", game = "_aGame"
            case previewMedia = "_aPreviewMedia", rootCategory = "_aRootCategory", subCategory = "_aSubCategory"
            case version = "_sVersion", likeCount = "_nLikeCount", viewCount = "_nViewCount"
            case dateModified = "_tsDateModified", dateAdded = "_tsDateAdded", dateUpdated = "_tsDateUpdated"
            case category = "_aCategory", files = "_aFiles"
        }
    }
    private struct Listing: Decodable { let records: [Record]; enum CodingKeys: String, CodingKey { case records = "_aRecords" } }
    private struct File: Decodable {
        let id: Int
        let name: String
        let size: Int64
        let url: URL
        let md5: String?
        let archived: Bool
        let version: String?
        let description: String?
        let downloadCount: Int?
        let dateAdded: Int?
        enum CodingKeys: String, CodingKey {
            case id = "_idRow", name = "_sFile", size = "_nFilesize", url = "_sDownloadUrl", md5 = "_sMd5Checksum", archived = "_bIsArchived"
            case version = "_sVersion", description = "_sDescription", downloadCount = "_nDownloadCount", dateAdded = "_tsDateAdded"
        }
    }
}

private extension String { var nilIfEmpty: String? { isEmpty ? nil : self } }

private final class NetworkGuard: NSObject, URLSessionDownloadDelegate, @unchecked Sendable {
    private let limit: Int64
    private let expectedBytes: Int64?
    private let progress: (@Sendable (BananaDownloadProgress) -> Void)?
    private let lock = NSLock()
    private var rejected: ModError?
    private var lastProgressAt: TimeInterval = 0
    private var downloadTask: URLSessionDownloadTask?
    private var continuation: CheckedContinuation<(URL, URLResponse), any Error>?
    private var cancelled = false
    var failure: ModError? { lock.lock(); defer { lock.unlock() }; return rejected }
    init(limit: Int64, expectedBytes: Int64? = nil, progress: (@Sendable (BananaDownloadProgress) -> Void)? = nil) {
        self.limit = limit
        self.expectedBytes = expectedBytes
        self.progress = progress
    }
    private func reject(_ message: String) { lock.lock(); defer { lock.unlock() }; if rejected == nil { rejected = ModError(message) } }

    func download(_ request: URLRequest, in session: URLSession) async throws -> (URL, URLResponse) {
        try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                let task = session.downloadTask(with: request)
                lock.lock()
                if cancelled {
                    lock.unlock()
                    task.cancel()
                    continuation.resume(throwing: CancellationError())
                    return
                }
                self.downloadTask = task
                self.continuation = continuation
                lock.unlock()
                task.resume()
            }
        } onCancel: {
            self.cancel()
        }
    }

    private func cancel() {
        lock.lock()
        cancelled = true
        let task = downloadTask
        lock.unlock()
        task?.cancel()
    }

    private func complete(_ result: Result<(URL, URLResponse), any Error>) {
        lock.lock()
        let continuation = continuation
        self.continuation = nil
        downloadTask = nil
        let cancelled = cancelled
        lock.unlock()
        if let continuation, !cancelled {
            continuation.resume(with: result)
        } else {
            if case .success(let (file, _)) = result { try? FileManager.default.removeItem(at: file) }
            continuation?.resume(throwing: CancellationError())
        }
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse,
                    newRequest request: URLRequest, completionHandler: @escaping @Sendable (URLRequest?) -> Void) {
        guard let url = request.url, GameBanana.trusted(url) else {
            reject("GameBanana redirected to an untrusted download URL.")
            completionHandler(nil)
            return
        }
        completionHandler(request)
    }

    func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didWriteData bytesWritten: Int64,
                    totalBytesWritten: Int64, totalBytesExpectedToWrite: Int64) {
        if totalBytesWritten > limit || totalBytesExpectedToWrite > limit {
            reject("This download exceeds the 512 MiB limit.")
            downloadTask.cancel()
            return
        }
        guard let progress else { return }
        let total = totalBytesExpectedToWrite > 0 ? totalBytesExpectedToWrite : expectedBytes
        let now = ProcessInfo.processInfo.systemUptime
        lock.lock()
        let report = now - lastProgressAt >= 0.1 || totalBytesWritten == total
        if report { lastProgressAt = now }
        lock.unlock()
        if report { progress(BananaDownloadProgress(receivedBytes: totalBytesWritten, totalBytes: total)) }
    }

    func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didFinishDownloadingTo location: URL) {
        let saved = FileManager.default.temporaryDirectory.appendingPathComponent("Mush8-transfer-\(UUID().uuidString)")
        do {
            guard let response = downloadTask.response else { throw ModError("GameBanana returned no download response.") }
            try FileManager.default.moveItem(at: location, to: saved)
            complete(.success((saved, response)))
        } catch {
            complete(.failure(error))
        }
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: (any Error)?) {
        complete(.failure(error ?? ModError("GameBanana download completed without a file.")))
    }
}
