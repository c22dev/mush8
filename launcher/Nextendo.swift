// Mush8 - Nextendo.swift
// Constantin Clerc (c22dev) - 0.1
import AuthenticationServices
import CryptoKit
import Foundation
import Network
import Security
import SwiftUI

struct NextendoLogin {
    let pid: UInt64
    let name: String
    let token: String
    let access: String
}

enum Nextendo {
    static let clientID = "nxc_PZv-XFn79sLo"
    static let scopes = "identity friends game.matchmaking presence"
    static let authorize = URL(string: "https://nextendo.network/oauth/authorize")!
    static let tokenURL = URL(string: "https://nextendo.network/api/oauth/token")!
    static let nexTokenURL = URL(string: "https://nextendo.network/api/nex-token")!
    static let onlineStatusURL = URL(string: "https://nextendo.network/api/online-status")!
    static let callbackPort: UInt16 = 47823

    struct Failure: LocalizedError {
        let errorDescription: String?
        init(_ text: String) { errorDescription = text }
    }

    static var signedIn: Bool { !NextendoKeychain.read("refresh_token").isEmpty }

    @MainActor static func signIn() async throws -> NextendoLogin {
        let verifier = randomURLSafe(32)
        let challenge = base64url(Data(SHA256.hash(data: Data(verifier.utf8))))
        let state = randomURLSafe(16)
        let callback = try await LoopbackCallback.start(preferring: callbackPort, state: state)
        defer { callback.stop() }
        let redirect = "http://127.0.0.1:\(callback.port)/callback"
        var c = URLComponents(url: authorize, resolvingAgainstBaseURL: false)!
        c.queryItems = [
            URLQueryItem(name: "client_id", value: clientID),
            URLQueryItem(name: "redirect_uri", value: redirect),
            URLQueryItem(name: "response_type", value: "code"),
            URLQueryItem(name: "scope", value: scopes),
            URLQueryItem(name: "state", value: state),
            URLQueryItem(name: "code_challenge", value: challenge),
            URLQueryItem(name: "code_challenge_method", value: "S256"),
        ]
        let session = ASWebAuthenticationSession(url: c.url!, callbackURLScheme: "mush8") { _, error in
            callback.fail(error ?? Failure("Sign-in was cancelled"))
        }
        let anchor = AuthAnchor()
        session.presentationContextProvider = anchor
        session.start()
        defer { withExtendedLifetime(anchor) { session.cancel() } }
        let query = try await callback.wait()
        session.cancel()
        if let error = query["error"] { throw Failure("Nextendo refused the sign-in (\(error))") }
        guard let code = query["code"], !code.isEmpty else { throw Failure("Nextendo sent no sign-in code") }
        if let stale = inFlight { _ = try? await stale.value }
        try await token(["grant_type": "authorization_code", "client_id": clientID, "code": code, "code_verifier": verifier, "redirect_uri": redirect])
        return try await freshLogin()
    }

    @MainActor private static var inFlight: Task<NextendoLogin, Error>?

    @MainActor static func gameLogin() async throws -> NextendoLogin {
        if let task = inFlight { return try await task.value }
        let task = Task { try await freshLogin() }
        inFlight = task
        defer { inFlight = nil }
        return try await task.value
    }

    private static func freshLogin() async throws -> NextendoLogin {
        let refresh = NextendoKeychain.read("refresh_token")
        guard !refresh.isEmpty else { throw Failure("Sign in to Nextendo under Multiplayer first") }
        let access = try await token(["grant_type": "refresh_token", "client_id": clientID, "refresh_token": refresh])
        var request = URLRequest(url: nexTokenURL, timeoutInterval: 12)
        request.setValue("Bearer \(access)", forHTTPHeaderField: "Authorization")
        let json = try await send(request)
        guard let nex = json["nex_token"] as? String, !nex.isEmpty, let pid = (json["pid"] as? NSNumber)?.uint64Value, pid != 0 else {
            throw Failure("Nextendo gave no game token: the account needs a confirmed e-mail and a linked Discord account")
        }
        return NextendoLogin(pid: pid, name: json["username"] as? String ?? "", token: nex, access: access)
    }

    static func freshAccess() async throws -> String {
        let refresh = NextendoKeychain.read("refresh_token")
        guard !refresh.isEmpty else { throw Failure("Sign in to Nextendo under Multiplayer first") }
        return try await token(["grant_type": "refresh_token", "client_id": clientID, "refresh_token": refresh])
    }

    enum Gate: Equatable {
        case allowed
        case refused(String)
        case unknown(String)

