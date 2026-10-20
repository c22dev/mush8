// Build with macos/WolverineUSB.swift. Close games/probes that already own the USB interface.
import Foundation

@main struct VerifyWolverineUSB {
    static func main() {
        let seconds = CommandLine.arguments.dropFirst().first.flatMap(Double.init) ?? 30
        guard seconds.isFinite, (1...300).contains(seconds) else {
            fputs("Usage: verify-wolverine-usb [seconds: 1...300]\n", stderr)
            exit(2)
        }
        let started = Date()
        var previous = Data()
        var status = ""
        var changes = 0
        while Date().timeIntervalSince(started) < seconds {
            var bytes = [UInt8](repeating: 0, count: 20)
            let id = mk8WolverinePoll(&bytes, bytes.count)
            let current = WolverineUSB.shared.status
            if current != status { print(current); status = current; fflush(stdout) }
            if id != 0 && Data(bytes) != previous {
                changes += 1
                previous = Data(bytes)
                print("XUSB", bytes.map { String(format: "%02x", $0) }.joined(separator: " "))
                fflush(stdout)
            }
            Thread.sleep(forTimeInterval: 1.0 / 240.0)
        }
        print("Changed input snapshots:", changes)
    }
}
