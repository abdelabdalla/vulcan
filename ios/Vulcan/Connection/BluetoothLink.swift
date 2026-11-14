import CoreBluetooth
import Foundation

/// Bluetooth LE fallback to the PC's remote-control service (see daemon/src/bluetooth.rs).
/// Carries the same JSON messages as the WebSocket, split into packets whose first byte is
/// 1 (more follows) or 0 (last). The link is encrypted by the phone's existing bond with the PC.
@MainActor
final class BluetoothLink: NSObject {
    static let service = CBUUID(string: "8F3E0001-5C2A-4E6B-9C1D-7A5B3E9F2C01")
    private static let rxUUID = CBUUID(string: "8F3E0002-5C2A-4E6B-9C1D-7A5B3E9F2C01")
    private static let txUUID = CBUUID(string: "8F3E0003-5C2A-4E6B-9C1D-7A5B3E9F2C01")

    var onMessage: ((String) -> Void)?
    var onClose: (() -> Void)?

    private var central: CBCentralManager?
    private var peripheral: CBPeripheral?
    private var rx: CBCharacteristic?
    private var tx: CBCharacteristic?
    private var reassembly = Data()
    private var outbox: [Data] = []
    private var ready: CheckedContinuation<Bool, Never>?
    private var poweredOn: CheckedContinuation<Bool, Never>?
    private var closed = false

    /// Find the PC (already connected to the phone, or by scanning) and open the link.
    func connect(timeout: TimeInterval = 8) async -> Bool {
        let central = CBCentralManager(delegate: self, queue: .main, options: [CBCentralManagerOptionShowPowerAlertKey: false])
        self.central = central
        if central.state != .poweredOn {
            let on = await withCheckedContinuation { continuation in
                poweredOn = continuation
                // Don't wait forever if Bluetooth never reports powered on.
                DispatchQueue.main.asyncAfter(deadline: .now() + timeout) { [weak self] in
                    MainActor.assumeIsolated {
                        self?.poweredOn?.resume(returning: false)
                        self?.poweredOn = nil
                    }
                }
            }
            guard on else { return false }
        }
        return await withCheckedContinuation { continuation in
            ready = continuation
            // The phone is usually already connected to the PC (notifications bond).
            if let known = central.retrieveConnectedPeripherals(withServices: [Self.service]).first {
                use(known)
            } else {
                central.scanForPeripherals(withServices: [Self.service])
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + timeout) { [weak self] in
                MainActor.assumeIsolated { self?.finishConnect(false) }
            }
        }
    }

    func send(_ text: String) {
        guard let peripheral, let rx else { return }
        let limit = Swift.max(peripheral.maximumWriteValueLength(for: .withoutResponse), 20)
        let bytes = Data(text.utf8)
        let payload = limit - 1
        var offset = 0
        repeat {
            let end = min(offset + payload, bytes.count)
            var packet = Data([end < bytes.count ? 1 : 0])
            packet.append(bytes[offset..<end])
            outbox.append(packet)
            offset = end
        } while offset < bytes.count
        flush(peripheral, rx)
    }

    func close() {
        guard !closed else { return }
        closed = true
        central?.stopScan()
        if let peripheral { central?.cancelPeripheralConnection(peripheral) }
        onClose?()
    }

    private func flush(_ peripheral: CBPeripheral, _ rx: CBCharacteristic) {
        while !outbox.isEmpty, peripheral.canSendWriteWithoutResponse {
            peripheral.writeValue(outbox.removeFirst(), for: rx, type: .withoutResponse)
        }
    }

    private func use(_ p: CBPeripheral) {
        peripheral = p
        p.delegate = self
        central?.connect(p)
    }

    private func finishConnect(_ ok: Bool) {
        guard let ready else { return }
        self.ready = nil
        if !ok { central?.stopScan() }
        ready.resume(returning: ok)
    }
}

extension BluetoothLink: CBCentralManagerDelegate {
    nonisolated func centralManagerDidUpdateState(_ central: CBCentralManager) {
        MainActor.assumeIsolated {
            switch central.state {
            case .poweredOn:
                poweredOn?.resume(returning: true)
                poweredOn = nil
            case .poweredOff, .unauthorized, .unsupported:
                poweredOn?.resume(returning: false)
                poweredOn = nil
                if peripheral != nil { close() }
            default:
                break
            }
        }
    }

    nonisolated func centralManager(_ central: CBCentralManager, didDiscover p: CBPeripheral, advertisementData: [String: Any], rssi: NSNumber) {
        MainActor.assumeIsolated {
            guard peripheral == nil else { return }
            central.stopScan()
            use(p)
        }
    }

    nonisolated func centralManager(_ central: CBCentralManager, didConnect p: CBPeripheral) {
        MainActor.assumeIsolated { p.discoverServices([Self.service]) }
    }

    nonisolated func centralManager(_ central: CBCentralManager, didFailToConnect p: CBPeripheral, error: Error?) {
        MainActor.assumeIsolated { finishConnect(false) }
    }

    nonisolated func centralManager(_ central: CBCentralManager, didDisconnectPeripheral p: CBPeripheral, error: Error?) {
        MainActor.assumeIsolated {
            finishConnect(false)
            if !closed {
                closed = true
                onClose?()
            }
        }
    }
}

extension BluetoothLink: CBPeripheralDelegate {
    nonisolated func peripheral(_ p: CBPeripheral, didDiscoverServices error: Error?) {
        MainActor.assumeIsolated {
            guard let service = p.services?.first(where: { $0.uuid == Self.service }) else { return finishConnect(false) }
            p.discoverCharacteristics([Self.rxUUID, Self.txUUID], for: service)
        }
    }

    nonisolated func peripheral(_ p: CBPeripheral, didDiscoverCharacteristicsFor service: CBService, error: Error?) {
        MainActor.assumeIsolated {
            rx = service.characteristics?.first { $0.uuid == Self.rxUUID }
            tx = service.characteristics?.first { $0.uuid == Self.txUUID }
            guard let tx, rx != nil else { return finishConnect(false) }
            p.setNotifyValue(true, for: tx)
        }
    }

    nonisolated func peripheral(_ p: CBPeripheral, didUpdateNotificationStateFor c: CBCharacteristic, error: Error?) {
        MainActor.assumeIsolated { finishConnect(error == nil && c.isNotifying) }
    }

    nonisolated func peripheral(_ p: CBPeripheral, didUpdateValueFor c: CBCharacteristic, error: Error?) {
        guard let value = c.value, let header = value.first else { return }
        MainActor.assumeIsolated {
            reassembly.append(value.dropFirst())
            if reassembly.count > 1 << 20 { reassembly.removeAll() }
            if header == 0 {
                let text = String(decoding: reassembly, as: UTF8.self)
                reassembly.removeAll()
                onMessage?(text)
            }
        }
    }

    nonisolated func peripheralIsReady(toSendWriteWithoutResponse p: CBPeripheral) {
        MainActor.assumeIsolated {
            if let rx { flush(p, rx) }
        }
    }
}
