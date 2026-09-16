let originalMode = CommandLine.arguments.contains("--original")
let count = originalMode ? 4000 : 20000
let words = Array(repeating: "word", count: count)
let text = words.joined(separator: " ")
let done = DispatchSemaphore(value: 0)
let thread = Thread {
    let result = originalMode ? Original.alignBaseWordsToUTF8Ranges(baseText: text, baseWords: words)
        : Patched.alignBaseWordsToUTF8Ranges(baseText: text, baseWords: words)
    precondition(result.count == count && result.allSatisfy { $0 != nil })
    print("alignment words=\(count) stack_bytes=524288 passed")
    done.signal()
}
thread.stackSize = 524288
thread.start()
done.wait()
let alphabet = ["a", "b", " ", ".", "é", "🙂", "’", "#", "-"]
var seed: UInt64 = 731
func next(_ max: Int) -> Int { seed = seed &* 6364136223846793005 &+ 1; return Int((seed >> 32) % UInt64(max)) }
for _ in 0..<10000 {
    let text = (0..<next(14)).map { _ in alphabet[next(alphabet.count)] }.joined()
    let words = (0..<next(5)).map { _ in (0..<(next(3)+1)).map { _ in alphabet[next(alphabet.count)] }.joined() }
    precondition(Original.alignBaseWordsToUTF8Ranges(baseText: text, baseWords: words) == Patched.alignBaseWordsToUTF8Ranges(baseText: text, baseWords: words))
}
print("differential_cases=10000 passed")

let fixtures: [(String, [String])] = [
    ("a a a a a", ["a", "a", "a"]),
    ("x y x y", ["x", "y"]),
    ("candy", ["and"]),
    ("---", ["-", "-"]),
    ("“café,” 東京 C++ C# @home $5 .env", ["“café,”", "東京", "C++", "C#", "@home", "$5", ".env"]),
    ("é é", ["é", "é"]),
    ("a b", ["a", "", "b"])
]
for (text, words) in fixtures {
    precondition(Original.alignBaseWordsToUTF8Ranges(baseText: text, baseWords: words) == Patched.alignBaseWordsToUTF8Ranges(baseText: text, baseWords: words))
}
precondition(Patched.alignBaseWordsToUTF8Ranges(baseText: "---", baseWords: ["-", "-"]) == [nil, nil])
precondition(Patched.alignBaseWordsToUTF8Ranges(baseText: "“café,” 東京", baseWords: ["“café,”", "東京"]) == [3..<8, 13..<19])
print("ambiguity_unicode_fixtures=\(fixtures.count) passed")
