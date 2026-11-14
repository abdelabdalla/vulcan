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
}
