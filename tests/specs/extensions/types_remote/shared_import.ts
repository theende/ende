import { answer } from "extension:@acme/remote";
// The extension's own module, imported by the app directly: the app's
// --allow-import applies to it, as it does when running.
import * as direct from "https://localhost:5545/run/extension_lockfile/remote_ext.ts";

console.log(direct, await answer());
