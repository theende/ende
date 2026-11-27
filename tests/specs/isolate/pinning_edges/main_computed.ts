// The sandbox is reached only through a computed import.
const name = "./vendor/m/" + "plain.ts";
const { both } = await import(name);
console.log(await both());
