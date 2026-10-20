// Native app-local USB input for Razer Wolverine V3 TE 8K PC (1532:0a5a).
// No virtual HID device, system extension or external USB library is installed.
// Registry/IOUSBHost API usage adapted from OpenJoystickDriver (MIT); see THIRD-PARTY-NOTICES.md.
import Foundation
import IOKit
import IOUSBHost

final class WolverineUSB: @unchecked Sendable {
    static let shared = WolverineUSB()
    private let queue = DispatchQueue(label: "org.mush8.wolverine-usb", qos: .userInteractive)
    private let lock = NSLock()
    private var started = false
    private var report = Data()
    private var deviceID: UInt64 = 0
    private var message = "Razer USB：尚未检测"
    private var timer: DispatchSourceTimer?
    // The USB objects, generation and open ID are confined to queue.
    private var interface: IOUSBHostInterface?
    private var input: IOUSBHostPipe?
    private var openedID: UInt64 = 0
    private var generation: UInt64 = 0
    private var muted = false

    var status: String { lock.lock(); defer { lock.unlock() }; return message }
    private func publish(_ text: String, id: UInt64 = 0, data: Data = Data()) {
        lock.lock(); message = text; deviceID = id; report = data; lock.unlock()
    }
    func start() {
        lock.lock()
        guard !started else { lock.unlock(); return }
        started = true
        lock.unlock()
        queue.async { [self] in
            let timer = DispatchSource.makeTimerSource(queue: self.queue)
            timer.schedule(deadline: .now(), repeating: .seconds(1))
            timer.setEventHandler { [weak self] in self?.scan() }
            self.timer = timer
            timer.resume()
        }
    }
    func snapshot(into bytes: UnsafeMutablePointer<UInt8>, capacity: Int) -> UInt64 {
        start()
        lock.lock(); defer { lock.unlock() }
        guard capacity >= 20, deviceID != 0, report.count == 20 else { return 0 }
        report.copyBytes(to: bytes, count: 20)
        return deviceID
    }
    // Prefer the OS controller route if it already owns a Wolverine, avoiding double input.
    func setMuted(_ muted: Bool) {
        queue.async {
            guard self.muted != muted else { return }
            self.muted = muted
            if muted { self.close("Razer USB：使用系统手柄输入") }
            else { self.scan() }
        }
    }
    private func property(_ service: io_service_t, _ key: String) -> Int? {
        (IORegistryEntrySearchCFProperty(service, kIOServicePlane, key as CFString, kCFAllocatorDefault,
             IOOptionBits(kIORegistryIterateRecursively | kIORegistryIterateParents)) as? NSNumber)?.intValue
    }
    private func scan() {
        guard !muted else { return }
        var iterator: io_iterator_t = 0
        guard IOServiceGetMatchingServices(kIOMainPortDefault, IOServiceMatching("IOUSBHostInterface"), &iterator) == KERN_SUCCESS else {
            close("Razer USB：无法枚举 USB 接口"); return
        }
        defer { IOObjectRelease(iterator) }
        while case let service = IOIteratorNext(iterator), service != 0 {
            defer { IOObjectRelease(service) }
            guard property(service, "idVendor") == 0x1532, property(service, "idProduct") == 0x0a5a,
                  property(service, "bInterfaceNumber") == 0, property(service, "bInterfaceClass") == 0xff,
                  property(service, "bInterfaceSubClass") == 0x5d, property(service, "bInterfaceProtocol") == 1 else { continue }
            var id: UInt64 = 0
            guard IORegistryEntryGetRegistryEntryID(service, &id) == KERN_SUCCESS, id != 0 else { continue }
            if interface != nil && openedID == id { return }
            close("Razer USB：正在连接")
            do {
                let opened = try IOUSBHostInterface(__ioService: service, options: [], queue: queue, interestHandler: nil)
                do {
                    // Query XUSB input capabilities on every new interface, including hot-plug.
                    // This reply describes capabilities, never a live button state.
                    let capabilities = try opened.ioData(withCapacity: 20)
                    var transferred = 0
                    let request = IOUSBDeviceRequest(bmRequestType: 0xc1, bRequest: 1, wValue: 0x0100, wIndex: 0, wLength: 20)
                    try opened.__send(request, data: capabilities, bytesTransferred: &transferred, completionTimeout: 1)
                    let pipe = try opened.copyPipe(withAddress: 0x81)
                    let buffer = try opened.ioData(withCapacity: 64)
                    interface = opened; input = pipe; openedID = id
                    generation &+= 1
                    publish("Razer USB：已打开，等待输入")
                    read(pipe, buffer: buffer, generation: generation)
                    // Standard XUSB player-one indicator; no firmware/vendor configuration is written.
                    let output = try opened.copyPipe(withAddress: 0x01)
                    let led = try opened.ioData(withCapacity: 3)
                    [UInt8(1), 3, 6].withUnsafeBytes { led.mutableBytes.copyMemory(from: $0.baseAddress!, byteCount: 3) }
                    try output.enqueueIORequest(with: led, completionTimeout: 0) { _, _ in _ = led }
                } catch {
                    if interface == nil { opened.destroy() }
                    throw error
                }
            } catch {
                close("Razer USB：连接失败（\((error as NSError).code)），请检查其他应用是否占用")
            }
            return // One exact-model raw USB controller at a time.
        }
        close("Razer USB：未连接 Wolverine V3 TE 8K PC")
    }
    private func read(_ pipe: IOUSBHostPipe, buffer: NSMutableData, generation expected: UInt64) {
        do {
            // Interrupt endpoints must use zero timeout; one request stays queued continuously.
            try pipe.enqueueIORequest(with: buffer, completionTimeout: 0) { [weak self] result, count in
                guard let self, expected == self.generation else { return }
                guard result == kIOReturnSuccess else {
                    self.close("Razer USB：输入断开（\(String(format: "%08x", result))）"); return
                }
                if count == 20 {
                    let data = Data(bytes: buffer.bytes, count: count)
                    if data[0] == 0 && data[1] == 20 {
                        self.publish("Razer USB：Wolverine V3 TE 8K PC 已就绪", id: self.openedID, data: data)
                    }
                }
                self.read(pipe, buffer: buffer, generation: expected)
            }
        } catch { close("Razer USB：读取失败（\((error as NSError).code)）") }
    }
    private func close(_ reason: String) {
        generation &+= 1
        let old = interface
        interface = nil; openedID = 0
        try? input?.__abort(with: .asynchronous)
        input = nil
        old?.destroy()
        publish(reason)
    }
}

@_cdecl("mk8WolverinePoll")
func mk8WolverinePoll(_ bytes: UnsafeMutablePointer<UInt8>?, _ capacity: Int) -> UInt64 {
    guard let bytes else { return 0 }
    return WolverineUSB.shared.snapshot(into: bytes, capacity: capacity)
}

@_cdecl("mk8WolverinePreferSystem")
func mk8WolverinePreferSystem(_ preferred: Int32) { WolverineUSB.shared.setMuted(preferred != 0) }
