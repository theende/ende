// Ende maps ./src/qux.ts to ./types/qux.ts; tsc on its own wouldn't.
import { qux } from "./src/qux.ts";

const label: string = qux;
console.log(label);
