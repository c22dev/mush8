// Mush8 - ModBrowser.swift
import SwiftUI

struct ModBrowserView: View {
    let folder: URL
    let onInstalled: (String) -> Void
    @Environment(\.dismiss) private var dismiss
    #if os(iOS)
    @Environment(\.horizontalSizeClass) private var horizontalSizeClass
    #endif
    @FocusState private var searchFocused: Bool
    @State private var query = ""
    @State private var submittedQuery = ""
    @State private var categoryID: Int?
    @State private var sort: BananaSort = .newest
    @State private var categories: [BananaCategory] = []
    @State private var categoryError: String?
    @State private var loadingCategories = false
    @State private var page = 1
    @State private var results: [BananaMod] = []
    @State private var selection: Int?
    @State private var files: [BananaFile] = []
    @State private var installedFolderNames: Set<String> = []
    @State private var searching = false
    @State private var loadingFiles = false
    @State private var installing = false
    @State private var activeFile: BananaFile?
    @State private var transferID: UUID?
    @State private var downloadProgress: BananaDownloadProgress?
    @State private var message: String?
    @State private var error: String?
    @State private var searchTask: Task<Void, Never>?
    @State private var categoriesTask: Task<Void, Never>?
    @State private var filesTask: Task<Void, Never>?
    @State private var installTask: Task<Void, Never>?

    private var selectedMod: BananaMod? { results.first { $0.id == selection } }
    private var directLookup: Bool {
        Int(submittedQuery) != nil || URL(string: submittedQuery)?.host != nil
    }

    var body: some View {
        NavigationSplitView {
            sidebar
                .navigationTitle("GameBanana")
                .navigationSplitViewColumnWidth(min: 320, ideal: 380, max: 440)
                #if os(iOS)
                .toolbar {
                    if horizontalSizeClass == .compact {
                        ToolbarItem(placement: .confirmationAction) { doneButton }
                    }
                }
                #endif
        } detail: {
            detail
                .navigationTitle(selectedMod?.name ?? "Mod Details")
                .toolbar {
                    ToolbarItem(placement: .confirmationAction) { doneButton }
                }
                #if os(iOS)
                .navigationBarTitleDisplayMode(.inline)
                #endif
        }
        .navigationSplitViewStyle(.balanced)
        .safeAreaInset(edge: .bottom, spacing: 0) {
            if installing, let activeFile {
                ModDownloadStatus(file: activeFile, progress: downloadProgress) { installTask?.cancel() }
                    .padding(16)
                    .background(.bar)
            } else if message != nil || error != nil {
                statusMessage
            }
        }
        #if os(macOS)
        .frame(minWidth: 980, idealWidth: 1100, maxWidth: .infinity, minHeight: 640, idealHeight: 740, maxHeight: .infinity)
        #endif
        .interactiveDismissDisabled(installing)
        .onAppear {
            refreshInstalled()
            search(page: 1)
            loadCategories()
        }
        .onChange(of: selection) { _, _ in loadFiles() }
        .onChange(of: categoryID) { _, _ in search(page: 1) }
        .onChange(of: sort) { _, _ in search(page: 1) }
        .onDisappear {
            searchTask?.cancel()
            categoriesTask?.cancel()
            filesTask?.cancel()
            installTask?.cancel()
        }
    }

    private var doneButton: some View {
        Button("Done") { dismiss() }
            .keyboardShortcut(.cancelAction)
            .disabled(installing)
    }

