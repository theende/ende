// The engine has no Katakana_Or_Hiragana script, so the type checker rejects
// it too instead of passing code that fails to parse.
console.log(/\p{Script=Katakana_Or_Hiragana}/u.test("a"));
