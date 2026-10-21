// localhost isn't in the default --allow-import list.
import { returnsHi } from "http://localhost:4545/subdir/mod1.ts";

export const hi = returnsHi;