    private var sidebar: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 10) {
                HStack(spacing: 8) {
                    HStack(spacing: 6) {
                        Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                        TextField("Search mods, URL, or ID", text: $query)
                            .textFieldStyle(.plain)
                            .focused($searchFocused)
                            .onSubmit(submit)
                            .autocorrectionDisabled()
                            #if os(iOS)
                            .textInputAutocapitalization(.never)
                            .submitLabel(.search)
                            #endif
                        if !query.isEmpty {
                            Button {
                                query = ""
                                if !submittedQuery.isEmpty {
                                    submittedQuery = ""
                                    search(page: 1)
                                }
                            } label: {
                                Image(systemName: "xmark.circle.fill").foregroundStyle(.secondary)
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel("Clear search")
                        }
                    }
                    .padding(.horizontal, 10)
                    .padding(.vertical, 7)
                    .background(.quaternary.opacity(0.6), in: RoundedRectangle(cornerRadius: 10))

                    Button("Search", systemImage: "magnifyingglass", action: submit)
                        .labelStyle(.iconOnly)
                        .buttonStyle(.borderedProminent)
                        .accessibilityLabel("Search mods")

                    Picker("Sort", selection: $sort) {
                        ForEach(BananaSort.allCases) { item in
                            Text(item.label).tag(item)
                        }
                    }
                    .pickerStyle(.menu)
                    .labelsHidden()
                    .fixedSize()
                    .disabled(directLookup)
                }

                if !categories.isEmpty && !directLookup {
                    ScrollView(.horizontal, showsIndicators: false) {
                        HStack(spacing: 6) {
                            sectionChip(title: "All", id: nil)
                            ForEach(categories) { category in
                                sectionChip(title: category.name, id: category.id)
                            }
                        }
                        .padding(.vertical, 2)
                    }
                }

                if loadingCategories {
                    ProgressView("Loading sections…").font(.caption)
                } else if categoryError != nil {
                    HStack {
                        Text("Sections unavailable. All mods are still searchable.")
                            .font(.caption).foregroundStyle(.secondary)
                        Button("Retry", action: loadCategories).font(.caption)
                    }
                } else if directLookup {
                    Text("A mod URL or ID opens that mod directly.")
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
            .padding(12)
            .disabled(installing)
            Divider()
            List(results, id: \.id, selection: $selection) { mod in
                NavigationLink(value: mod.id) {
                    modRow(mod)
                }
            }
            .listStyle(.plain)
            .disabled(searching || installing)
            .overlay {
                if searching {
                    ProgressView("Searching GameBanana…")
                        .padding().background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12))
                } else if results.isEmpty {
                    ContentUnavailableView("No mods found", systemImage: "magnifyingglass", description: Text("Try another name or section."))
                }
            }
            Divider()
            HStack {
                Button("Previous", systemImage: "chevron.left") { search(page: page - 1) }
                    .labelStyle(.iconOnly).accessibilityLabel("Previous page")
                    .disabled(page == 1 || searching || installing || directLookup)
                Spacer()
                Text("Page \(page)").font(.callout).monospacedDigit().foregroundStyle(.secondary)
                Spacer()
                Button("Next", systemImage: "chevron.right") { search(page: page + 1) }
                    .labelStyle(.iconOnly).accessibilityLabel("Next page")
                    .disabled(results.isEmpty || searching || installing || directLookup)
            }
            .buttonStyle(.bordered)
            .padding(10)
        }
    }

    private func sectionChip(title: String, id: Int?) -> some View {
        let active = categoryID == id
        return Button {
            categoryID = id
        } label: {
            Text(title)
                .font(.caption.weight(active ? .semibold : .regular))
                .padding(.horizontal, 10)
                .padding(.vertical, 5)
                .background(active ? Color.accentColor : Color.secondary.opacity(0.16), in: Capsule())
                .foregroundStyle(active ? Color.white : Color.primary)
        }
        .buttonStyle(.plain)
    }

    private func modRow(_ mod: BananaMod) -> some View {
        HStack(alignment: .top, spacing: 10) {
            ModThumbnail(url: mod.thumbnailURL, width: 76, height: 46, cornerRadius: 8)
            VStack(alignment: .leading, spacing: 4) {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text(mod.name).font(.subheadline.weight(.semibold)).lineLimit(2)
                    if isModInstalled(mod.id) {
                        Image(systemName: "checkmark.circle.fill")
                            .font(.caption)
                            .foregroundStyle(.green)
                            .accessibilityLabel("Installed")
                    }
                }
                Text(mod.author).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                HStack(spacing: 8) {
                    if let category = mod.categoryName {
                        Text(category)
                            .font(.caption2.weight(.medium))
                            .padding(.horizontal, 6)
                            .padding(.vertical, 2)
                            .background(.quaternary, in: Capsule())
                    }
                    if mod.likeCount > 0 {
                        HStack(spacing: 3) {
                            Image(systemName: "heart.fill")
                            Text("\(mod.likeCount)")
                        }
                        .font(.caption2).foregroundStyle(.secondary)
                    }
                    if let updated = relativeDate(mod.dateUpdated) {
                        Text(updated).font(.caption2).foregroundStyle(.secondary)
                    }
                }
            }
        }
        .padding(.vertical, 4)
    }

    @ViewBuilder private var detail: some View {
        if let mod = selectedMod {
            ScrollView {
                VStack(alignment: .leading, spacing: 20) {
                    modHeader(mod)
                    VStack(alignment: .leading, spacing: 14) {
                        HStack {
                            Text("Available downloads").font(.headline)
                            Spacer()
                            if !files.isEmpty {
                                Text("\(files.count) \(files.count == 1 ? "file" : "files")")
                                    .font(.caption).foregroundStyle(.secondary)
                            }
                        }
                        Text("Choose the file that matches your game version.")
                            .font(.callout).foregroundStyle(.secondary)
                        if loadingFiles {
                            ProgressView("Loading files…").padding(.vertical)
                        } else if files.isEmpty {
                            Text("No downloadable files found.").foregroundStyle(.secondary)
                            Button("Reload files", action: loadFiles).disabled(installing)
                        }
                        ForEach(files, id: \.id) { file in
                            fileCard(file, mod: mod)
                        }
                    }
                    Text(compatibilityNote).font(.caption).foregroundStyle(.secondary)
                }
                .frame(maxWidth: 760, alignment: .leading)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(20)
            }
        } else {
            ContentUnavailableView("Choose a mod", systemImage: "square.stack.3d.up", description: Text("Search or choose a section, then select a mod to see its downloads."))
        }
    }

    private func modHeader(_ mod: BananaMod) -> some View {
        VStack(alignment: .leading, spacing: 14) {
            if let hero = mod.heroURL ?? mod.thumbnailURL {
                ModThumbnail(url: hero, width: nil, height: 190, cornerRadius: 14)
            }
            VStack(alignment: .leading, spacing: 8) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(mod.name).font(.title2.weight(.bold))
                        .fixedSize(horizontal: false, vertical: true)
                    if let version = mod.version {
                        Text("v\(version)")
                            .font(.caption.weight(.semibold))
                            .padding(.horizontal, 8)
                            .padding(.vertical, 3)
                            .background(.quaternary, in: Capsule())
                    }
                }
                HStack(spacing: 12) {
                    metaItem(mod.author, systemImage: "person.fill")
                    if let category = mod.categoryName {
                        metaItem(category, systemImage: "tag.fill")
                    }
                    if mod.likeCount > 0 {
                        metaItem("\(mod.likeCount)", systemImage: "heart.fill")
                    }
                    if mod.viewCount > 0 {
                        metaItem(mod.viewCount.formatted(.number.locale(Locale(identifier: "en_US_POSIX"))), systemImage: "eye.fill")
                    }
                    if let updated = relativeDate(mod.dateUpdated) {
                        metaItem(updated, systemImage: "clock")
                    }
                }
                .font(.caption)
                .foregroundStyle(.secondary)

                Link(destination: mod.pageURL) {
                    Label("Open on GameBanana", systemImage: "arrow.up.right.square")
                        .font(.callout.weight(.medium))
                }
            }
        }
    }

    private func fileCard(_ file: BananaFile, mod: BananaMod) -> some View {
        let installed = isFileInstalled(modID: mod.id, fileID: file.id)
        #if os(iOS)
        let patchOnly = ["ips", "pchtxt"].contains((file.name as NSString).pathExtension.lowercased())
        #else
        let patchOnly = false
        #endif
        return VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .top, spacing: 8) {
                Text(file.name)
                    .font(.system(.body, design: .monospaced).weight(.semibold))
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                if let version = file.version {
                    Text("v\(version)")
                        .font(.caption2.weight(.semibold))
                        .padding(.horizontal, 7)
                        .padding(.vertical, 3)
                        .background(.quaternary, in: Capsule())
                }
            }
            if let note = file.note {
                Text(note)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            HStack(spacing: 14) {
                metaItem(ByteCountFormatter.string(fromByteCount: file.size, countStyle: .file), systemImage: "archivebox")
                if file.downloadCount > 0 {
                    metaItem(file.downloadCount.formatted(.number.locale(Locale(identifier: "en_US_POSIX"))), systemImage: "arrow.down.to.line")
                }
                if let added = relativeDate(file.dateAdded) {
                    metaItem(added, systemImage: "calendar")
                }
                Spacer(minLength: 8)
                if installed {
                    Label("Installed", systemImage: "checkmark.circle.fill")
                        .font(.callout.weight(.semibold))
                        .foregroundStyle(.green)
                        .padding(.horizontal, 12)
                        .padding(.vertical, 6)
                        .background(.green.opacity(0.14), in: Capsule())
                } else if patchOnly {
                    Label("Mac rebuild required", systemImage: "hammer")
                        .font(.caption.weight(.medium))
                        .foregroundStyle(.orange)
                } else {
                    Button("Install", systemImage: "arrow.down.circle.fill") { install(file, for: mod) }
                        .buttonStyle(.borderedProminent)
                        .controlSize(.regular)
                        .accessibilityLabel("Install \(file.name)")
                        .disabled(installing)
                }
            }
            .font(.caption)
            .foregroundStyle(.secondary)
        }
        .padding(16)
        .background(.quaternary.opacity(0.45), in: RoundedRectangle(cornerRadius: 14))
        .overlay { RoundedRectangle(cornerRadius: 14).strokeBorder(.separator.opacity(0.5)) }
    }

    private var statusMessage: some View {
        HStack(spacing: 12) {
            Label(error ?? message ?? "", systemImage: error == nil ? "checkmark.circle.fill" : "exclamationmark.triangle.fill")
                .font(.callout)
                .foregroundStyle(error == nil ? Color.green : Color.red)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
            Button {
                error = nil
                message = nil
            } label: {
                Image(systemName: "xmark")
                    .font(.caption.weight(.bold))
                    .foregroundStyle(.secondary)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Dismiss message")
        }
        .padding(14)
        .background(.bar)
    }

    private func refreshInstalled() {
        let modsDir = caselessChild(folder, "mods") ?? folder.appendingPathComponent("mods")
        installedFolderNames = Set((try? FileManager.default.contentsOfDirectory(atPath: modsDir.path)) ?? [])
    }

    private func isFileInstalled(modID: Int, fileID: Int) -> Bool {
        let suffix = "[GB\(modID)-\(fileID)]"
        return installedFolderNames.contains { $0.hasSuffix(suffix) }
    }

    private func isModInstalled(_ modID: Int) -> Bool {
        let token = "[GB\(modID)-"
        return installedFolderNames.contains { $0.contains(token) }
    }

    private func metaItem(_ text: String, systemImage: String) -> some View {
        HStack(spacing: 4) {
            Image(systemName: systemImage)
            Text(text)
        }
    }

    private func relativeDate(_ timestamp: Int?) -> String? {
        guard let timestamp, timestamp > 0 else { return nil }
        let formatter = RelativeDateTimeFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.unitsStyle = .abbreviated
        return formatter.localizedString(for: Date(timeIntervalSince1970: TimeInterval(timestamp)), relativeTo: Date())
    }

    private var compatibilityNote: String {
        #if os(macOS)
        "Files install into your mods folder. Check the author's game-version requirements. Code patches need Prepare before Play."
        #else
        "Files install into this app's mods folder. iOS supports RomFS-only mods; code patches require rebuilding on a Mac."
        #endif
    }

    private func submit() {
        guard !installing else { return }
        searchFocused = false
        submittedQuery = query.trimmingCharacters(in: .whitespacesAndNewlines)
        search(page: 1)
    }

    private func search(page next: Int) {
        guard !installing else { return }
        searchTask?.cancel()
        filesTask?.cancel()
        page = next
        searching = true
        loadingFiles = false
        selection = nil
        files = []
        error = nil
        message = nil
        let term = submittedQuery
        let category = categoryID
        let currentSort = sort
        searchTask = Task {
            do {
                let found = try await GameBanana.search(term, page: next, categoryID: category, sort: currentSort)
                try Task.checkCancellation()
                results = found
                searching = false
                #if os(iOS)
                if horizontalSizeClass != .compact { selection = found.first?.id }
                #else
                selection = found.first?.id
                #endif
            } catch {
                guard !Task.isCancelled else { return }
                results = []
                searching = false
                self.error = error.localizedDescription
            }
        }
    }

    private func loadCategories() {
        categoriesTask?.cancel()
        loadingCategories = true
        categoryError = nil
        categoriesTask = Task {
            do {
                let found = try await GameBanana.categories()
                try Task.checkCancellation()
                categories = found
                loadingCategories = false
            } catch {
                guard !Task.isCancelled else { return }
                loadingCategories = false
                categoryError = error.localizedDescription
            }
        }
    }

    private func loadFiles() {
        filesTask?.cancel()
        files = []
        loadingFiles = false
        error = nil
        guard let mod = selectedMod else { return }
        loadingFiles = true
        filesTask = Task {
            do {
                let found = try await GameBanana.files(for: mod)
                try Task.checkCancellation()
                files = found
                loadingFiles = false
            } catch {
                guard !Task.isCancelled else { return }
                loadingFiles = false
                self.error = error.localizedDescription
            }
        }
    }

    private func install(_ file: BananaFile, for mod: BananaMod) {
        guard !installing else { return }
        installing = true
        activeFile = file
        downloadProgress = BananaDownloadProgress(receivedBytes: 0, totalBytes: file.size > 0 ? file.size : nil)
        let token = UUID()
        transferID = token
        error = nil
        message = nil
        installTask = Task {
            defer {
                installing = false
                activeFile = nil
                transferID = nil
                downloadProgress = nil
                installTask = nil
            }
            do {
                let name = try await ModInstaller.install(mod: mod, file: file, in: folder) { update in
                    Task { @MainActor in
                        guard transferID == token else { return }
                        // Late delegate events must not rewind bytes or the installation stage.
                        if let update {
                            guard let current = downloadProgress, update.receivedBytes >= current.receivedBytes else { return }
                        }
                        downloadProgress = update
                    }
                }
                message = "Installed \(name). You can turn it on or off in Mods."
                refreshInstalled()
                onInstalled(name)
            } catch {
                if Task.isCancelled { message = "Download cancelled." }
                else { self.error = error.localizedDescription }
            }
        }
    }
}

