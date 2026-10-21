import { add } from "./util.tsx";
import("./dynamic.tsx").then((m: { value: number }) =>
  console.log(add(m.value, 1))
);
