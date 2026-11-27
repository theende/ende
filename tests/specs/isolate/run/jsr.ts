import { add } from "jsr:@endetest/multiple-exports@1/add";
import { subtract } from "jsr:@endetest/multiple-exports@1/subtract";

const sum = add(1, 2);
console.log(sum instanceof Promise, await sum, await subtract(3, 2));
