import XCTest
@testable import Vulcan

final class VulcanTests: XCTestCase {
    private let fp = String(repeating: "ab", count: 32)
    private let token = String(repeating: "0f", count: 32)

    func testParsesPairingURL() throws {
        let url = URL(string: "vulcan://pair?h=192.168.1.139&p=47800&t=\(token)&f=\(fp.uppercased())&n=arch&d=My%20iPhone")!
        let p = try XCTUnwrap(Pairing(url: url))
        XCTAssertEqual(p.host, "192.168.1.139")
        XCTAssertEqual(p.port, 47800)
        XCTAssertEqual(p.token, token)
        XCTAssertEqual(p.fingerprint, fp, "fingerprint is normalised to lowercase")
        XCTAssertEqual(p.hostname, "arch")
        XCTAssertEqual(p.deviceName, "My iPhone")
    }

    func testRejectsBadPairingURLs() {
        XCTAssertNil(Pairing(url: URL(string: "https://example.com")!))
        XCTAssertNil(Pairing(url: URL(string: "vulcan://pair?h=x&p=1&t=short&f=\(fp)")!))
        XCTAssertNil(Pairing(url: URL(string: "vulcan://pair?h=x&t=\(token)&f=\(fp)")!))
    }

    func testCombos() {
        let c = Combo("Ctrl+Shift+T")
        XCTAssertEqual(c?.key, "T")
        XCTAssertEqual(c?.mods, ["ctrl", "shift"])
        XCTAssertEqual(Combo("Super")?.mods, [])
        XCTAssertNil(Combo("Hyper+X"))
        XCTAssertEqual(Combo.parseList("Ctrl+C\n\nAlt+Tab\nBogus+Q").map(\.id), ["Ctrl+C", "Alt+Tab"])
    }

    func testMessageEncoding() throws {
        let json = ClientMessage.key("q", mods: ["super"]).json
        let obj = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any])
        XCTAssertEqual(obj["t"] as? String, "key")
        XCTAssertEqual(obj["k"] as? String, "q")
        XCTAssertEqual(obj["mods"] as? [String], ["super"])
        XCTAssertEqual(obj["s"] as? String, "press")
        XCTAssertEqual(ServerMessage.parse(#"{"t":"paused","paused":true}"#)?.paused, true)
    }

    private func object(_ m: ClientMessage) throws -> [String: Any] {
        try XCTUnwrap(JSONSerialization.jsonObject(with: Data(m.json.utf8)) as? [String: Any])
    }

    func testControlMessages() throws {
        let media = try object(.media(.playPause))
        XCTAssertEqual(media["t"] as? String, "media")
        XCTAssertEqual(media["a"] as? String, "play_pause")

        let vol = try object(.volume(.set, value: 0.4))
        XCTAssertEqual(vol["a"] as? String, "set")
        XCTAssertEqual(vol["value"] as? Double, 0.4)
        XCTAssertNil(try object(.volume(.up))["value"], "no value key unless setting")

        XCTAssertEqual(try object(.power(.poweroff))["a"] as? String, "poweroff")
        XCTAssertEqual(try object(.display(.toggle))["t"] as? String, "display")
        XCTAssertEqual(try object(.display(.toggle))["a"] as? String, "toggle")
        XCTAssertNil(try object(.display(.toggle))["monitor"], "all monitors when no name")
        XCTAssertEqual(try object(.display(.off, monitor: "DP-2"))["monitor"] as? String, "DP-2")
        XCTAssertEqual(try object(.cmd("Workspace 2"))["id"] as? String, "Workspace 2")
        XCTAssertEqual(try object(.usb("Keyboard light"))["name"] as? String, "Keyboard light")
        XCTAssertNil(try object(.usb("Keyboard light"))["on"], "toggle when no state given")
        let clip = try object(.clipSet(.image(Data([1, 2]), mime: "image/png"), auto: true))
        XCTAssertEqual(clip["t"] as? String, "clip_set")
        XCTAssertEqual(clip["kind"] as? String, "image")
        XCTAssertEqual(clip["data"] as? String, "AQI=")
        XCTAssertEqual(clip["auto"] as? Bool, true)
        let settings = try object(.clipSettings(toPhone: nil, toPC: false))
        XCTAssertEqual(settings["to_pc"] as? Bool, false)
        XCTAssertNil(settings["to_phone"])
        XCTAssertTrue(ClientMessage.clipSettings(toPhone: true, toPC: nil).allowedWhilePaused)
        XCTAssertTrue(ClientMessage.refresh.allowedWhilePaused)
        XCTAssertFalse(ClientMessage.media(.next).allowedWhilePaused)
    }

    func testDecodesControlReplies() throws {
        let welcome = try XCTUnwrap(ServerMessage.parse(
            #"{"t":"welcome","v":1,"host":"arch","device":"iPhone","caps":["input","power"],"commands":[{"id":"WS 1","name":"WS 1","icon":"1.square"}]}"#
        ))
        XCTAssertEqual(welcome.caps, ["input", "power"])
        XCTAssertEqual(welcome.commands?.first?.icon, "1.square")
        let vol = try XCTUnwrap(ServerMessage.parse(#"{"t":"volume","level":0.5,"muted":true}"#))
        XCTAssertEqual(vol.level, 0.5)
        XCTAssertEqual(vol.muted, true)
        let displays = try XCTUnwrap(ServerMessage.parse(
            #"{"t":"displays","monitors":[{"name":"DP-2","label":"Left","x":0.0,"y":0.0,"w":3840.0,"h":2160.0,"on":false}]}"#
        ))
        XCTAssertEqual(displays.monitors?.first?.label, "Left")
        XCTAssertEqual(displays.monitors?.first?.on, false)
        let usb = try XCTUnwrap(ServerMessage.parse(
            #"{"t":"usb_devices","devices":[{"name":"Keyboard light","icon":"keyboard","on":false,"present":true}]}"#
        ))
        XCTAssertEqual(usb.devices?.first?.on, false)
        let clip = try XCTUnwrap(ServerMessage.parse(#"{"t":"clip","kind":"text","text":"hi","hash":"ab"}"#))
        XCTAssertEqual(clip.text, "hi")
        XCTAssertEqual(clip.hash, "ab")
        let settings = try XCTUnwrap(ServerMessage.parse(#"{"t":"clip_settings","to_phone":false,"to_pc":true}"#))
        XCTAssertEqual(settings.toPhone, false)
        XCTAssertEqual(settings.toPc, true)
        let media = try XCTUnwrap(ServerMessage.parse(#"{"t":"media","status":"Playing","title":"S","artist":"A","player":"spotify"}"#))
        XCTAssertEqual(media.status, "Playing")
    }
}
