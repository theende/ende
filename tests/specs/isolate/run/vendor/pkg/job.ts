// Runs in a worker the sandbox starts: a module of its own sandbox, with
// its permissions.
import { count } from "./state.ts";

let read;
try {
  read = Ende.readTextFileSync("secret.txt");
} catch (e) {
  read = (e as Error).name;
}
postMessage(`${count()} ${read}`);
