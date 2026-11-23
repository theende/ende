// Unicode 17 data: a script, an emoji and a case mapping added in 17.0.
console.log(/\p{Script=Sidetic}/u.test("\u{10940}"));
console.log(/\p{Emoji}/u.test("\u{1FAC8}"));
console.log("\u{16EA0}".toLowerCase().codePointAt(0)!.toString(16));

// The type checker follows the engine: identifiers and regular expression
// script names added in Unicode 16 (Garay) and 17 pass `ende check`.
// The formatter would replace the escapes with the characters themselves.
// deno-fmt-ignore
const \u{16EA0} = 1;
// deno-fmt-ignore
const \u{10D4A} = 2;
// deno-fmt-ignore
console.log(\u{16EA0} + \u{10D4A});
console.log(/\p{Script=Garay}/u.test("\u{10D4A}"));
console.log(/\p{scx=Tolong_Siki}/u.test("\u{11DB0}"));
console.log(/\p{sc=Berf}/u.test("\u{16EA0}"));
