// Mush8 - ModContents.swift
import SwiftUI

struct ModContentsView: View {
    let mod: Mod
    let folder: URL
    @Environment(\.dismiss) private var dismiss
    @State private var tree: ModFileTree?
    @State private var error: String?
    @State private var filter = ""
    @State private var collapsed: Set<String> = []

    private var visibleEntries: [ModFileEntry] {
        guard let entries = tree?.entries else { return [] }
        let trimmed = filter.trimmingCharacters(in: .whitespacesAndNewlines)
        if !trimmed.isEmpty {
            return entries.filter { $0.path.localizedCaseInsensitiveContains(trimmed) }
        }
        return entries.filter { entry in
            !collapsed.contains { prefix in
                entry.path != prefix && entry.path.hasPrefix(prefix + "/")
            }
        }
    }

    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 0) {
                VStack(alignment: .leading, spacing: 8) {
                    Text(mod.displayTitle).font(.headline).textSelection(.enabled)
                    HStack(spacing: 12) {
                        Label(mod.path, systemImage: "folder").font(.caption.monospaced()).foregroundStyle(.secondary).textSelection(.enabled)
                        if let tree {
                            let fileCount = tree.entries.filter { !$0.directory && !$0.symbolicLink }.count
                            let totalBytes = tree.entries.reduce(Int64(0)) { $0 + ($1.directory || $1.symbolicLink ? 0 : $1.size) }
                            Spacer()
                            Text("\(fileCount) files · \(ByteCountFormatter.string(fromByteCount: totalBytes, countStyle: .file))")
                                .font(.caption).foregroundStyle(.secondary).monospacedDigit()
                        }
                    }
                    if !canDeleteMod(mod) {
                        Text("Shared Atmosphere folder: files are shown read-only.").font(.caption).foregroundStyle(.secondary)
                    }
                    HStack(spacing: 6) {
                        Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                        TextField("Filter files in mod…", text: $filter).textFieldStyle(.plain)
                        if !filter.isEmpty {
                            Button { filter = "" } label: {
                                Image(systemName: "xmark.circle.fill").foregroundStyle(.secondary)
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    .padding(.horizontal, 10)
                    .padding(.vertical, 6)
                    .background(.quaternary.opacity(0.6), in: RoundedRectangle(cornerRadius: 8))
                }
                .padding()
                Divider()
                if let tree {
                    List(visibleEntries) { entry in
                        HStack(alignment: .center, spacing: 8) {
                            if entry.directory && filter.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                                Button {
                                    if collapsed.contains(entry.path) { collapsed.remove(entry.path) }
                                    else { collapsed.insert(entry.path) }
                                } label: {
                                    Image(systemName: collapsed.contains(entry.path) ? "chevron.right" : "chevron.down")
                                        .font(.caption.weight(.semibold))
                                        .foregroundStyle(.secondary)
                                        .frame(width: 14)
                                }
                                .buttonStyle(.plain)
                            } else {
                                Spacer().frame(width: 14)
                            }
                            Image(systemName: entry.symbolicLink ? "link" : (entry.directory ? "folder.fill" : "doc.text"))
                                .foregroundStyle(entry.directory ? Color.accentColor : .secondary)
                                .frame(width: 20)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(filter.isEmpty ? entry.name : entry.path)
                                    .font(.system(.callout, design: .monospaced).weight(entry.directory ? .semibold : .regular))
                                    .textSelection(.enabled)
                                if entry.symbolicLink { Text("Symbolic link · not followed").font(.caption).foregroundStyle(.secondary) }
                            }
                            Spacer(minLength: 8)
                            if !entry.directory && !entry.symbolicLink {
                                Text(ByteCountFormatter.string(fromByteCount: entry.size, countStyle: .file))
                                    .font(.caption).foregroundStyle(.secondary).monospacedDigit()
                            }
                        }
                        .padding(.leading, filter.isEmpty ? CGFloat(min(entry.depth, 10)) * 14 : 0)
                        .accessibilityElement(children: .combine)
                        .accessibilityLabel("\(entry.path), \(entry.symbolicLink ? "symbolic link" : (entry.directory ? "folder" : "file"))")
                    }
                    .listStyle(.plain)
                    if tree.truncated {
                        Text("Showing the first 2,000 entries, up to 32 folder levels.")
                            .font(.caption).foregroundStyle(.secondary).padding()
                    }
                } else if let error {
                    ContentUnavailableView("Files could not be read", systemImage: "folder.badge.questionmark", description: Text(error))
                } else {
                    ProgressView("Reading files…").frame(maxWidth: .infinity, maxHeight: .infinity)
                }
            }
            .navigationTitle("Mod Files")
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
            }
        }
        #if os(macOS)
        .frame(minWidth: 560, idealWidth: 700, minHeight: 440, idealHeight: 560)
        #endif
        .task {
            let worker = Task.detached(priority: .userInitiated) { try modFileTree(mod, in: folder) }
            do {
                tree = try await withTaskCancellationHandler { try await worker.value } onCancel: { worker.cancel() }
            } catch is CancellationError {
            } catch { self.error = error.localizedDescription }
        }
    }
}
