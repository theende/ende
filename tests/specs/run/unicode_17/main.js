// Unicode 17 data: a script, an emoji and a case mapping added in 17.0.
console.log(/\p{Script=Sidetic}/u.test("\u{10940}"));
console.log(/\p{Emoji}/u.test("\u{1FAC8}"));
console.log("\u{16EA0}".toLowerCase().codePointAt(0).toString(16));