private struct ModThumbnail: View {
    let url: URL?
    let width: CGFloat?
    let height: CGFloat
    let cornerRadius: CGFloat

    var body: some View {
        Group {
            if let url {
                AsyncImage(url: url) { phase in
                    switch phase {
                    case .success(let image):
                        image.resizable().scaledToFill()
                    default:
                        placeholder
                    }
                }
            } else {
                placeholder
            }
        }
        .frame(width: width, height: height)
        .frame(maxWidth: width == nil ? .infinity : nil)
        .clipShape(RoundedRectangle(cornerRadius: cornerRadius))
        .overlay {
            RoundedRectangle(cornerRadius: cornerRadius)
                .strokeBorder(.separator.opacity(0.35))
        }
    }

    private var placeholder: some View {
        ZStack {
            LinearGradient(colors: [.secondary.opacity(0.22), .secondary.opacity(0.08)],
                           startPoint: .topLeading, endPoint: .bottomTrailing)
            Image(systemName: "flag.checkered")
                .font(.title3)
                .foregroundStyle(.secondary)
        }
    }
}

private struct ModDownloadStatus: View {
    let file: BananaFile
    let progress: BananaDownloadProgress?
    let cancel: () -> Void
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize
    @ScaledMetric(relativeTo: .caption) private var ringSize = 54.0