        init(status: Int, json: [String: Any]) {
            guard status == 200 else {
                self = .unknown("HTTP \(status)" + ((json["error"] as? String).map { ": \($0)" } ?? ""))
                return
            }
            switch json["allow"] as? Bool {
            case true?: self = .allowed
            case false?: self = .refused(json["reason"] as? String ?? "")
            case nil: self = .unknown("HTTP 200 without an answer")
            }
        }

        var log: String {
            switch self {
            case .allowed: "allowed"
            case .refused(let reason): "refused (\(reason.isEmpty ? "no reason given" : reason))"
            case .unknown(let detail): "not checked (\(detail))"
            }
        }

        var problem: String? {
            guard case .refused(let reason) = self else { return nil }
            let why = switch reason {
            case "discord_unlinked": "this account isn't linked to Discord. Open your account page on nextendo.network and use \"link on Discord\""
            case "unverified": "this account's e-mail address isn't confirmed. Confirm it from the e-mail Nextendo sent you"
            case "elsewhere": "this account is already online on another device. Leave online play there first"
            case "disabled": "this account is disabled"
            case "unknown": "it doesn't recognise this account. Sign out and sign in again under Multiplayer"
            default: "it gave the reason \"\(reason.isEmpty ? "none" : reason)\""
            }
            return "Nextendo refuses online play: \(why)."
        }
    }

    static func onlineGate(_ token: String) async -> Gate {
        var request = URLRequest(url: onlineStatusURL, timeoutInterval: 8)
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        do {
            let (data, response) = try await URLSession(configuration: .ephemeral).data(for: request)
            let json = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] ?? [:]
            return Gate(status: (response as? HTTPURLResponse)?.statusCode ?? 0, json: json)
        } catch {
            return .unknown(error.localizedDescription)
        }
    }

    static func signOut() {
        try? NextendoKeychain.write("refresh_token", "")
    }

    @discardableResult
    private static func token(_ form: [String: String]) async throws -> String {
        var request = URLRequest(url: tokenURL, timeoutInterval: 12)
        request.httpMethod = "POST"
        request.setValue("application/x-www-form-urlencoded", forHTTPHeaderField: "Content-Type")
        var allowed = CharacterSet.alphanumerics
        allowed.insert(charactersIn: "-._~")
        request.httpBody = form.map { "\($0.key)=\($0.value.addingPercentEncoding(withAllowedCharacters: allowed) ?? "")" }.joined(separator: "&").data(using: .utf8)
        let json: [String: Any]
        do {
            json = try await send(request)
        } catch let e as Failure where e.errorDescription == "invalid_grant" && form["grant_type"] == "refresh_token" {
            signOut()
            throw Failure("Your Nextendo sign-in expired, sign in again")
        }
        guard let access = json["access_token"] as? String, !access.isEmpty else { throw Failure("Nextendo sent no access token") }
        if let refresh = json["refresh_token"] as? String, !refresh.isEmpty { try NextendoKeychain.write("refresh_token", refresh) }
        return access
    }

    private static func send(_ request: URLRequest) async throws -> [String: Any] {
        let (data, response) = try await URLSession(configuration: .ephemeral).data(for: request)
        let json = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] ?? [:]
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        guard status == 200 else { throw Failure(json["error"] as? String ?? "Nextendo answered \(status)") }
        return json
    }

    private static func base64url(_ data: Data) -> String {
        data.base64EncodedString().replacingOccurrences(of: "+", with: "-").replacingOccurrences(of: "/", with: "_").trimmingCharacters(in: CharacterSet(charactersIn: "="))
    }

    private static func randomURLSafe(_ count: Int) -> String {
        var bytes = [UInt8](repeating: 0, count: count)
        _ = SecRandomCopyBytes(kSecRandomDefault, count, &bytes)
        return base64url(Data(bytes))
    }
}

private final class AuthAnchor: NSObject, ASWebAuthenticationPresentationContextProviding {
    func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
        #if os(iOS)
        let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
        let windows = scenes.flatMap(\.windows)
        return windows.first(where: \.isKeyWindow) ?? windows.first ?? ASPresentationAnchor(windowScene: scenes[0])
        #else
        return NSApp.keyWindow ?? NSApp.windows.first ?? ASPresentationAnchor()
        #endif
    }
}

private final class LoopbackCallback: @unchecked Sendable {
    private let listener: NWListener
    private let state: String
    private let lock = NSLock()
    private var result: Result<[String: String], Error>?
    private var waiter: CheckedContinuation<[String: String], Error>?
    var port: UInt16 { listener.port?.rawValue ?? 0 }

    private init(_ listener: NWListener, state: String) {
        self.listener = listener
        self.state = state
    }

    static func start(preferring preferred: UInt16, state: String) async throws -> LoopbackCallback {
        do {
            return try await start(on: NWEndpoint.Port(rawValue: preferred)!, state: state)
        } catch {
            return try await start(on: .any, state: state)
        }
    }

