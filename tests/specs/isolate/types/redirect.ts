// The URL redirects into the sandboxed prefix, so running sandboxes it, and
// checking types it as its proxy presents it.
import { answer } from "https://localhost:5545/isolate/redirect_into_sandbox.ts";

const n: number = answer();
console.log(n);
