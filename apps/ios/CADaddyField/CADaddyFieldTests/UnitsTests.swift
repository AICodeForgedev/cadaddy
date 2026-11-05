import XCTest

final class UnitsTests: XCTestCase {
    func testFeetInchesFormatting() {
        XCTAssertEqual(Units.feetInches(0.4572), "1' 6\"")
        XCTAssertEqual(Units.feetInches(1.2192), "4' 0\"")
        XCTAssertEqual(Units.feetInches(2.6035), "8' 6 1/2\"")
        XCTAssertEqual(Units.feetInches(0.003175), "0' 1/8\"")
        XCTAssertEqual(Units.feetInches(0.009525), "0' 3/8\"")
        XCTAssertEqual(Units.feetInches(0.0), "0' 0\"")
        XCTAssertEqual(Units.feetInches(-0.3556), "-1' 2\"")
        XCTAssertEqual(Units.feetInches(.nan), "—")
        XCTAssertEqual(Units.feetInches(.infinity), "—")
        // Rounds to the nearest 1/8": 0.30 m = 11.811" -> 11 3/4".
        XCTAssertEqual(Units.feetInches(0.30), "0' 11 3/4\"")
        // 11.95" rounds up into the next foot.
        XCTAssertEqual(Units.feetInches(Units.meters(fromInches: 11.95)), "1' 0\"")
    }

    func testParseLengths() throws {
        func near(_ s: String, _ m: Double, file: StaticString = #filePath, line: UInt = #line) {
            guard let v = Units.parseLength(s) else { return XCTFail("nil for \(s)", file: file, line: line) }
            XCTAssertEqual(v, m, accuracy: 1e-9, s, file: file, line: line)
        }
        near("8' 6 1/2\"", 2.6035)
        near("8'-6 1/2\"", 2.6035)
        near("8'6\"", 2.5908)
        near("18\"", 0.4572)
        near("3/8\"", 0.009525)
        near("8.5'", 2.5908)
        near("-1' 2\"", -0.3556)
        near("2.59 m", 2.59)
        near("259 cm", 2.59)
        near("2590 mm", 2.59)
        near("1.25", 1.25)
        near("6 1/2″", 0.1651)
        for bad in ["", "abc", "1/0\"", "8' x\"", "1 2 3\"", String(repeating: "9", count: 100)] {
            XCTAssertNil(Units.parseLength(bad), bad)
        }
    }

    func testRoundTripMetersFeetInches() {
        for eighths in stride(from: 0, through: 12 * 8 * 30, by: 7) {
            let m = Units.meters(fromInches: Double(eighths) / 8)
            let text = Units.feetInches(m)
            guard let back = Units.parseLength(text) else { return XCTFail(text) }
            XCTAssertEqual(back, m, accuracy: 1e-9, text)
        }
    }

    func testConversions() {
        XCTAssertEqual(Units.inches(fromMeters: 0.0254), 1, accuracy: 1e-12)
        XCTAssertEqual(Units.meters(fromFeet: 10), 3.048, accuracy: 1e-12)
        XCTAssertEqual(Units.inchesText(0.0254 * 0.42), "0.42 in")
    }
}
