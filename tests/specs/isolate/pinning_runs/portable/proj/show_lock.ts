// Whether the lockfile's isolate section names a path of this machine.
const lock = JSON.parse(Ende.readTextFileSync("ende.lock"));
const text = JSON.stringify(lock.isolate);
console.log(/"(\/|[A-Za-z]:\\\\)/.test(text) ? "absolute" : "portable");