    private var phase: String {
        guard let progress else { return "Installing…" }
        return progress.fraction == 1 ? "Checking download…" : "Downloading…"
    }

    var body: some View {
        let layout = dynamicTypeSize.isAccessibilitySize
            ? AnyLayout(VStackLayout(alignment: .leading, spacing: 12))
            : AnyLayout(HStackLayout(spacing: 16))
        layout {
            ZStack {
                if let fraction = progress?.fraction {
                    Circle().stroke(.secondary.opacity(0.2), lineWidth: 4)
                    Circle().trim(from: 0, to: fraction)
                        .stroke(.tint, style: StrokeStyle(lineWidth: 4, lineCap: .round))
                        .rotationEffect(.degrees(-90))
                        .animation(reduceMotion ? nil : .easeOut(duration: 0.15), value: fraction)
                    Text(fraction, format: .percent.precision(.fractionLength(0)))
                        .font(.caption.weight(.semibold)).monospacedDigit()
                } else {
                    ProgressView().controlSize(.large)
                }
            }
            .frame(width: ringSize, height: ringSize)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(phase)
            .accessibilityValue(progress?.fraction.map { "\(Int(($0 * 100).rounded())) percent" } ?? "In progress")
            VStack(alignment: .leading, spacing: 4) {
                Text(phase).font(.headline)
                Text(file.name).font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                Text(byteDescription).font(.caption).foregroundStyle(.secondary).monospacedDigit()
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            Button("Cancel", action: cancel).buttonStyle(.bordered).controlSize(.large)
        }
    }

    private var byteDescription: String {
        guard let progress else { return "Extracting and checking mod files" }
        let received = ByteCountFormatter.string(fromByteCount: progress.receivedBytes, countStyle: .file)
        guard let total = progress.totalBytes else { return received }
        return "\(received) / \(ByteCountFormatter.string(fromByteCount: total, countStyle: .file))"
    }
}
