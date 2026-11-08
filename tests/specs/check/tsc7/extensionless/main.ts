// The module has no extension; it's served as TypeScript.
import { loaded } from "http://localhost:4545/subdir/no_ext";

const name: string = loaded;
console.log(name);
