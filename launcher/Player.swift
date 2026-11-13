// Mush8 - Player.swift
// Constantin Clerc (c22dev) - 0.1
import SwiftUI

private let notCountries: Set<String> = ["AC", "CP", "DG", "EA", "EU", "EZ", "IC", "QO", "TA", "UN", "XK"]

let countryCodes: [String] = Locale.Region.isoRegions.map(\.identifier)
    .filter { $0.count == 2 && $0.allSatisfy(\.isLetter) && !notCountries.contains($0) }
    .sorted { countryName($0).compare(countryName($1), locale: appLocale) == .orderedAscending }

let appLocale = Locale(identifier: Bundle.main.preferredLocalizations.first ?? "en")

func countryName(_ code: String) -> String { appLocale.localizedString(forRegionCode: code) ?? code }

func playerNickname(nextendo: Bool, source: String, custom: String, nextendoName: String) -> String {
    let custom = custom.trimmingCharacters(in: .whitespaces)
    if nextendo && source != "custom" { return nextendoName }
    return custom.isEmpty ? nextendoName : custom
}

func playerCountry(nextendo: Bool, country: String) -> String { nextendo ? country : "" }

func limitedNickname(_ name: String) -> String {
    var out = ""
    for letter in name {
        guard out.unicodeScalars.count + letter.unicodeScalars.count <= 10, out.utf8.count + letter.utf8.count <= 0x1F else { break }
        out.append(letter)
    }
    return out
}

struct PlayerIdentity: View {
    let nextendo: Bool
    let nextendoName: String
    @Binding var nameSource: String
    @Binding var name: String
    @Binding var country: String

    var body: some View {
        if nextendo {
            Picker("Player name", selection: $nameSource) {
                Text(nextendoName.isEmpty ? "Nextendo" : "Nextendo (\(nextendoName))").tag("nextendo")
                Text("Custom").tag("custom")
            }
        }
        if !nextendo || nameSource == "custom" {
            TextField(nextendo ? "Name" : "Player name", text: $name, prompt: Text(nextendoName.isEmpty ? "Player" : nextendoName))
                .onChange(of: name) { name = limitedNickname(name) }
        }
        if nextendo {
            Picker("Country", selection: $country) {
                Text("None").tag("")
                ForEach(countryCodes, id: \.self) { Text(countryName($0)).tag($0) }
            }
        }
    }
}