    private static func start(on port: NWEndpoint.Port, state: String) async throws -> LoopbackCallback {
        let params = NWParameters.tcp
        params.requiredLocalEndpoint = .hostPort(host: .ipv4(.loopback), port: port)
        let callback = LoopbackCallback(try NWListener(using: params), state: state)
        callback.listener.newConnectionHandler = { [weak callback] c in callback?.serve(c) }
        try await withCheckedThrowingContinuation { (ready: CheckedContinuation<Void, Error>) in
            let once = Once()
            callback.listener.stateUpdateHandler = { state in
                switch state {
                case .ready:
                    if once.first() { ready.resume() }
                case .failed(let e), .waiting(let e):
                    if once.first() {
                        callback.listener.cancel()
                        ready.resume(throwing: e)
                    }
                default:
                    break
                }
            }
            callback.listener.start(queue: .global())
        }
        return callback
    }

    private func serve(_ c: NWConnection) {
        c.start(queue: .global())
        c.receive(minimumIncompleteLength: 1, maximumLength: 16384) { [weak self] data, _, _, _ in
            let line = data.flatMap { String(data: $0, encoding: .utf8) }?.components(separatedBy: "\r\n").first ?? ""
            let parts = line.split(separator: " ")
            var query: [String: String] = [:]
            let url = parts.count >= 2 && parts[0] == "GET" ? URLComponents(string: "http://127.0.0.1" + parts[1]) : nil
            for item in url?.queryItems ?? [] { query[item.name] = item.value ?? "" }
            guard url?.path == "/callback", query["state"] == self?.state else {
                c.send(content: Data("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".utf8), completion: .contentProcessed { _ in c.cancel() })
                return
            }
            self?.finish(.success(query))
            c.send(content: Data("HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n".utf8), completion: .contentProcessed { _ in c.cancel() })
        }
    }

    func fail(_ error: Error) {
        finish(.failure(error))
    }

    private func finish(_ r: Result<[String: String], Error>) {
        lock.lock()
        guard result == nil else {
            lock.unlock()
            return
        }
        result = r
        let w = waiter
        waiter = nil
        lock.unlock()
        w?.resume(with: r)
    }

    func wait() async throws -> [String: String] {
        try await withCheckedThrowingContinuation { c in
            lock.lock()
            if let r = result {
                lock.unlock()
                c.resume(with: r)
            } else {
                waiter = c
                lock.unlock()
            }
        }
    }

    func stop() {
        listener.cancel()
    }
}

private final class Once: @unchecked Sendable {
    private let lock = NSLock()
    private var done = false

    func first() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        let was = done
        done = true
        return !was
    }
}

enum NextendoKeychain {
    private static func query(_ key: String) -> [String: Any] {
        [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: "Mush8 Nextendo", kSecAttrAccount as String: key]
    }

    static func read(_ key: String) -> String {
        var q = query(key)
        q[kSecReturnData as String] = true
        var out: CFTypeRef?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess, let data = out as? Data else { return "" }
        return String(decoding: data, as: UTF8.self)
    }

    static func write(_ key: String, _ value: String) throws {
        let deleted = SecItemDelete(query(key) as CFDictionary)
        guard deleted == errSecSuccess || deleted == errSecItemNotFound else { throw Nextendo.Failure("Keychain error \(deleted)") }
        guard !value.isEmpty else { return }
        var q = query(key)
        q[kSecValueData as String] = Data(value.utf8)
        q[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        let added = SecItemAdd(q as CFDictionary, nil)
        guard added == errSecSuccess else { throw Nextendo.Failure("Couldn't keep the Nextendo sign-in in the Keychain (error \(added))") }
    }
}

struct NextendoSignIn: View {
    @Binding var pid: UInt64
    @Binding var name: String
    @State private var busy = false
    @State private var error: String?

    var body: some View {
        if pid != 0 && Nextendo.signedIn {
            HStack {
                Text("Signed in as \(name.isEmpty ? String(pid) : name)")
                Spacer()
                Button("Sign out") {
                    Nextendo.signOut()
                    pid = 0
                    name = ""
                }
            }
        } else {
            HStack {
                Button(busy ? "Signing in…" : "Sign in with Nextendo", action: signIn).disabled(busy)
                if let error { Text(error).font(.caption).foregroundStyle(.red) }
            }
        }
    }

    func signIn() {
        guard !busy else { return }
        busy = true
        error = nil
        Task { @MainActor in
            defer { busy = false }
            do {
                let login = try await Nextendo.signIn()
                pid = login.pid
                name = login.name
            } catch let e as ASWebAuthenticationSessionError where e.code == .canceledLogin {
            } catch {
                self.error = error.localizedDescription
            }
        }
    }
}
