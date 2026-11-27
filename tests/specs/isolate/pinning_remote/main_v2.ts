import { hasHome, version } from "jsr:@endetest/isolate-manifest@1.1.0";

console.log(await version(), await hasHome());
