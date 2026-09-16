// Compiled with the actual pinned original and patched routines by check-ctc-tensors.py.
var cases = 0
for type in [MLMultiArrayDataType.float16, .float32, .double, .int32] {
    for rank in [3, 4] {
        for strided in [false, true] {
            let shape = rank == 3 ? [1, 17, 31] : [1, 31, 1, 17]
            let strides = rank == 3 ? [17 * 64, 64, 2] : [31 * 40, 40, 40, 2]
            let pointer = UnsafeMutableRawPointer.allocate(byteCount: 65536, alignment: 16)
            pointer.initializeMemory(as: UInt8.self, repeating: 0, count: 65536)
            do {
                let array: MLMultiArray
                if strided {
                    array = try MLMultiArray(dataPointer: pointer, shape: shape.map(NSNumber.init),
                        dataType: type, strides: strides.map(NSNumber.init), deallocator: nil)
                } else {
                    array = try MLMultiArray(shape: shape.map(NSNumber.init), dataType: type)
                }
                for t in 0..<17 {
                    for v in 0..<31 {
                        let index = rank == 3 ? [0, t, v] : [0, v, 0, t]
                        array[index.map(NSNumber.init)] = NSNumber(value: Float(t * 31 + v - 250) / 13)
                    }
                }
                for temperature: Float in [0.7, 1.0] {
                    for bias: Float in [0, 0.2] {
                        let expected = try Original().makeLogProbs(from: array, temperature: temperature, blankBias: bias)
                        let actual = try Patched().makeLogProbs(from: array, temperature: temperature, blankBias: bias)
                        precondition(actual.count == expected.count)
                        for (a, b) in zip(actual, expected) {
                            precondition(a.map(\.bitPattern) == b.map(\.bitPattern), "Strided tensor conversion changed model scores")
                        }
                        cases += 1
                    }
                }
            }
            pointer.deallocate()
        }
    }
}
for type in [MLMultiArrayDataType.float16, .float32] {
    for shape: [NSNumber] in [[1100], [1, 1100]] {
    for count in [0, 1, 1000, 1100] {
        let samples = (0..<count).map { Float($0 - 500) / 17 }
        let array = try MLMultiArray(shape: shape, dataType: type)
        // Allocators do not promise zeroed CoreML buffers. Poison all padding first.
        for i in 0..<array.count { array[i] = 123 }
        fill(array, samples)
        let expected = try MLMultiArray(shape: shape, dataType: type)
        for i in 0..<array.count { expected[i] = NSNumber(value: i < count ? samples[i] : 0) }
        for i in 0..<array.count {
            precondition(array[i].floatValue.bitPattern == expected[i].floatValue.bitPattern,
                "Input audio or zero padding changed")
        }
        cases += 1
    }
    }
}
let halfSamples = (UInt32(0)...UInt32(65535)).map { Float(Float16(bitPattern: UInt16($0))) }
let allHalves = try MLMultiArray(shape: [65536], dataType: .float16)
fill(allHalves, halfSamples)
for i in 0..<halfSamples.count {
    if halfSamples[i].isNaN { precondition(allHalves[i].floatValue.isNaN) }
    else { precondition(allHalves[i].floatValue.bitPattern == halfSamples[i].bitPattern) }
}
print("CTC tensor equivalence passed: \(cases) layout/padding cases and 65,536 half-float patterns")
