// Developer-only reference check. Not bundled or invoked by Macscan.
import Foundation
let root = URL(fileURLWithPath: "/System/Volumes/Data", isDirectory: true)
let values = try root.resourceValues(forKeys: [.volumeUUIDStringKey, .volumeAvailableCapacityKey, .volumeAvailableCapacityForImportantUsageKey])
print("uuid=\(values.volumeUUIDString ?? "unavailable")")
print("free=\(values.volumeAvailableCapacity.map(String.init) ?? "unavailable")")
print("important=\(values.volumeAvailableCapacityForImportantUsage.map(String.init) ?? "unavailable")")
