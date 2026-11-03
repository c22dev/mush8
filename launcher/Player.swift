// Mush8 - Player.swift
// Constantin Clerc (c22dev) - 0.1
import SwiftUI

let countryCodes: [String] = Locale.Region.isoRegions.map(\.identifier)
    .filter { $0.count == 2 && $0.allSatisfy(\.isLetter) }
    .sorted { countryName($0).localizedCompare(countryName($1)) == .orderedAscending }

func countryName(_ code: String) -> String { Locale.current.localizedString(forRegionCode: code) ?? code }

func playerNickname(nextendo: Bool, source: String, custom: String, nextendoName: String) -> String {
    nextendo && source != "custom" ? nextendoName : custom.trimmingCharacters(in: .whitespaces)
}

func playerCountry(nextendo: Bool, country: String) -> String { nextendo ? country : "" }

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
            TextField(nextendo ? "Name" : "Player name", text: $name, prompt: Text("Player"))
                .onChange(of: name) { if name.count > 10 { name = String(name.prefix(10)) } }
        }
        if nextendo {
            Picker("Country", selection: $country) {
                Text("None").tag("")
                ForEach(countryCodes, id: \.self) { Text(countryName($0)).tag($0) }
            }
        }
    }
}
